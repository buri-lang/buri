//! Shortcut fusion over `core/list`'s combinators.
//!
//! A chain such as `xs.map(ctx, f).filter(ctx, p).fold(g, 0)` builds two lists
//! nobody names: `map` allocates one and writes every element into it, `filter`
//! allocates two more and copies, and `fold` reads the last one back and drops
//! it. Measured on a 40,000-element pipeline run 2,000 times, that is 1.83 GB of
//! stores and the matching loads against only 6,205 heap blocks — so the cost is
//! the traffic and not the allocator, and deleting the intermediate list is
//! worth 2.4× on that pipeline, 2.7× on `map|map|fold` and 4.9× on
//! `range|map|filter|len`.
//!
//! This pass deletes them by *composing the steps* rather than by inventing a
//! fused loop: `fold(map(xs, f), g, z)` becomes `fold(xs, |a, x| g(a, f(x)), z)`,
//! with `f`'s body spliced into the accumulator step. Nothing downstream learns
//! a new shape — no new IR node, no new intrinsic key, no backend change —
//! because the result is the same combinator over a different list with a bigger
//! lambda, which `middle::closures` lifts and both native backends already
//! open-code as one loop.
//!
//! # Where it runs, and why that is the whole design
//!
//! **The native branch only** (`middle::native`), after `derives` and before
//! `closures`. The JavaScript backend is handed the tree before this pass and
//! sees exactly what it saw yesterday: no golden changes hands.
//!
//! That is not caution about the goldens. `cli/tests/native/agreement.rs`
//! compares the answers of the JavaScript artifact against both native ones, and
//! that comparison is the only mechanical check this rewrite has. Fusing in the
//! shared middle would fuse *both* sides identically, and a differential test
//! whose two sides share the transformation under test proves nothing about it.
//! Keeping the pass native-only makes JavaScript the reference implementation of
//! every pipeline in the corpus.
//!
//! The cost of that choice is that JavaScript does not get faster here. It is
//! the smaller loss: V8 allocates and collects an intermediate array with a bump
//! pointer and a generational nursery, which is a different and much cheaper
//! machine than `malloc` plus a copy, so the same rewrite is worth less there
//! than that differential test is worth here.
//!
//! # What licenses it
//!
//! Only the **context-free** combinators fuse: `map`, `filter`, `fold`, `count`,
//! `any`, `all`. Their step functions take no context, and SPEC 10.6 forbids a
//! lambda from capturing an effect-carrying value, so a step cannot print, read
//! a clock or observe an allocator — it is a pure function of its element.
//! Interleaving pure steps is unobservable, which is the whole argument. The
//! `*Ctx` variants thread a context precisely so that a step *can* have an
//! effect, and they are excluded for that reason.
//!
//! The intermediate list's own allocation is not observable either:
//! `core/alloc`'s counters are a type-level cost definition and exclude the
//! list, string and closure rows by design
//! (`standard_library/sources/alloc.buri`), so no program can count the block
//! this pass deletes.
//!
//! **The one residual divergence** is which of two *diverging* steps aborts
//! first. Unfused, `map`'s step runs on every element before `fold`'s step runs
//! on any; fused, they interleave. If `f` traps on element 3 and `g` traps on
//! element 0, the unfused program aborts inside `f` and the fused one inside
//! `g`. Both abort, on the same input, and no terminating program can tell the
//! difference. It is the standard caveat of shortcut fusion and it is recorded
//! here rather than left to be discovered.
//!
//! # What it will not touch
//!
//! * A step that is not written as a lambda at the call site. Composing a step
//!   that is a name means calling it from inside the fused lambda, which
//!   captures it, which allocates an environment and costs an indirect call per
//!   element — the traffic goes and the dispatch gets worse.
//! * A producer whose body contains `?`, a `Continue` or a `Loop`. `?` exits the
//!   lambda it is written in, and splicing a body moves which lambda that is.
//! * A dropped `ctx` argument that is anything but a read. The fused call does
//!   not build the producer's list, so the producer's `Allocator` argument is never
//!   evaluated, and an argument that was a call would have been.
//! * `filter(map(…))` and `map(filter(…))`, which are not fusions: the first
//!   would have to answer source elements where it answers mapped ones, and the
//!   second changes length.

