//! Tail-call elimination.
//!
//! Implementations must eliminate tail calls, including mutually recursive
//! ones, so that tail-recursive functions run in constant stack space
//! (SPEC 8.3). JavaScript cannot be relied on for this — no engine but
//! JavaScriptCore implements proper tail calls — so the compiler performs the
//! elimination itself (SPEC 8.3.1):
//!
//! | Shape | Transformation | Cost |
//! |---|---|---|
//! | A function tail-calls itself | rewrite to a loop with parameter rebinding | none |
//! | A statically known group tail-calls each other | merge into one function with a dispatch switch | one branch per bounce |
//!
//! Both are exact: the emitted loop is what a hand-written loop would have
//! been. They apply because Buri has no dynamic dispatch, so the call graph of
//! direct calls is fully known after monomorphization.
//!
//! Two things about the shape are easy to get wrong, and both were:
//!
//!  * **What counts as tail position.** It is not only a block's result, an
//!    `if` arm and a match arm: the right operand of `&&`, `||` and `??` is
//!    the whole expression's result whenever it is reached, so a call there is
//!    a tail call too. [`tail_callees`] decides this, and [`rewrite_tails`]
//!    rewrites exactly what it counted — a shape one of them counted and the
//!    other did not gave a `while (true)` nothing ever continues, which looks
//!    like elimination and is not.
//!  * **What the loop rebinds.** The loop assigns the parameters in place,
//!    which is exact for every read within the iteration that wrote them and
//!    wrong for a closure, which keeps the slot rather than the value. See
//!    [`snapshot_captures`]. Rebinding parameters in place is the parallel-move
//!    problem; `reference/` vendors the theory (Rideau, Serpette and Leroy) and
//!    the survey of how other compilers to JavaScript solve it (Thivierge and
//!    Feeley; Vouillon and Balat).
//!
//! # A rewrite, not an analysis the emitter reads back
//!
//! The first of those two hazards was a consequence of the shape of this
//! module rather than of anything about tail calls: [`analyze`] produced a
//! [`Plan`] the *emitter* then consulted, so the rule for tail position had
//! two implementations that had to agree. They shared [`tail_callees`], which
//! is what kept them agreeing; that was one implementation too many at one
//! backend and three too many at three.
//!
//! [`rewrite`] is where that ends. Afterwards the elimination is *in the
//! program*:
//!
//!  * a self-looping function's body becomes `Loop { entries: [body] }`, and
//!    each of its own tail calls a `Continue { func: None, entry: 0, args }`;
//!  * a mutually tail-recursive group becomes one new function whose body is a
//!    `Loop` with one entry per member, and each member becomes a forwarder
//!    whose whole body is a `Continue { func: Some(merged), entry: i }` — which
//!    is the "dispatch parameter" of the table above, materialised by whichever
//!    backend is emitting rather than invented here as a value at a type the
//!    middle end would have had to make up.
//!
//! A backend then emits what it is given, naively, and cannot disagree with
//! this module about tail position because it is no longer asked.
//!
//! [`Plan`] stays: it is the analysis, [`rewrite`] is its one consumer, and a
//! backend that reads it is a backend that has grown a second opinion.
//!
//! Design: `design/native/ARCHITECTURE.md` §2.2.

use crate::compiler::middle::inline::shift_expr;
use crate::compiler::middle::monomorphize::{Func, FuncKind, Program};
use crate::compiler::middle::strongly_connected;
use crate::compiler::semantics::typed::{self, Expr, ExprKind, Stmt};
use crate::compiler::semantics::types::{FuncIdx, LocalId};
use crate::hash::{Map as HashMap, Set as HashSet};

/// How one function is emitted.
///
/// One value per function replaces a `HashSet` of self-loopers and a
/// `HashMap` of group members that had to be disjoint: a function in both got
/// a loop *and* a dispatch arm, and which one won depended on the order the
/// backend happened to test them in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Strategy {
    /// Emitted as an ordinary function.
    Plain,
    /// Tail-calls itself and nothing else in a cycle: emitted as a loop.
    SelfLoop,
    /// One of a mutually tail-recursive group, merged into one function with a
    /// dispatch switch.
    Member { group: usize, index: usize },
}

pub struct Plan {
    /// Indexed by function. `strategy[f]` and `groups[g][i]` are built from
    /// one SCC result and agree by construction: `Member { group, index }`
    /// always satisfies `groups[group][index] == f`.
    strategy: Vec<Strategy>,
    /// Mutually tail-recursive groups, each merged into one function.
    pub groups: Vec<Vec<usize>>,
}

