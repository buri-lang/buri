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
//!   not build the producer's list, so the producer's `Alloc` argument is never
//!   evaluated, and an argument that was a call would have been.
//! * `filter(map(…))` and `map(filter(…))`, which are not fusions: the first
//!   would have to answer source elements where it answers mapped ones, and the
//!   second changes length.

use crate::compiler::middle::monomorphize::{Func, FuncKind, Program};
use crate::compiler::semantics::typed::{
    self, Callee, Expr, ExprKind, PatKind, Pattern, Stmt,
};
use crate::compiler::semantics::types::{FuncIdx, LocalId, Ty};
use crate::diagnostics::Span;

/// Fuses every combinator chain in the program.
pub fn run(program: &mut Program) {
    let keys: Vec<Option<String>> =
        program.funcs.iter().map(|f| f.intrinsic_key().map(str::to_string)).collect();
    let names: Vec<(String, String, Span)> = program
        .funcs
        .iter()
        .map(|f| (f.symbol.clone(), f.debug_name.clone(), f.span))
        .collect();
    let base = program.funcs.len();
    let mut minted: Vec<Func> = Vec::new();
    for i in 0..base {
        let Some(func) = program.funcs.get_mut(i) else { continue };
        let Some(mut body) = func.take_body() else { continue };
        let mut fx = Fuse { keys: &keys, names: &names, base, minted: &mut minted };
        fx.expr(&mut body);
        if let Some(func) = program.funcs.get_mut(i) {
            func.set_body(body);
        }
    }
    program.funcs.extend(minted);
}

/// Which `core/list` combinator a call is.
///
/// The argument positions below are `core/list`'s own declaration order, and
/// they are the same six keys `backend::cranelift::emit::list_call` reads.
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

    fn arity(self) -> usize {
        match self {
            Combinator::Map | Combinator::Filter | Combinator::Fold => 3,
            Combinator::Count | Combinator::Any | Combinator::All => 2,
        }
    }

    /// How many parameters the step takes.
    fn step_params(self) -> usize {
        match self {
            Combinator::Fold => 2,
            _ => 1,
        }
    }
}

struct Fuse<'a> {
    keys: &'a [Option<String>],
    names: &'a [(String, String, Span)],
    base: usize,
    minted: &'a mut Vec<Func>,
}