use crate::compiler::middle::monomorphize::{Func, FuncKind, Program};
use crate::compiler::semantics::layered::Dense;
use crate::compiler::semantics::name::Name;
use crate::compiler::semantics::typed::{self, Expr, ExprKind, PatKind, Pattern, Stmt};
use crate::compiler::semantics::types::{FuncIdx, LocalId, Ty, TyKind};
use crate::diagnostics::Span;

/// Fuses every combinator chain in the program.
pub fn run(program: &mut Program) {
    for i in program.funcs.ids() {
        // Out of the table while it is fused, so the walk can mint into it.
        let Some(mut body) = program.funcs.get_mut(i).take_body() else {
            continue;
        };
        Fuse {
            funcs: &mut program.funcs,
        }
        .expr(&mut body);
        program.funcs.get_mut(i).set_body(body);
    }
}

/// Which `core/list` combinator a call is.
///
/// The argument positions below are `core/list`'s own declaration order, and
/// they are the same six keys `backend::intrinsic_keys::list_call` reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Combinator {
    /// `map(self, ctx, f)`.
    Map,
    /// `filter(self, ctx, keep)`.
    Filter,
    /// `fold(self, f, init)`.
    Fold,
    /// `count(self, pred)`.
    Count,
    /// `any(self, pred)`.
    Any,
    /// `all(self, pred)`.
    All,
}

/// The combinators this pass fuses, and the intrinsic key each is.
///
/// One table rather than two matches, because the two are inverses and a row
/// present in one and absent from the other is a combinator this pass would
/// recognise and be unable to write back.
const KEYS: &[(Combinator, &str)] = &[
    (Combinator::Map, "list.map"),
    (Combinator::Filter, "list.filter"),
    (Combinator::Fold, "list.fold"),
    (Combinator::Count, "list.count"),
    (Combinator::Any, "list.any"),
    (Combinator::All, "list.all"),
];

impl Combinator {
    fn of(key: &str) -> Option<Combinator> {
        KEYS.iter().find(|(_, k)| *k == key).map(|(c, _)| *c)
    }

    fn key(self) -> &'static str {
        match KEYS.iter().find(|(c, _)| *c == self) {
            Some((_, k)) => k,
            // Unreachable: `KEYS` has a row per variant, and `Combinator::of` is the
            // only way one is made.
            None => "",
        }
    }

    /// Where the step function sits in the argument list.
    fn step_at(self) -> usize {
        match self {
            Combinator::Map | Combinator::Filter => 2,
            Combinator::Fold | Combinator::Count | Combinator::Any | Combinator::All => 1,
        }
    }
}

struct Fuse<'a> {
    funcs: &'a mut Dense<FuncIdx, Func>,
}