impl Plan {
    pub fn strategy(&self, f: usize) -> Strategy {
        self.strategy.get(f).copied().unwrap_or(Strategy::Plain)
    }

    pub fn is_self_loop(&self, f: usize) -> bool {
        self.strategy(f) == Strategy::SelfLoop
    }
}

/// Collects the functions called in tail position.
pub fn tail_callees(e: &Expr, out: &mut Vec<usize>) {
    match &e.kind {
        ExprKind::CallFn { func, .. } => out.extend(func.func().map(|i| i.index())),
        ExprKind::Block { tail: Some(t), .. } => tail_callees(t, out),
        ExprKind::If { then, else_, .. } => {
            tail_callees(then, out);
            tail_callees(else_, out);
        }
        ExprKind::Match { arms, .. } => {
            for a in arms {
                tail_callees(&a.body, out);
            }
        }
        // The right operand of a short-circuiting operator *is* the result
        // whenever it is reached — `a && f(x)` is `f(x)` when `a` holds,
        // `a || f(x)` is `f(x)` when it does not, and `opt ?? f(x)` is `f(x)`
        // when `opt` is empty — so it is in tail position when the whole
        // expression is. The left operand never is: its value is inspected.
        //
        // These are the shapes of `all`, `any` and a linear search, which is
        // to say the recursion an immutable language writes most often.
        ExprKind::And { rhs, .. }
        | ExprKind::Or { rhs, .. }
        | ExprKind::Coalesce { rhs, .. } => tail_callees(rhs, out),
        _ => {}
    }
}

/// Whether a call sits in tail position within this expression tree — used to
/// decide, at each node, whether to recurse with the tail flag still set.
pub fn analyze(program: &Program) -> Plan {
    let n = program.funcs.len();
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (f, es) in program.funcs.iter().zip(edges.iter_mut()) {
        if let Some(body) = f.body() {
            tail_callees(body, es);
            es.sort();
            es.dedup();
        }
    }

    let sccs = strongly_connected(&edges);
    let mut plan = Plan { strategy: vec![Strategy::Plain; n], groups: Vec::new() };
    for scc in sccs {
        if let [f] = *scc.as_slice() {
            if edges.get(f).is_some_and(|es| es.contains(&f)) {
                if let Some(slot) = plan.strategy.get_mut(f) {
                    *slot = Strategy::SelfLoop;
                }
            }
            continue;
        }
        // A statically known group of functions that tail-call each other.
        let group = plan.groups.len();
        for (i, f) in scc.iter().enumerate() {
            if let Some(slot) = plan.strategy.get_mut(*f) {
                *slot = Strategy::Member { group, index: i };
            }
        }
        plan.groups.push(scc);
    }
    plan
}

/// Rewrites the tree so that elimination is in the program rather than in an
/// emitter.
///
/// Runs once, on the whole program, before any backend sees it. Afterwards no
/// tail call to a function in a cycle survives as a call: every one of them is
/// a `Continue`, and every function that was the target of one has a `Loop`
/// for a body.
///
/// Idempotent by construction rather than by a guard: a second run finds no
/// tail *call* left to rewrite, because [`tail_callees`] counts `CallFn` and
/// the first run replaced each one.
pub fn rewrite(program: &mut Program) {
    let plan = analyze(program);

    // The merged functions are appended, so every index the plan holds — and
    // every index anywhere else in the program — still names what it named.
    // `dce` and `inline` both depend on that, and both say so.
    let groups = plan.groups.clone();
    for (gi, group) in groups.iter().enumerate() {
        merge_group(program, gi, group);
    }

    for f in 0..program.funcs.len() {
        if !plan.is_self_loop(f) {
            continue;
        }
        let Some(func) = program.funcs.get_mut(f) else { continue };
        let Some(mut body) = func.take_body() else { continue };
        rewrite_tails(&mut body, &|callee| (callee == f).then_some(0));
        let mut entries = vec![body];
        snapshot_captures(&mut func.locals, &mut func.params, &mut entries);
        let ty = func.ret.clone();
        let span = func.span;
        func.set_body(Expr::new(ExprKind::Loop { entries }, ty, span));
    }
}