impl Fuse<'_> {
    /// Depth first, then to fixpoint at this node: a three-stage chain fuses its
    /// outer pair first, and what is left in the same place is a two-stage one.
    fn expr(&mut self, e: &mut Expr) {
        typed::children_mut(e, &mut |child| self.expr(child));
        while self.once(e) {}
    }

    fn key<'e>(&self, e: &'e Expr) -> Option<(&str, FuncIdx, &'e Vec<Expr>)> {
        let ExprKind::CallFn { func, args } = &e.kind else { return None };
        let idx = func.func()?;
        // A minted instance is not in `keys`, and a chain fuses one stage at a
        // time — so the instance this pass wrote a moment ago is exactly the
        // one the next stage has to recognise.
        let key = match self.keys.get(idx.index()) {
            Some(k) => k.as_deref()?,
            None => self.minted.get(idx.index().checked_sub(self.base)?)?.intrinsic_key()?,
        };
        Some((key, idx, args))
    }

    fn combinator(&self, e: &Expr) -> Option<(Combinator, FuncIdx)> {
        let (key, idx, args) = self.key(e)?;
        let combinator = Combinator::of(key)?;
        if args.len() != combinator.arity() {
            return None;
        }
        Some((combinator, idx))
    }

    /// One fusion at this node, or nothing.
    fn once(&mut self, e: &mut Expr) -> bool {
        if self.len_of_filter(e) {
            return true;
        }
        match self.plan(e) {
            Some(plan) => self.rewrite(e, plan),
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
    fn len_of_filter(&mut self, e: &mut Expr) -> bool {
        let Some(("list.len", _, args)) = self.key(e) else { return false };
        if args.len() != 1 {
            return false;
        }
        let Some(inner) = args.first() else { return false };
        let Some((Combinator::Filter, from)) = self.combinator(inner) else { return false };
        let Some(producer_args) = call_args(inner) else { return false };
        let Some(ctx) = producer_args.get(1) else { return false };
        if !readonly(ctx) {
            return false;
        }
        let node_ty = e.ty.clone();
        let Some(mut inner) = take_arg(e, 0) else { return false };
        let Some(keep) = take_arg(&mut inner, Combinator::Filter.step_at()) else { return false };
        let Some(source) = take_arg(&mut inner, 0) else { return false };
        let tys = vec![source.ty.clone(), keep.ty.clone()];
        let func = self.mint(Combinator::Count, from, tys, node_ty);
        if let ExprKind::CallFn { func: callee, args } = &mut e.kind {
            *callee = Callee::Func(func);
            *args = vec![source, keep];
        }
        true
    }

    /// Everything the rewrite needs to know, decided while the node is only
    /// read, so that nothing is taken apart before it is known to fuse.
    fn plan(&self, e: &Expr) -> Option<Plan> {
        let (consumer, consumer_idx) = self.combinator(e)?;
        let args = call_args(e)?;
        let source = args.first()?;
        let (producer, _) = self.combinator(source)?;
        // The two shapes that are not fusions at all.
        if matches!(
            (consumer, producer),
            (Combinator::Filter, Combinator::Map) | (Combinator::Map, Combinator::Filter)
        ) {
            return None;
        }
        let producer_args = call_args(source)?;
        let ExprKind::Lambda { params: consumer_params, .. } =
            &args.get(consumer.step_at())?.kind
        else {
            return None;
        };
        let ExprKind::Lambda { params: producer_params, body: producer_body, .. } =
            &producer_args.get(producer.step_at())?.kind
        else {
            return None;
        };
        if producer_params.len() != 1 || consumer_params.len() != consumer.step_params() {
            return None;
        }
        let elem = *producer_params.first()?;
        // The two lambdas were written apart, so their parameters are different
        // locals of the same table; equal ones would mean the fused step bound
        // one slot twice.
        if consumer_params.contains(&elem) {
            return None;
        }
        if !movable(producer_body) {
            return None;
        }
        // `map` and `filter` both take an `Alloc` for the block they build, and
        // the fused call builds no such block.
        if !readonly(producer_args.get(1)?) {
            return None;
        }
        let producer_source = producer_args.first()?;
        let Ty::Array(elem_ty) = &producer_source.ty else { return None };
        Some(Plan {
            consumer,
            consumer_idx,
            producer,
            elem,
            elem_ty: (**elem_ty).clone(),
            // A `filter` answers the list it was given, and so does a `map` from
            // a type to itself; either way the instance in hand is already over
            // the right element type and no new one is needed.
            same_elements: producer_source.ty == source.ty,
        })
    }

    fn rewrite(&mut self, e: &mut Expr, plan: Plan) -> bool {
        let node_ty = e.ty.clone();
        let Some(mut producer) = take_arg(e, 0) else { return false };
        let Some(producer_step) = take_arg(&mut producer, plan.producer.step_at()) else {
            return false;
        };
        let Some(source) = take_arg(&mut producer, 0) else { return false };
        let Some(step) = take_arg(e, plan.consumer.step_at()) else { return false };
        let span = step.span;
        let ExprKind::Lambda { body: producer_body, captures: producer_captures, .. } =
            producer_step.kind
        else {
            return false;
        };
        let ExprKind::Lambda {
            params: consumer_params,
            body: consumer_body,
            captures: consumer_captures,
        } = step.kind
        else {
            return false;
        };

        let acc_ty = consumer_body.ty.clone();
        let Some(body) =
            compose(&plan, *producer_body, &consumer_params, *consumer_body, span)
        else {
            return false;
        };
        let mut captures = producer_captures;
        for c in consumer_captures {
            if !captures.contains(&c) {
                captures.push(c);
            }
        }
        let (params, param_tys) = match (plan.consumer, consumer_params.first()) {
            (Combinator::Fold, Some(acc)) => {
                (vec![*acc, plan.elem], vec![acc_ty, plan.elem_ty.clone()])
            }
            (Combinator::Fold, None) => return false,
            _ => (vec![plan.elem], vec![plan.elem_ty.clone()]),
        };
        let ret_ty = body.ty.clone();
        let fused = Expr::new(
            ExprKind::Lambda { params, body: Box::new(body), captures },
            Ty::Fn(param_tys, Box::new(ret_ty)),
            span,
        );

        // Both slots already hold the placeholder `take_arg` left behind.
        let Some(args) = call_args_mut(e) else { return false };
        if let Some(a) = args.first_mut() {
            *a = source;
        }
        if let Some(a) = args.get_mut(plan.consumer.step_at()) {
            *a = fused;
        }
        if !plan.same_elements {
            let arg_tys: Vec<Ty> = args.iter().map(|a| a.ty.clone()).collect();
            let func = self.mint(plan.consumer, plan.consumer_idx, arg_tys, node_ty);
            if let ExprKind::CallFn { func: callee, .. } = &mut e.kind {
                *callee = Callee::Func(func);
            }
        }
        true
    }

    /// A fresh instance of a combinator over the source's element type.
    ///
    /// It is a [`FuncKind::Intrinsic`] — a name and a signature and no body — so
    /// minting one is writing down the shape the call now has. `lower` turns it
    /// into `Body::Runtime` and the backend open-codes the loop from the types
    /// of the arguments at the call site, which is why the instance never needs
    /// to have existed before.
    fn mint(&mut self, combinator: Combinator, from: FuncIdx, params: Vec<Ty>, ret: Ty) -> FuncIdx {
        let idx = FuncIdx(u32::try_from(self.base.saturating_add(self.minted.len())).unwrap_or(u32::MAX));
        let (symbol, debug_name, span) = match self.names.get(from.index()) {
            Some(n) => n.clone(),
            None => match self.minted.get(from.index().saturating_sub(self.base)) {
                Some(f) => (f.symbol.clone(), f.debug_name.clone(), f.span),
                None => (combinator.key().into(), combinator.key().into(), Span::default()),
            },
        };
        self.minted.push(Func {
            symbol: format!("{symbol}$fused{}", idx.index()),
            debug_name,
            params: (0..params.len())
                .map(|i| LocalId(u32::try_from(i).unwrap_or(u32::MAX)))
                .collect(),
            locals: params
                .into_iter()
                .enumerate()
                .map(|(i, ty)| typed::Local { name: format!("a{i}"), ty, span })
                .collect(),
            kind: FuncKind::Intrinsic(combinator.key().to_string()),
            ret,
            desc: None,
            span,
        });
        idx
    }
}

/// What [`Fuse::plan`] decided, so that [`Fuse::rewrite`] takes the node apart
/// only once it is certain.
struct Plan {
    consumer: Combinator,
    consumer_idx: FuncIdx,
    producer: Combinator,
    /// The producer step's parameter, which becomes the fused step's element.
    elem: LocalId,
    elem_ty: Ty,
    same_elements: bool,
}

/// The fused step's body.
fn compose(
    plan: &Plan,
    producer_body: Expr,
    consumer_params: &[LocalId],
    consumer_body: Expr,
    span: Span,
) -> Option<Expr> {
    let bound = *consumer_params.last()?;
    let ty = consumer_body.ty.clone();
    match plan.producer {
        // `g(a, f(x))`: the consumer step's element parameter is bound to the
        // producer step's body, which reads the fused step's own parameter.
        Combinator::Map => {
            let value_ty = producer_body.ty.clone();
            Some(Expr::new(
                ExprKind::Block {
                    stmts: vec![binding(bound, value_ty, producer_body, span)],
                    tail: Some(Box::new(consumer_body)),
                },
                ty,
                span,
            ))
        }
        // The producer's predicate becomes the fused step's guard, and what the
        // step answers when the guard fails is the whole of the difference
        // between the four consumers that admit one.
        Combinator::Filter => {
            let elem = Expr::new(ExprKind::Local(plan.elem), plan.elem_ty.clone(), span);
            let kept = Expr::new(
                ExprKind::Block {
                    stmts: vec![binding(bound, plan.elem_ty.clone(), elem, span)],
                    tail: Some(Box::new(consumer_body)),
                },
                ty.clone(),
                span,
            );
            let dropped = match (plan.consumer, consumer_params.first()) {
                // An element the filter drops leaves the accumulator alone.
                (Combinator::Fold, Some(acc)) => Expr::new(ExprKind::Local(*acc), ty.clone(), span),
                (Combinator::Fold, None) => return None,
                // `all` over the kept elements is `!keep(x) || pred(x)`.
                (Combinator::All, _) => Expr::new(ExprKind::Bool(true), ty.clone(), span),
                _ => Expr::new(ExprKind::Bool(false), ty.clone(), span),
            };
            Some(Expr::new(
                ExprKind::If {
                    cond: Box::new(producer_body),
                    then: Box::new(kept),
                    else_: Box::new(dropped),
                },
                ty,
                span,
            ))
        }
        _ => Some(consumer_body),
    }
}

fn binding(local: LocalId, ty: Ty, value: Expr, span: Span) -> Stmt {
    Stmt::Let {
        pattern: Pattern { kind: PatKind::Bind { local, sub: None }, ty, span },
        value,
        span,
    }
}

fn unit() -> Expr {
    Expr::new(ExprKind::Unit, Ty::Unit, Span::default())
}

fn call_args(e: &Expr) -> Option<&Vec<Expr>> {
    match &e.kind {
        ExprKind::CallFn { args, .. } => Some(args),
        _ => None,
    }
}

fn call_args_mut(e: &mut Expr) -> Option<&mut Vec<Expr>> {
    match &mut e.kind {
        ExprKind::CallFn { args, .. } => Some(args),
        _ => None,
    }
}

/// Takes one argument out of a call, leaving a placeholder the rewrite either
/// overwrites or discards with the node.
fn take_arg(e: &mut Expr, at: usize) -> Option<Expr> {
    let slot = call_args_mut(e)?.get_mut(at)?;
    Some(std::mem::replace(slot, unit()))
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
        ExprKind::Local(_) | ExprKind::Unit | ExprKind::FnRef(_) => true,
        ExprKind::CtxGet { base, .. }
        | ExprKind::Field { base, .. }
        | ExprKind::TupleIndex { base, .. } => readonly(base),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::run;
    use crate::compiler::middle::monomorphize::{Func, FuncKind, Program, ProgramRoots};
    use crate::compiler::semantics::typed::{Callee, Expr, ExprKind, Local, Stmt};
    use crate::compiler::semantics::types::{FuncIdx, LocalId, Ty};
    use crate::diagnostics::Span;
    use crate::hash::Map as HashMap;

    fn list() -> Ty {
        Ty::Array(Box::new(Ty::Unit))
    }

    fn e(kind: ExprKind, ty: Ty) -> Expr {
        Expr::new(kind, ty, Span::default())
    }

    fn lambda(params: Vec<u32>, body: Expr) -> Expr {
        let ty = Ty::Fn(params.iter().map(|_| Ty::Unit).collect(), Box::new(body.ty.clone()));
        e(
            ExprKind::Lambda {
                params: params.into_iter().map(LocalId).collect(),
                body: Box::new(body),
                captures: Vec::new(),
            },
            ty,
        )
    }

    fn call(f: u32, args: Vec<Expr>, ty: Ty) -> Expr {
        e(ExprKind::CallFn { func: Callee::Func(FuncIdx(f)), args }, ty)
    }

    fn intrinsic(key: &str) -> Func {
        Func {
            symbol: key.to_string(),
            debug_name: key.to_string(),
            params: Vec::new(),
            locals: Vec::new(),
            kind: FuncKind::Intrinsic(key.to_string()),
            ret: Ty::Unit,
            desc: None,
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
                    name: format!("l{i}"),
                    ty: Ty::Unit,
                    span: Span::default(),
                })
                .collect(),
            kind: FuncKind::Body(body),
            ret: Ty::Unit,
            desc: None,
            span: Span::default(),
        }];
        funcs.extend(keys.iter().map(|k| intrinsic(k)));
        Program {
            funcs,
            roots: ProgramRoots::Main(FuncIdx(0)),
            descriptors: Vec::new(),
            desc_modules: Vec::new(),
            desc_index: HashMap::default(),
            ctx_layouts: HashMap::default(),
            shapes: Default::default(),
            stylesheet: String::new(),
            inline_styles: false,
            themes: false,
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
                e(ExprKind::Local(LocalId(0)), list()),
                e(ExprKind::Local(LocalId(1)), Ty::Unit),
                lambda(vec![2], e(ExprKind::Local(LocalId(2)), Ty::Unit)),
            ],
            list(),
        );
        let fold = call(
            2,
            vec![
                map,
                lambda(vec![3, 4], e(ExprKind::Local(LocalId(4)), Ty::Unit)),
                e(ExprKind::Unit, Ty::Unit),
            ],
            Ty::Unit,
        );
        let mut p = program(&["list.map", "list.fold"], fold);
        run(&mut p);

        let body = p.funcs[0].body().unwrap();
        let ExprKind::CallFn { func, args } = &body.kind else { panic!("still a call") };
        assert_eq!(func.func(), Some(FuncIdx(2)), "the fold instance is reused");
        assert!(matches!(args[0].kind, ExprKind::Local(LocalId(0))), "over the source list");
        let ExprKind::Lambda { params, body, .. } = &args[1].kind else { panic!("a lambda") };
        // The accumulator, then the *map's* parameter as the element.
        assert_eq!(params, &[LocalId(3), LocalId(2)]);
        let ExprKind::Block { stmts, .. } = &body.kind else { panic!("a block") };
        assert_eq!(stmts.len(), 1);
        let Stmt::Let { value, .. } = &stmts[0] else { panic!("a binding") };
        assert!(matches!(value.kind, ExprKind::Local(LocalId(2))), "the map's own body");
    }

    /// The filter's predicate becomes the step's guard and the accumulator is
    /// what an element it drops leaves behind.
    #[test]
    fn a_fold_over_a_filter_becomes_a_guarded_fold() {
        let filter = call(
            1,
            vec![
                e(ExprKind::Local(LocalId(0)), list()),
                e(ExprKind::Local(LocalId(1)), Ty::Unit),
                lambda(vec![2], e(ExprKind::Bool(true), Ty::Unit)),
            ],
            list(),
        );
        let fold = call(
            2,
            vec![
                filter,
                lambda(vec![3, 4], e(ExprKind::Local(LocalId(3)), Ty::Unit)),
                e(ExprKind::Unit, Ty::Unit),
            ],
            Ty::Unit,
        );
        let mut p = program(&["list.filter", "list.fold"], fold);
        run(&mut p);

        let ExprKind::CallFn { args, .. } = &p.funcs[0].body().unwrap().kind else {
            panic!("still a call")
        };
        let ExprKind::Lambda { body, .. } = &args[1].kind else { panic!("a lambda") };
        let ExprKind::If { else_, .. } = &body.kind else { panic!("a guard") };
        assert!(
            matches!(else_.kind, ExprKind::Local(LocalId(3))),
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
                e(ExprKind::Local(LocalId(0)), list()),
                e(ExprKind::Local(LocalId(1)), Ty::Unit),
                lambda(vec![2], e(ExprKind::Local(LocalId(2)), Ty::Unit)),
            ],
            list(),
        );
        let fold = call(
            2,
            vec![
                map,
                e(ExprKind::Local(LocalId(1)), Ty::Unit),
                lambda(vec![3, 4], e(ExprKind::Local(LocalId(4)), Ty::Unit)),
                e(ExprKind::Unit, Ty::Unit),
            ],
            Ty::Unit,
        );
        let mut p = program(&["list.mapCtx", "list.foldCtx"], fold);
        run(&mut p);
        let ExprKind::CallFn { args, .. } = &p.funcs[0].body().unwrap().kind else {
            panic!("still a call")
        };
        assert!(matches!(args[0].kind, ExprKind::CallFn { .. }), "the map is still there");
    }

    /// A step passed by name would have to be *called* from the fused step,
    /// which captures it — the traffic goes and the dispatch gets worse.
    #[test]
    fn a_step_that_is_not_a_lambda_does_not_fuse() {
        let map = call(
            1,
            vec![
                e(ExprKind::Local(LocalId(0)), list()),
                e(ExprKind::Local(LocalId(1)), Ty::Unit),
                e(ExprKind::Local(LocalId(5)), Ty::Unit),
            ],
            list(),
        );
        let fold = call(
            2,
            vec![
                map,
                lambda(vec![3, 4], e(ExprKind::Local(LocalId(4)), Ty::Unit)),
                e(ExprKind::Unit, Ty::Unit),
            ],
            Ty::Unit,
        );
        let mut p = program(&["list.map", "list.fold"], fold);
        run(&mut p);
        let ExprKind::CallFn { args, .. } = &p.funcs[0].body().unwrap().kind else {
            panic!("still a call")
        };
        assert!(matches!(args[0].kind, ExprKind::CallFn { .. }));
    }

    /// `filter(…).len()` is `count(…)` — and the instance for it is minted,
    /// because the program need never have called `count` at all.
    #[test]
    fn the_length_of_a_filter_is_a_count() {
        let filter = call(
            1,
            vec![
                e(ExprKind::Local(LocalId(0)), list()),
                e(ExprKind::Local(LocalId(1)), Ty::Unit),
                lambda(vec![2], e(ExprKind::Bool(true), Ty::Unit)),
            ],
            list(),
        );
        let len = call(2, vec![filter], Ty::Unit);
        let mut p = program(&["list.filter", "list.len"], len);
        let before = p.funcs.len();
        run(&mut p);

        let ExprKind::CallFn { func, args } = &p.funcs[0].body().unwrap().kind else {
            panic!("still a call")
        };
        assert_eq!(func.func(), Some(FuncIdx(before as u32)), "a minted instance");
        assert_eq!(p.funcs[before].intrinsic_key(), Some("list.count"));
        assert_eq!(args.len(), 2);
        assert!(matches!(args[0].kind, ExprKind::Local(LocalId(0))));
        assert!(matches!(args[1].kind, ExprKind::Lambda { .. }));
    }

    /// Three stages collapse to one, which is what the fixpoint at a node is
    /// for: the outer pair fuses and what is left in its place is a pair again.
    #[test]
    fn a_three_stage_chain_collapses_to_one_traversal() {
        let map = call(
            1,
            vec![
                e(ExprKind::Local(LocalId(0)), list()),
                e(ExprKind::Local(LocalId(1)), Ty::Unit),
                lambda(vec![2], e(ExprKind::Local(LocalId(2)), Ty::Unit)),
            ],
            list(),
        );
        let filter = call(
            2,
            vec![map, e(ExprKind::Local(LocalId(1)), Ty::Unit), lambda(vec![3], e(ExprKind::Bool(true), Ty::Unit))],
            list(),
        );
        let fold = call(
            3,
            vec![
                filter,
                lambda(vec![4, 5], e(ExprKind::Local(LocalId(5)), Ty::Unit)),
                e(ExprKind::Unit, Ty::Unit),
            ],
            Ty::Unit,
        );
        let mut p = program(&["list.map", "list.filter", "list.fold"], fold);
        run(&mut p);

        let mut calls = 0;
        crate::compiler::semantics::typed::walk(p.funcs[0].body().unwrap(), &mut |x| {
            if matches!(x.kind, ExprKind::CallFn { .. }) {
                calls += 1;
            }
        });
        assert_eq!(calls, 1, "one traversal, no intermediate list");
    }
}