impl Fuse<'_> {
    /// Depth first, then to fixpoint at this node: a three-stage chain fuses its
    /// outer pair first, and what is left in the same place is a two-stage one.
    fn expr(&mut self, e: &mut Expr) {
        typed::children_mut(e, &mut |child| self.expr(child));
        while self.once(e) {}
    }

    /// The combinator `f` is, if it is one.
    fn combinator(&self, f: FuncIdx) -> Option<Combinator> {
        // A chain fuses one stage at a time, so the instance this pass minted a
        // moment ago is exactly the one the next stage has to recognise.
        Combinator::of(self.funcs.get(f).intrinsic_key()?)
    }

    /// One fusion at this node, or nothing.
    fn once(&mut self, e: &mut Expr) -> bool {
        let ty = e.ty;
        let ExprKind::CallFunc { func, args } = &mut e.kind else {
            return false;
        };
        if self.funcs.get(*func).intrinsic_key() == Some("list.length") {
            return self.len_of_filter(ty, func, args);
        }
        match self.combinator(*func) {
            Some(consumer) => self.fuse(ty, consumer, func, args).is_some(),
            None => false,
        }
    }

    /// `filter(xs, ctx, keep).len()` is `count(xs, keep)`: the same predicate
    /// on the same elements in the same order, without the list in between.
    ///
    /// This one needs no lambda, because the step is passed through untouched
    /// rather than composed — and it is what puts a `len` at the end of a chain
    /// back in reach of the fusions above, which is where
    /// `range|map|filter|len` gets its traversal deleted.
    #[expect(
        clippy::indexing_slicing,
        reason = "the checker gives every call its declared arity: one argument to `length`, three to `filter`"
    )]
    fn len_of_filter(&mut self, ty: Ty, callee: &mut FuncIdx, args: &mut Vec<Expr>) -> bool {
        let ExprKind::CallFunc { func, args: inner } = &mut args[0].kind else {
            return false;
        };
        let from = *func;
        if self.combinator(from) != Some(Combinator::Filter) || !readonly(&inner[1]) {
            return false;
        }
        let keep = take(&mut inner[Combinator::Filter.step_at()]);
        let source = take(&mut inner[0]);
        let tys = vec![source.ty, keep.ty];
        *callee = self.mint(Combinator::Count, from, tys, ty);
        *args = vec![source, keep];
        true
    }

    /// `consumer(producer(xs, ..), ..)` as one `consumer` call over `xs`,
    /// whose step does both steps' work. Everything is read before anything
    /// is taken apart, so a call that doesn't fuse is left as it was.
    ///
    /// The checker has already matched every step's parameters to its
    /// combinator's, so `map`'s and `filter`'s steps take one and `fold`'s two.
    fn fuse(
        &mut self,
        ty: Ty,
        consumer: Combinator,
        callee: &mut FuncIdx,
        args: &mut [Expr],
    ) -> Option<()> {
        let (source, rest) = args.split_first_mut()?;
        let ExprKind::CallFunc { func, args: inner } = &mut source.kind else {
            return None;
        };
        let producer = self.combinator(*func)?;
        // The two shapes that are not fusions at all.
        if matches!(
            (consumer, producer),
            (Combinator::Filter, Combinator::Map) | (Combinator::Map, Combinator::Filter)
        ) {
            return None;
        }
        // `rest` starts after the source, which every combinator takes first.
        let step = rest.get_mut(consumer.step_at().saturating_sub(1))?;
        let span = step.span;
        let ExprKind::Lambda {
            params,
            body,
            captures,
        } = &mut step.kind
        else {
            return None;
        };
        let (list, inner_rest) = inner.split_first_mut()?;
        // `map` and `filter` both take an `Allocator` for the block they build,
        // and the fused call builds no such block.
        if !inner_rest.first().is_some_and(readonly) {
            return None;
        }
        let inner_step = inner_rest.get_mut(producer.step_at().saturating_sub(1))?;
        let ExprKind::Lambda {
            params: inner_params,
            body: inner_body,
            captures: inner_captures,
        } = &mut inner_step.kind
        else {
            return None;
        };
        // The two lambdas were written apart, so their parameters are different
        // locals of the same table: the fused step binds each slot once.
        let elem = *inner_params.first()?;
        // Every step takes its element last; only `fold`'s has an accumulator.
        let (&bound, before) = params.split_last()?;
        let elem_ty = match list.ty.kind() {
            TyKind::Array(elem_ty) => Some(*elem_ty),
            _ => None,
        }?;
        if !movable(inner_body) {
            return None;
        }

        let plan = Plan {
            consumer,
            producer,
            elem,
            elem_ty,
        };
        let acc = before.first().copied();
        let acc_ty = body.ty;
        // A `filter` answers the list it was given, and so does a `map` from a
        // type to itself; either way the instance in hand is already over the
        // right element type and no new one is needed.
        let same_elements = list.ty == source.ty;
        let body = compose(&plan, take(inner_body), acc, bound, take(body), span);
        let mut all_captures = std::mem::take(inner_captures);
        for c in std::mem::take(captures) {
            if !all_captures.contains(&c) {
                all_captures.push(c);
            }
        }
        let (params, param_tys) = match acc {
            Some(acc) => (vec![acc, plan.elem], vec![acc_ty, plan.elem_ty]),
            None => (vec![plan.elem], vec![plan.elem_ty]),
        };
        let ret_ty = body.ty;
        *step = Expr::new(
            ExprKind::Lambda {
                params,
                body: Box::new(body),
                captures: all_captures,
            },
            Ty::func(param_tys, ret_ty),
            span,
        );
        *source = take(list);
        if !same_elements {
            let arg_tys: Vec<Ty> = args.iter().map(|a| a.ty).collect();
            *callee = self.mint(consumer, *callee, arg_tys, ty);
        }
        Some(())
    }

    /// A fresh instance of a combinator over the source's element type.
    ///
    /// It is a [`FuncKind::Intrinsic`] — a name and a signature and no body — so
    /// minting one is writing down the shape the call now has. `lower` turns it
    /// into `Body::Runtime` and the backend open-codes the loop from the types
    /// of the arguments at the call site, which is why the instance never needs
    /// to have existed before.
    fn mint(&mut self, combinator: Combinator, from: FuncIdx, params: Vec<Ty>, ret: Ty) -> FuncIdx {
        let f = self.funcs.get(from);
        let (symbol, debug_name, span) = (f.symbol.clone(), f.debug_name.clone(), f.span);
        // The id the push below mints.
        let n = self.funcs.len();
        let locals: typed::Locals = params
            .into_iter()
            .enumerate()
            .map(|(i, ty)| typed::Local {
                name: Name::new(&format!("a{i}")),
                ty,
                span,
            })
            .collect();
        self.funcs.push(Func {
            symbol: format!("{symbol}$fused{n}"),
            debug_name,
            params: locals.ids().collect(),
            locals,
            kind: FuncKind::Intrinsic(combinator.key().to_string()),
            ret,
            desc: None,
            env: false,
            span,
        })
    }
}