/// Turns one mutually tail-recursive group into one function with a `Loop` of
/// as many entries as the group has members, and its members into forwarders.
///
/// The members' locals are appended to the merged function's table and shifted
/// by where they landed, which is the inliner's own manoeuvre and for the same
/// reason: `LocalId` is an index into one function's table, so appending is the
/// only way two bodies can share one.
fn merge_group(program: &mut Program, gi: usize, group: &[usize]) {
    // A member with no body cannot have had a tail call, so it cannot have
    // been in the group; a group that somehow holds one is left alone rather
    // than half-merged.
    if group.iter().any(|f| program.funcs.get(*f).is_none_or(|f| f.body().is_none())) {
        return;
    }
    let arity = max_arity(program, group);
    let Some(first) = program.funcs.get(*group.first().unwrap_or(&0)) else { return };
    let ret = first.ret.clone();
    let span = first.span;

    // One slot per parameter position, wide enough for the widest member. The
    // type is the first member that has a parameter there, which is what the
    // shared slot has always been: a member's own parameters are read out of
    // it and nothing reads a slot at another member's type.
    let mut locals: Vec<typed::Local> = Vec::new();
    for j in 0..arity {
        let ty = group
            .iter()
            .filter_map(|f| program.funcs.get(*f))
            .find_map(|f| f.params.get(j).and_then(|p| f.locals.get(p.index())))
            .map(|l| l.ty.clone())
            .unwrap_or(crate::compiler::semantics::types::Ty::Unit);
        locals.push(typed::Local { name: format!("a{j}"), ty, span });
    }
    let mut params: Vec<LocalId> = (0..arity).map(|j| LocalId(j as u32)).collect();

    let mut entries: Vec<Expr> = Vec::new();
    for f in group {
        let Some(func) = program.funcs.get(*f) else { return };
        let Some(body) = func.body() else { return };
        let offset = locals.len() as u32;
        locals.extend(func.locals.iter().cloned());
        let mut body = body.clone();
        shift_expr(&mut body, offset);
        // The member's parameters *are* the shared slots, so a read of one
        // reads the slot rather than a copy of it.
        let onto: HashMap<LocalId, LocalId> = func
            .params
            .iter()
            .enumerate()
            .filter_map(|(j, p)| Some((LocalId(p.0.checked_add(offset)?), *params.get(j)?)))
            .collect();
        substitute_locals(&mut body, &onto);
        rewrite_tails(&mut body, &|callee| group.iter().position(|m| *m == callee));
        entries.push(body);
    }

    snapshot_captures(&mut locals, &mut params, &mut entries);

    // What the group actually returns.
    //
    // `Func::ret` is `()` for every function with a body — it is filled in for
    // an intrinsic and for nothing else (`monomorphize.rs`) — so `ret` above is
    // `()` here, and `lower::returns` reads a *body's* type rather than that
    // field for exactly this reason. The merged function survives that because
    // its body is a `Loop` and `returns` special-cases one to its first entry;
    // a member's body is a `Continue`, which has no such arm, so a member
    // labelled `()` is lowered with a signature that returns nothing and every
    // caller of it reads an undefined value. The result is silent: a native
    // program printing `even(3)` printed the empty string rather than `false`.
    //
    // So the entries' own type is what the forwarders are labelled with. It is
    // read before `entries` moves into the merged function below.
    let ret_ty = entries.first().map_or_else(|| ret.clone(), |e| e.ty.clone());

    let merged = FuncIdx(program.funcs.len() as u32);
    let members: Vec<&str> = group
        .iter()
        .filter_map(|f| program.funcs.get(*f))
        .map(|f| f.debug_name.as_str())
        .collect();
    program.funcs.push(Func {
        symbol: format!("$tc{gi}"),
        debug_name: format!("tail group {}", members.join(", ")),
        params,
        locals,
        kind: FuncKind::Body(Expr::new(ExprKind::Loop { entries }, ret_ty.clone(), span)),
        ret: ret.clone(),
        desc: None,
        span,
    });

    // Each member keeps its name and its arity and forwards into the merged
    // function at its own entry, so a reference to it from outside the group —
    // a call that was not in tail position, or an `FnRef` — still works.
    for (i, f) in group.iter().enumerate() {
        let Some(func) = program.funcs.get_mut(*f) else { continue };
        let args: Vec<Expr> = func
            .params
            .iter()
            .filter_map(|p| {
                let l = func.locals.get(p.index())?;
                Some(Expr::new(ExprKind::Local(*p), l.ty.clone(), l.span))
            })
            .collect();
        let span = func.span;
        func.set_body(Expr::new(
            ExprKind::Continue { func: Some(merged), entry: i, args },
            ret_ty.clone(),
            span,
        ));
    }
}

/// Replaces every tail call `jump` claims with a `Continue` into the enclosing
/// loop, at the entry it answered with.
///
/// The forms it descends into are exactly the forms [`tail_callees`] counts,
/// and that is the whole point of the pair: the analysis that decided a
/// function loops and the rewrite that makes it loop cannot disagree about
/// what tail position is, because disagreeing means one of these two matches
/// has an arm the other does not.
fn rewrite_tails(e: &mut Expr, jump: &impl Fn(usize) -> Option<usize>) {
    match &mut e.kind {
        ExprKind::CallFn { func, args } => {
            let Some(entry) = func.func().map(|i| i.index()).and_then(jump) else { return };
            let args = std::mem::take(args);
            e.kind = ExprKind::Continue { func: None, entry, args };
        }
        ExprKind::Block { tail: Some(t), .. } => rewrite_tails(t, jump),
        ExprKind::If { then, else_, .. } => {
            rewrite_tails(then, jump);
            rewrite_tails(else_, jump);
        }
        ExprKind::Match { arms, .. } => {
            for a in arms {
                rewrite_tails(&mut a.body, jump);
            }
        }
        ExprKind::And { rhs, .. } | ExprKind::Or { rhs, .. } | ExprKind::Coalesce { rhs, .. } => {
            rewrite_tails(rhs, jump)
        }
        _ => {}
    }
}

/// Renames the locals `onto` names, wherever they are read.
///
/// Only reads: a parameter is never rebound by a pattern, so nothing in a body
/// *binds* one of these ids. A lambda's capture list holds ids too, and it is
/// the one place other than `Local` that names a local it did not bind.
fn substitute_locals(e: &mut Expr, onto: &HashMap<LocalId, LocalId>) {
    match &mut e.kind {
        ExprKind::Local(l) => {
            if let Some(to) = onto.get(l) {
                *l = *to;
            }
        }
        ExprKind::Lambda { captures, .. } => {
            for c in captures.iter_mut() {
                if let Some(to) = onto.get(c) {
                    *c = *to;
                }
            }
        }
        _ => {}
    }
    crate::compiler::middle::inline::each_child_mut(e, &mut |child| {
        substitute_locals(child, onto);
    });
}

/// Gives each iteration its own binding for every parameter a closure in the
/// loop captures.
///
/// The loop rebinds its parameters in place, which is exact for every read
/// within the iteration that wrote them and wrong for a closure: a closure
/// keeps the *slot*, so a lambda made on one iteration would see the next
/// iteration's value — the bug every language with a loop and a closure has
/// had once. The fix is a fresh binding per iteration, and it belongs here
/// rather than in an emitter because it is a property of the rewrite: the
/// rewrite is what turned a parameter that could not change into a slot that
/// does.
///
/// A parameter no closure captures is left alone, and so is one the loop only
/// ever rebinds to itself: neither can be stale.
fn snapshot_captures(
    locals: &mut Vec<typed::Local>,
    params: &mut [LocalId],
    entries: &mut [Expr],
) {
    let mut captured: HashSet<LocalId> = HashSet::default();
    let mut rebound: HashSet<LocalId> = HashSet::default();
    for entry in entries.iter() {
        typed::walk(entry, &mut |e| match &e.kind {
            ExprKind::Lambda { captures, .. } => captured.extend(captures.iter().copied()),
            ExprKind::Continue { func: None, args, .. } => {
                for (j, arg) in args.iter().enumerate() {
                    let Some(param) = params.get(j) else { continue };
                    if !matches!(&arg.kind, ExprKind::Local(l) if l == param) {
                        rebound.insert(*param);
                    }
                }
            }
            _ => {}
        });
    }

    let mut prologue: Vec<Stmt> = Vec::new();
    for j in 0..params.len() {
        let Some(&param) = params.get(j) else { continue };
        if !captured.contains(&param) || !rebound.contains(&param) {
            continue;
        }
        let Some(local) = locals.get(param.index()).cloned() else { continue };
        let carrier = LocalId(locals.len() as u32);
        locals.push(typed::Local {
            name: format!("{}_loop", local.name),
            ty: local.ty.clone(),
            span: local.span,
        });
        if let Some(slot) = params.get_mut(j) {
            *slot = carrier;
        }
        // Re-executed on every entry to the loop body, which is what makes the
        // binding a closure captures belong to that iteration alone.
        prologue.push(Stmt::Let {
            pattern: typed::Pattern {
                kind: typed::PatKind::Bind { local: param, sub: None },
                ty: local.ty.clone(),
                span: local.span,
            },
            value: Expr::new(ExprKind::Local(carrier), local.ty, local.span),
            span: local.span,
        });
    }
    if prologue.is_empty() {
        return;
    }
    for entry in entries.iter_mut() {
        let ty = entry.ty.clone();
        let span = entry.span;
        let inner = std::mem::replace(entry, Expr::new(ExprKind::Unit, ty.clone(), span));
        *entry = Expr::new(
            ExprKind::Block { stmts: prologue.clone(), tail: Some(Box::new(inner)) },
            ty,
            span,
        );
    }
}