/// The two combinators [`Fuse::fuse`] read, and the fused step's element.
struct Plan {
    consumer: Combinator,
    producer: Combinator,
    /// The producer step's parameter, which becomes the fused step's element.
    elem: LocalId,
    elem_ty: Ty,
}

/// The fused step's body.
fn compose(
    plan: &Plan,
    producer_body: Expr,
    acc: Option<LocalId>,
    bound: LocalId,
    consumer_body: Expr,
    span: Span,
) -> Expr {
    let ty = consumer_body.ty;
    match plan.producer {
        // `g(a, f(x))`: the consumer step's element parameter is bound to the
        // producer step's body, which reads the fused step's own parameter.
        Combinator::Map => {
            let value_ty = producer_body.ty;
            Expr::new(
                ExprKind::Block {
                    stmts: vec![binding(bound, value_ty, producer_body, span)],
                    tail: Some(Box::new(consumer_body)),
                },
                ty,
                span,
            )
        }
        // The producer's predicate becomes the fused step's guard, and what the
        // step answers when the guard fails is the whole of the difference
        // between the four consumers that admit one.
        Combinator::Filter => {
            let elem = Expr::new(ExprKind::Local(plan.elem), plan.elem_ty, span);
            let kept = Expr::new(
                ExprKind::Block {
                    stmts: vec![binding(bound, plan.elem_ty, elem, span)],
                    tail: Some(Box::new(consumer_body)),
                },
                ty,
                span,
            );
            let dropped = match (plan.consumer, acc) {
                // An element the filter drops leaves the accumulator alone.
                (Combinator::Fold, Some(acc)) => Expr::new(ExprKind::Local(acc), ty, span),
                // `all` over the kept elements is `!keep(x) || pred(x)`.
                (Combinator::All, _) => Expr::new(ExprKind::Bool(true), ty, span),
                _ => Expr::new(ExprKind::Bool(false), ty, span),
            };
            Expr::new(
                ExprKind::If {
                    cond: Box::new(producer_body),
                    then: Box::new(kept),
                    else_: Box::new(dropped),
                },
                ty,
                span,
            )
        }
        _ => consumer_body,
    }
}

fn binding(local: LocalId, ty: Ty, value: Expr, span: Span) -> Stmt {
    Stmt::Let {
        pattern: Pattern {
            kind: PatKind::Bind { local, sub: None },
            ty,
            span,
        },
        value,
        span,
    }
}

fn unit() -> Expr {
    Expr::new(ExprKind::Unit, Ty::UNIT, Span::default())
}

/// Takes an expression out of its place, leaving a placeholder the rewrite
/// overwrites.
fn take(e: &mut Expr) -> Expr {
    std::mem::replace(e, unit())
}

/// Whether a step's body may be spliced into another lambda.
///
/// `?` exits the lambda it is written in, and splicing changes which lambda that
/// is. `Continue` and `Loop` are whole function bodies and cannot appear inside
/// a step at all, so finding one means the shape is not what this pass thinks.
fn movable(e: &Expr) -> bool {
    let mut ok = true;
    typed::walk(e, &mut |x| {
        if matches!(
            x.kind,
            ExprKind::Try { .. } | ExprKind::Continue { .. } | ExprKind::Loop { .. }
        ) {
            ok = false;
        }
    });
    ok
}

/// Whether dropping this argument's evaluation is invisible.
fn readonly(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Local(_) | ExprKind::Unit | ExprKind::FuncRef(_) => true,
        ExprKind::CtxGet { base, .. }
        | ExprKind::Field { base, .. }
        | ExprKind::TupleIndex { base, .. } => readonly(base),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::run;
    use crate::compiler::middle::func_id;
    use crate::compiler::middle::local_id;
    use crate::compiler::middle::monomorphize::{Func, FuncKind, Program, ProgramRoots};
    use crate::compiler::semantics::name::Name;
    use crate::compiler::semantics::typed::{Expr, ExprKind, Local, Stmt};
    use crate::compiler::semantics::types::Ty;
    use crate::diagnostics::Span;
    use crate::hash::Map as HashMap;

    fn list() -> Ty {
        Ty::array(Ty::UNIT)
    }

    fn e(kind: ExprKind, ty: Ty) -> Expr {
        Expr::new(kind, ty, Span::default())
    }

    fn lambda(params: Vec<u32>, body: Expr) -> Expr {
        let ty = Ty::func(params.iter().map(|_| Ty::UNIT), body.ty);
        e(
            ExprKind::Lambda {
                params: params.into_iter().map(local_id).collect(),
                body: Box::new(body),
                captures: Vec::new(),
            },
            ty,
        )
    }

    fn call(f: u32, args: Vec<Expr>, ty: Ty) -> Expr {
        e(
            ExprKind::CallFunc {
                func: func_id(f),
                args,
            },
            ty,
        )
    }

    fn intrinsic(key: &str) -> Func {
        Func {
            symbol: key.to_string(),
            debug_name: key.to_string(),
            params: Vec::new(),
            locals: Default::default(),
            kind: FuncKind::Intrinsic(key.to_string()),
            ret: Ty::UNIT,
            desc: None,
            env: false,
            span: Span::default(),
        }
    }

    /// `funcs[0]` is the host; the keys follow it in the order given.
    fn program(keys: &[&str], body: Expr) -> Program {
        let mut funcs = vec![Func {
            symbol: "host".to_string(),
            debug_name: "host".to_string(),
            params: Vec::new(),
            locals: (0..8)
                .map(|i| Local {
                    name: Name::new(&format!("l{i}")),
                    ty: Ty::UNIT,
                    span: Span::default(),
                })
                .collect(),
            kind: FuncKind::Body(body),
            ret: Ty::UNIT,
            desc: None,
            env: false,
            span: Span::default(),
        }];
        funcs.extend(keys.iter().map(|k| intrinsic(k)));
        Program {
            funcs: funcs.into_iter().collect(),
            roots: ProgramRoots::Main(func_id(0)),
            descriptors: Vec::new(),
            desc_modules: Vec::new(),
            desc_index: HashMap::default(),
            cell_equal: HashMap::default(),
            ctx_layouts: HashMap::default(),
            shapes: Default::default(),
            stylesheet: String::new(),
            inline_styles: false,
            inline_animations: false,
            icons: false,
            tooltips: false,
            themes: false,
            chunks: Vec::new(),
            hosted: Default::default(),
            instances: Default::default(),
        }
    }

    /// The map's body is spliced into the fold's step, bound to the parameter
    /// the fold used to read the mapped element out of.
    #[test]
    fn a_fold_over_a_map_becomes_one_fold() {
        // fold(map(xs, ctx, |x| x), |a, m| m, z)
        let map = call(
            1,
            vec![
                e(ExprKind::Local(local_id(0)), list()),
                e(ExprKind::Local(local_id(1)), Ty::UNIT),
                lambda(vec![2], e(ExprKind::Local(local_id(2)), Ty::UNIT)),
            ],
            list(),
        );
        let fold = call(
            2,
            vec![
                map,
                lambda(vec![3, 4], e(ExprKind::Local(local_id(4)), Ty::UNIT)),
                e(ExprKind::Unit, Ty::UNIT),
            ],
            Ty::UNIT,
        );
        let mut p = program(&["list.map", "list.fold"], fold);
        run(&mut p);

        let body = p.funcs.as_slice()[0].body().unwrap();
        let ExprKind::CallFunc { func, args } = &body.kind else {
            panic!("still a call")
        };
        assert_eq!(*func, func_id(2), "the fold instance is reused");
        assert!(
            matches!(args[0].kind, ExprKind::Local(l) if l == local_id(0)),
            "over the source list"
        );
        let ExprKind::Lambda { params, body, .. } = &args[1].kind else {
            panic!("a lambda")
        };
        // The accumulator, then the *map's* parameter as the element.
        assert_eq!(params, &[local_id(3), local_id(2)]);
        let ExprKind::Block { stmts, .. } = &body.kind else {
            panic!("a block")
        };
        assert_eq!(stmts.len(), 1);
        let Stmt::Let { value, .. } = &stmts[0] else {
            panic!("a binding")
        };
        assert!(
            matches!(value.kind, ExprKind::Local(l) if l == local_id(2)),
            "the map's own body"
        );
    }

    /// The filter's predicate becomes the step's guard and the accumulator is
    /// what an element it drops leaves behind.
    #[test]
    fn a_fold_over_a_filter_becomes_a_guarded_fold() {
        let filter = call(
            1,
            vec![
                e(ExprKind::Local(local_id(0)), list()),
                e(ExprKind::Local(local_id(1)), Ty::UNIT),
                lambda(vec![2], e(ExprKind::Bool(true), Ty::UNIT)),
            ],
            list(),
        );
        let fold = call(
            2,
            vec![
                filter,
                lambda(vec![3, 4], e(ExprKind::Local(local_id(3)), Ty::UNIT)),
                e(ExprKind::Unit, Ty::UNIT),
            ],
            Ty::UNIT,
        );
        let mut p = program(&["list.filter", "list.fold"], fold);
        run(&mut p);

        let ExprKind::CallFunc { args, .. } = &p.funcs.as_slice()[0].body().unwrap().kind else {
            panic!("still a call")
        };
        let ExprKind::Lambda { body, .. } = &args[1].kind else {
            panic!("a lambda")
        };
        let ExprKind::If { else_, .. } = &body.kind else {
            panic!("a guard")
        };
        assert!(
            matches!(else_.kind, ExprKind::Local(l) if l == local_id(3)),
            "a dropped element leaves the accumulator alone"
        );
    }

    /// The `*Ctx` variants thread a context so that a step may have an effect,
    /// which is the one thing fusion may not reorder.
    #[test]
    fn a_context_threading_variant_does_not_fuse() {
        let map = call(
            1,
            vec![
                e(ExprKind::Local(local_id(0)), list()),
                e(ExprKind::Local(local_id(1)), Ty::UNIT),
                lambda(vec![2], e(ExprKind::Local(local_id(2)), Ty::UNIT)),
            ],
            list(),
        );
        let fold = call(
            2,
            vec![
                map,
                e(ExprKind::Local(local_id(1)), Ty::UNIT),
                lambda(vec![3, 4], e(ExprKind::Local(local_id(4)), Ty::UNIT)),
                e(ExprKind::Unit, Ty::UNIT),
            ],
            Ty::UNIT,
        );
        let mut p = program(&["list.mapCtx", "list.foldCtx"], fold);
        run(&mut p);
        let ExprKind::CallFunc { args, .. } = &p.funcs.as_slice()[0].body().unwrap().kind else {
            panic!("still a call")
        };
        assert!(
            matches!(args[0].kind, ExprKind::CallFunc { .. }),
            "the map is still there"
        );
    }

    /// A step passed by name would have to be *called* from the fused step,
    /// which captures it — the traffic goes and the dispatch gets worse.
    #[test]
    fn a_step_that_is_not_a_lambda_does_not_fuse() {
        let map = call(
            1,
            vec![
                e(ExprKind::Local(local_id(0)), list()),
                e(ExprKind::Local(local_id(1)), Ty::UNIT),
                e(ExprKind::Local(local_id(5)), Ty::UNIT),
            ],
            list(),
        );
        let fold = call(
            2,
            vec![
                map,
                lambda(vec![3, 4], e(ExprKind::Local(local_id(4)), Ty::UNIT)),
                e(ExprKind::Unit, Ty::UNIT),
            ],
            Ty::UNIT,
        );
        let mut p = program(&["list.map", "list.fold"], fold);
        run(&mut p);
        let ExprKind::CallFunc { args, .. } = &p.funcs.as_slice()[0].body().unwrap().kind else {
            panic!("still a call")
        };
        assert!(matches!(args[0].kind, ExprKind::CallFunc { .. }));
    }

    /// `filter(…).len()` is `count(…)` — and the instance for it is minted,
    /// because the program need never have called `count` at all.
    #[test]
    fn the_length_of_a_filter_is_a_count() {
        let filter = call(
            1,
            vec![
                e(ExprKind::Local(local_id(0)), list()),
                e(ExprKind::Local(local_id(1)), Ty::UNIT),
                lambda(vec![2], e(ExprKind::Bool(true), Ty::UNIT)),
            ],
            list(),
        );
        let len = call(2, vec![filter], Ty::UNIT);
        let mut p = program(&["list.filter", "list.length"], len);
        let before = p.funcs.len();
        run(&mut p);

        let ExprKind::CallFunc { func, args } = &p.funcs.as_slice()[0].body().unwrap().kind else {
            panic!("still a call")
        };
        assert_eq!(*func, func_id(before as u32), "a minted instance");
        assert_eq!(
            p.funcs.as_slice()[before].intrinsic_key(),
            Some("list.count")
        );
        assert_eq!(args.len(), 2);
        assert!(matches!(args[0].kind, ExprKind::Local(l) if l == local_id(0)));
        assert!(matches!(args[1].kind, ExprKind::Lambda { .. }));
    }

    /// Three stages collapse to one, which is what the fixpoint at a node is
    /// for: the outer pair fuses and what is left in its place is a pair again.
    #[test]
    fn a_three_stage_chain_collapses_to_one_traversal() {
        let map = call(
            1,
            vec![
                e(ExprKind::Local(local_id(0)), list()),
                e(ExprKind::Local(local_id(1)), Ty::UNIT),
                lambda(vec![2], e(ExprKind::Local(local_id(2)), Ty::UNIT)),
            ],
            list(),
        );
        let filter = call(
            2,
            vec![
                map,
                e(ExprKind::Local(local_id(1)), Ty::UNIT),
                lambda(vec![3], e(ExprKind::Bool(true), Ty::UNIT)),
            ],
            list(),
        );
        let fold = call(
            3,
            vec![
                filter,
                lambda(vec![4, 5], e(ExprKind::Local(local_id(5)), Ty::UNIT)),
                e(ExprKind::Unit, Ty::UNIT),
            ],
            Ty::UNIT,
        );
        let mut p = program(&["list.map", "list.filter", "list.fold"], fold);
        run(&mut p);

        let mut calls = 0;
        crate::compiler::semantics::typed::walk(p.funcs.as_slice()[0].body().unwrap(), &mut |x| {
            if matches!(x.kind, ExprKind::CallFunc { .. }) {
                calls += 1;
            }
        });
        assert_eq!(calls, 1, "one traversal, no intermediate list");
    }
}