/// The locals a group member's parameters occupy, for the merged function.
pub fn max_arity(program: &Program, group: &[usize]) -> usize {
    group.iter().filter_map(|f| program.funcs.get(*f)).map(|f| f.params.len()).max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::rewrite;
    use crate::compiler::middle::monomorphize::{Func, FuncKind, Program, ProgramRoots};
    use crate::compiler::semantics::typed::{Callee, Expr, ExprKind, Local, PatKind};
    use crate::compiler::semantics::types::{FuncIdx, LocalId, Ty};
    use crate::diagnostics::Span;
    use crate::hash::Map as HashMap;

    fn e(kind: ExprKind) -> Expr {
        Expr::new(kind, Ty::Unit, Span::default())
    }

    fn call(to: u32, args: Vec<Expr>) -> Expr {
        e(ExprKind::CallFn { func: Callee::Func(FuncIdx(to)), args })
    }

    fn local(name: &str) -> Local {
        Local { name: name.to_string(), ty: Ty::Unit, span: Span::default() }
    }

    fn func(symbol: &str, params: Vec<u32>, locals: Vec<Local>, body: Expr) -> Func {
        Func {
            symbol: symbol.to_string(),
            debug_name: symbol.to_string(),
            params: params.into_iter().map(LocalId).collect(),
            locals,
            kind: FuncKind::Body(body),
            ret: Ty::Unit,
            desc: None,
            span: Span::default(),
        }
    }

    fn program(funcs: Vec<Func>) -> Program {
        Program {
            funcs,
            roots: ProgramRoots::Main(FuncIdx(0)),
            descriptors: Vec::new(),
            desc_modules: Vec::new(),
            desc_index: HashMap::default(),
            ctx_layouts: HashMap::default(),
            shapes: Default::default(),
            stylesheet: String::new(),
        }
    }

    /// The body loops and the call is gone: what used to be a `Plan` the
    /// emitter read back is now the shape of the tree.
    #[test]
    fn a_self_tail_call_becomes_a_loop_and_a_jump() {
        let mut p = program(vec![func(
            "f",
            vec![0],
            vec![local("n")],
            call(0, vec![e(ExprKind::Local(LocalId(0)))]),
        )]);
        rewrite(&mut p);
        let ExprKind::Loop { entries } = &p.funcs[0].body().unwrap().kind else {
            panic!("a self tail call is a loop");
        };
        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0].kind, ExprKind::Continue { func: None, entry: 0, .. }));
    }

    /// The right operand of `??` is the whole expression's value whenever it is
    /// reached, so a call there is a tail call — the shape the historical
    /// miscompile turned on, and the one an analysis and an emitter could
    /// disagree about.
    #[test]
    fn a_call_after_a_short_circuit_is_a_tail_call() {
        let body = e(ExprKind::Or {
            lhs: Box::new(e(ExprKind::Bool(false))),
            rhs: Box::new(call(0, Vec::new())),
        });
        let mut p = program(vec![func("f", Vec::new(), Vec::new(), body)]);
        rewrite(&mut p);
        let ExprKind::Loop { entries } = &p.funcs[0].body().unwrap().kind else {
            panic!("a tail call under `||` still loops");
        };
        let ExprKind::Or { rhs, .. } = &entries[0].kind else { panic!("the operator stays") };
        assert!(matches!(rhs.kind, ExprKind::Continue { func: None, .. }));
    }

    /// A call in argument position is not a tail call, and rewriting one would
    /// be a loop that never returns the value it was asked for.
    #[test]
    fn a_call_that_is_not_in_tail_position_is_left_alone() {
        let mut p = program(vec![func(
            "f",
            vec![0],
            vec![local("n")],
            e(ExprKind::Tuple(vec![call(0, Vec::new())])),
        )]);
        rewrite(&mut p);
        assert!(matches!(p.funcs[0].body().unwrap().kind, ExprKind::Tuple(_)));
    }

    /// Two functions that tail-call each other become one loop with two
    /// entries, and two forwarders that keep their names and their arity.
    #[test]
    fn a_mutual_group_merges_into_one_function_with_two_entries() {
        let mut p = program(vec![
            func("a", vec![0], vec![local("x")], call(1, vec![e(ExprKind::Local(LocalId(0)))])),
            func("b", vec![0], vec![local("y")], call(0, vec![e(ExprKind::Local(LocalId(0)))])),
        ]);
        rewrite(&mut p);
        assert_eq!(p.funcs.len(), 3);
        let ExprKind::Loop { entries } = &p.funcs[2].body().unwrap().kind else {
            panic!("the merged function loops");
        };
        assert_eq!(entries.len(), 2);
        // Member 0's body jumps to entry 1, and member 1's to entry 0.
        assert!(matches!(entries[0].kind, ExprKind::Continue { func: None, entry: 1, .. }));
        assert!(matches!(entries[1].kind, ExprKind::Continue { func: None, entry: 0, .. }));
        // The members keep their symbols and forward at their own entry.
        assert_eq!(p.funcs[0].symbol, "a");
        assert!(matches!(
            p.funcs[0].body().unwrap().kind,
            ExprKind::Continue { func: Some(FuncIdx(2)), entry: 0, .. }
        ));
        assert!(matches!(
            p.funcs[1].body().unwrap().kind,
            ExprKind::Continue { func: Some(FuncIdx(2)), entry: 1, .. }
        ));
    }

    /// The Elm bug (elm/compiler#2268): the loop rebinds a slot, and a closure
    /// keeps the slot rather than the value, so every iteration's lambda must
    /// read a binding of its own.
    #[test]
    fn a_parameter_a_closure_captures_is_snapshotted_per_iteration() {
        let lambda = e(ExprKind::Lambda {
            params: Vec::new(),
            body: Box::new(e(ExprKind::Local(LocalId(0)))),
            captures: vec![LocalId(0)],
        });
        let body = e(ExprKind::Block {
            stmts: vec![crate::compiler::semantics::typed::Stmt::Expr(lambda)],
            tail: Some(Box::new(call(0, vec![e(ExprKind::Unit)]))),
        });
        let mut p = program(vec![func("f", vec![0], vec![local("n")], body)]);
        rewrite(&mut p);

        // The parameter is now a carrier, and the original id is bound from it
        // at the top of every iteration.
        assert_eq!(p.funcs[0].params, vec![LocalId(1)]);
        assert_eq!(p.funcs[0].locals[1].name, "n_loop");
        let ExprKind::Loop { entries } = &p.funcs[0].body().unwrap().kind else {
            panic!("it loops");
        };
        let ExprKind::Block { stmts, .. } = &entries[0].kind else { panic!("prologue first") };
        let crate::compiler::semantics::typed::Stmt::Let { pattern, value, .. } = &stmts[0]
        else {
            panic!("the prologue binds")
        };
        assert!(matches!(pattern.kind, PatKind::Bind { local: LocalId(0), .. }));
        assert!(matches!(value.kind, ExprKind::Local(LocalId(1))));
    }

    /// A parameter the loop only ever rebinds to itself cannot be stale, so it
    /// costs nothing: the tighter output is kept where it is exact.
    #[test]
    fn a_parameter_that_never_changes_needs_no_snapshot() {
        let lambda = e(ExprKind::Lambda {
            params: Vec::new(),
            body: Box::new(e(ExprKind::Local(LocalId(0)))),
            captures: vec![LocalId(0)],
        });
        let body = e(ExprKind::Block {
            stmts: vec![crate::compiler::semantics::typed::Stmt::Expr(lambda)],
            tail: Some(Box::new(call(0, vec![e(ExprKind::Local(LocalId(0)))]))),
        });
        let mut p = program(vec![func("f", vec![0], vec![local("n")], body)]);
        rewrite(&mut p);
        assert_eq!(p.funcs[0].params, vec![LocalId(0)]);
        assert_eq!(p.funcs[0].locals.len(), 1);
    }
}
