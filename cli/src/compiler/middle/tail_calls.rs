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
use crate::compiler::semantics::types::{FuncIdx, LocalId, Ty};
use crate::hash::{Map as HashMap, Set as HashSet};

/// How one function is emitted.
///
/// One value per function replaces a `HashSet` of self-loopers and a
/// `HashMap` of group members that had to be disjoint: a function in both got
/// a loop *and* a dispatch arm, and which one won depended on the order the
/// backend happened to test them in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Strategy {
    /// Emitted as an ordinary function.
    Plain,
    /// Tail-calls itself and nothing else in a cycle: emitted as a loop.
    SelfLoop,
    /// One of a mutually tail-recursive group, merged into one function with a
    /// dispatch switch.
    Member { group: usize, index: usize },
}

struct Plan {
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

    fn is_self_loop(&self, f: usize) -> bool {
        self.strategy(f) == Strategy::SelfLoop
    }
}

/// Collects the functions called in tail position.
fn tail_callees(e: &Expr, out: &mut Vec<usize>) {
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
fn analyze(program: &Program) -> Plan {
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
    for (group_index, group) in groups.iter().enumerate() {
        merge_group(program, group_index, group);
    }

    for f in 0..program.funcs.len() {
        if !plan.is_self_loop(f) {
            continue;
        }
        let Some(func) = program.funcs.get_mut(f) else { continue };
        let Some(mut body) = func.take_body() else { continue };
        // A self-loop's slots *are* its parameters, in order.
        let identity: Vec<usize> = (0..func.params.len()).collect();
        rewrite_tails(&mut body, &|callee| (callee == f).then_some((0, &identity[..])));
        let mut entries = vec![body];
        let owned = [func.params.len()];
        snapshot_captures(&mut func.locals, &mut func.params, &mut entries, &owned);
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
fn merge_group(program: &mut Program, group_index: usize, group: &[usize]) {
    // A member with no body cannot have had a tail call, so it cannot have
    // been in the group; a group that somehow holds one is left alone rather
    // than half-merged.
    if group.iter().any(|f| program.funcs.get(*f).is_none_or(|f| f.body().is_none())) {
        return;
    }
    let Some(first) = program.funcs.get(*group.first().unwrap_or(&0)) else { return };
    let ret = first.ret.clone();
    let span = first.span;

    // Which of a member's parameters each shared slot is, per member.
    //
    // A slot is shared, so it has one type and every member reading it reads it
    // at that type. The old rule — slot `j` is parameter `j`, typed by the
    // first member that has one there — is only sound where the members agree
    // position by position, and a group whose members do not is issue #29:
    // `walkFrom(ctx, [U8], Int, Walk)` and `walkOne(ctx, [U8], Int, U8, Walk)`
    // put a `Walk` and a `U8` in one slot, so the merged function compared a
    // pointer against 255 and released it as two different types. The IR
    // verifier says so now; before it existed the program simply corrupted the
    // heap.
    let Some(slots) = shared_slots(program, group) else {
        // No consistent assignment exists — two members disagree about what
        // the group's parameters *are*, not merely about their order. Leaving
        // the group unmerged costs it mutual tail-call elimination and keeps
        // it correct, which is the right way round; merging it was a
        // miscompile.
        return;
    };
    let mut locals: Vec<typed::Local> = slots
        .types
        .iter()
        .enumerate()
        .map(|(j, ty)| typed::Local { name: format!("a{j}"), ty: ty.clone(), span })
        .collect();
    let mut params: Vec<LocalId> = (0..slots.types.len()).map(|j| LocalId(j as u32)).collect();

    let mut entries: Vec<Expr> = Vec::new();
    for (i, f) in group.iter().enumerate() {
        let Some(func) = program.funcs.get(*f) else { return };
        let Some(body) = func.body() else { return };
        let offset = locals.len() as u32;
        locals.extend(func.locals.iter().cloned());
        let mut body = body.clone();
        shift_expr(&mut body, offset);
        // The member's parameters *are* the shared slots, so a read of one
        // reads the slot rather than a copy of it. `order[i][j]` is the
        // parameter slot `j` holds for this member, which is the identity
        // wherever the members' signatures agree.
        let Some(mine) = slots.order.get(i) else { return };
        let onto: HashMap<LocalId, LocalId> = mine
            .iter()
            .enumerate()
            .filter_map(|(j, k)| {
                let p = func.params.get(*k)?;
                Some((LocalId(p.0.checked_add(offset)?), *params.get(j)?))
            })
            .collect();
        substitute_locals(&mut body, &onto);
        let order = &slots.order;
        rewrite_tails(&mut body, &|callee| {
            let entry = group.iter().position(|m| *m == callee)?;
            Some((entry, order.get(entry)?.as_slice()))
        });
        // A slot this member owns and its body never reads still arrives
        // holding a count somebody has to spend. `middle::rc` drops a parameter
        // nothing reads at all, but "nothing" is asked of the whole merged
        // function, and a slot another member reads is not that. Binding it to
        // a name this entry drops is the same disposal written where only this
        // entry runs it — and it is what lets [`rc`]'s loop stop balancing its
        // entries against each other, which is the other half of #29: an entry
        // was releasing slots that belong to a *different* member and hold
        // undefined words on this path.
        let mut read: HashSet<LocalId> = HashSet::default();
        typed::walk(&body, &mut |e| {
            if let ExprKind::Local(l) = &e.kind {
                read.insert(*l);
            }
        });
        let mut dead: Vec<Stmt> = Vec::new();
        for j in 0..mine.len() {
            let Some(&slot) = params.get(j) else { continue };
            if read.contains(&slot) {
                continue;
            }
            let Some(local) = locals.get(slot.index()).cloned() else { continue };
            let bound = LocalId(locals.len() as u32);
            locals.push(typed::Local {
                name: format!("{}_unread", local.name),
                ty: local.ty.clone(),
                span: local.span,
            });
            dead.push(Stmt::Let {
                pattern: typed::Pattern {
                    kind: typed::PatKind::Bind { local: bound, sub: None },
                    ty: local.ty.clone(),
                    span: local.span,
                },
                value: Expr::new(ExprKind::Local(slot), local.ty, local.span),
                span: local.span,
            });
        }
        if !dead.is_empty() {
            let ty = body.ty.clone();
            let at = body.span;
            body = Expr::new(ExprKind::Block { stmts: dead, tail: Some(Box::new(body)) }, ty, at);
        }
        entries.push(body);
    }

    let owned: Vec<usize> = slots.order.iter().map(Vec::len).collect();
    snapshot_captures(&mut locals, &mut params, &mut entries, &owned);

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
        symbol: format!("$tc{group_index}"),
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
        // In *slot* order rather than the member's own: `lower::continue_`
        // hands argument `j` to slot `j` and pads what is missing, which is
        // what makes a member's slots a prefix a requirement of
        // [`shared_slots`] rather than a coincidence.
        let Some(mine) = slots.order.get(i) else { continue };
        let args: Vec<Expr> = mine
            .iter()
            .filter_map(|k| {
                let p = func.params.get(*k)?;
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
fn rewrite_tails<'a>(e: &mut Expr, jump: &impl Fn(usize) -> Option<(usize, &'a [usize])>) {
    match &mut e.kind {
        ExprKind::CallFn { func, args } => {
            let Some((entry, order)) = func.func().map(|i| i.index()).and_then(jump) else {
                return;
            };
            let mut args = std::mem::take(args);
            // The call's arguments are in the callee's own parameter order and
            // the jump's are in slot order, which is the identity wherever the
            // group's members agree about their parameters. See
            // [`shared_slots`].
            if order.iter().enumerate().any(|(j, k)| j != *k) {
                let mut taken: Vec<Option<Expr>> = args.drain(..).map(Some).collect();
                args = order.iter().filter_map(|k| taken.get_mut(*k)?.take()).collect();
            }
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
    typed::children_mut(e, &mut |child| {
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
    owned: &[usize],
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

    let mut prologue: Vec<(usize, Stmt)> = Vec::new();
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
        prologue.push((
            j,
            Stmt::Let {
                pattern: typed::Pattern {
                    kind: typed::PatKind::Bind { local: param, sub: None },
                    ty: local.ty.clone(),
                    span: local.span,
                },
                value: Expr::new(ExprKind::Local(carrier), local.ty, local.span),
                span: local.span,
            },
        ));
    }
    if prologue.is_empty() {
        return;
    }
    for (i, entry) in entries.iter_mut().enumerate() {
        // Only the slots this entry's member owns. A slot belonging to another
        // member holds undefined words on this path, and a snapshot of one is
        // a binding this entry would then release — the merged-group half of
        // the same mistake `owned` exists to stop.
        let mine: Vec<Stmt> = prologue
            .iter()
            .filter(|(j, _)| *j < owned.get(i).copied().unwrap_or(usize::MAX))
            .map(|(_, s)| s.clone())
            .collect();
        if mine.is_empty() {
            continue;
        }
        let ty = entry.ty.clone();
        let span = entry.span;
        let inner = std::mem::replace(entry, Expr::new(ExprKind::Unit, ty.clone(), span));
        *entry =
            Expr::new(ExprKind::Block { stmts: mine, tail: Some(Box::new(inner)) }, ty, span);
    }
}

/// The parameter slots a merged group shares, and which parameter of each
/// member fills each of them.
struct Slots {
    /// One type per slot. The merged function's parameter list.
    types: Vec<Ty>,
    /// `order[i][j]` is the index, in member `i`'s own parameter list, of the
    /// parameter that lives in slot `j`. Every member's slots are the prefix
    /// `0..order[i].len()`, which is what lets `lower::continue_` keep handing
    /// argument `j` to slot `j` and padding the tail with `undef`.
    order: Vec<Vec<usize>>,
}

/// Assigns one slot per parameter of the widest member, so that **every**
/// member reads every slot it owns at that slot's own type.
///
/// The members are taken narrowest first and each one has to *account for*
/// every slot already allocated — one parameter of its own, at that type, not
/// already spoken for — and then extends the list with whatever it has left
/// over. So the slot list grows monotonically and each member owns a prefix of
/// it. Where the members' signatures already agree the answer is the identity,
/// which is what the merged functions looked like before this existed.
///
/// `None` where no such assignment exists: two members of the same arity with
/// different parameter *types*, say. There is nothing to share then, and the
/// caller leaves the group unmerged rather than putting two types in one slot.
/// Issue #29 is what one slot at two types does — a `Walk` compared against
/// `255` and released as a `U8`.
fn shared_slots(program: &Program, group: &[usize]) -> Option<Slots> {
    let params: Vec<Vec<Ty>> = group
        .iter()
        .map(|f| {
            let func = program.funcs.get(*f)?;
            func.params
                .iter()
                .map(|p| func.locals.get(p.index()).map(|l| l.ty.clone()))
                .collect::<Option<Vec<Ty>>>()
        })
        .collect::<Option<Vec<_>>>()?;
    let mut narrowest: Vec<usize> = (0..group.len()).collect();
    narrowest.sort_by_key(|i| params.get(*i).map_or(0, Vec::len));

    let mut types: Vec<Ty> = Vec::new();
    let mut order: Vec<Vec<usize>> = vec![Vec::new(); group.len()];
    for i in narrowest {
        let mine = params.get(i)?;
        let mut taken = vec![false; mine.len()];
        let mut filled: Vec<usize> = Vec::with_capacity(mine.len());
        for ty in &types {
            let k = mine
                .iter()
                .zip(taken.iter())
                .position(|(m, spoken)| !*spoken && m == ty)?;
            if let Some(spoken) = taken.get_mut(k) {
                *spoken = true;
            }
            filled.push(k);
        }
        for ((k, ty), spoken) in mine.iter().enumerate().zip(taken.iter()) {
            if !*spoken {
                types.push(ty.clone());
                filled.push(k);
            }
        }
        if let Some(slot) = order.get_mut(i) {
            *slot = filled;
        }
    }
    Some(Slots { types, order })
}

#[cfg(test)]
mod tests {
    use super::rewrite;
    use crate::compiler::middle::monomorphize::{Func, FuncKind, Program, ProgramRoots};
    use crate::compiler::semantics::typed::{Callee, Expr, ExprKind, Local, PatKind, Stmt};
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

    fn typed(name: &str, ty: Ty) -> Local {
        Local { name: name.to_string(), ty, span: Span::default() }
    }

    /// Two types that are not `Ty::Unit` and not each other, which is all the
    /// slot allocation asks of them.
    fn a_ty() -> Ty {
        Ty::Array(Box::new(Ty::Unit))
    }

    fn b_ty() -> Ty {
        Ty::Tuple(vec![Ty::Unit])
    }

    /// The types of a function's parameters, in order.
    fn param_tys(f: &Func) -> Vec<Ty> {
        f.params.iter().filter_map(|p| f.locals.get(p.index()).map(|l| l.ty.clone())).collect()
    }

    /// Which local each argument of a `Continue` names, in order.
    fn arg_locals(e: &Expr) -> Vec<LocalId> {
        let ExprKind::Continue { args, .. } = &e.kind else { panic!("a `Continue`") };
        args.iter()
            .map(|a| match a.kind {
                ExprKind::Local(l) => l,
                _ => panic!("a forwarder passes its own parameters"),
            })
            .collect()
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
            inline_styles: false,
            themes: false,
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

    /// **A shared slot holds one type**, and a member whose parameters do not
    /// line up with another's is passed in slot order rather than its own.
    ///
    /// The old rule was slot `j` is parameter `j`, typed by the first member
    /// that had one there — sound only where the members agree position by
    /// position. `narrow(a)` and `wide(b, a)` do not: position 0 is an `a` for
    /// one and a `b` for the other, so one slot held two types, and the merged
    /// function read a value of one at the other. Issue #29, where a `Walk` was
    /// compared against `255` and released as a `U8`.
    ///
    /// [`shared_slots`] takes the members narrowest first, so the slots are
    /// `[a, b]` — `narrow`'s one parameter, then what `wide` has left over —
    /// and `wide` forwards its parameters the other way round.
    #[test]
    fn a_group_whose_parameters_disagree_gets_one_slot_per_type() {
        let mut p = program(vec![
            func(
                "narrow",
                vec![0],
                vec![typed("n", a_ty())],
                call(1, vec![e(ExprKind::Unit), e(ExprKind::Local(LocalId(0)))]),
            ),
            func(
                "wide",
                vec![0, 1],
                vec![typed("w", b_ty()), typed("v", a_ty())],
                call(0, vec![e(ExprKind::Local(LocalId(1)))]),
            ),
        ]);
        rewrite(&mut p);
        assert_eq!(p.funcs.len(), 3, "the group merged");
        // Narrowest first: `narrow`'s `a`, then the `b` only `wide` has.
        assert_eq!(param_tys(&p.funcs[2]), vec![a_ty(), b_ty()]);
        // `narrow` fills the one slot it owns; `wide` fills both, and its `a`
        // goes first because that is the slot's type.
        assert_eq!(arg_locals(p.funcs[0].body().expect("a forwarder")), vec![LocalId(0)]);
        assert_eq!(
            arg_locals(p.funcs[1].body().expect("a forwarder")),
            vec![LocalId(1), LocalId(0)]
        );
        // The tail call *inside* the merged function is permuted the same way:
        // `narrow` calls `wide(unit, n)`, whose `a` argument is second.
        let ExprKind::Loop { entries } = &p.funcs[2].body().expect("a body").kind else {
            panic!("the merged function loops")
        };
        let ExprKind::Continue { args, entry, .. } = &entries[0].kind else {
            panic!("`narrow` jumps to `wide`")
        };
        assert_eq!(*entry, 1);
        assert!(matches!(args[0].kind, ExprKind::Local(_)), "the `a` argument leads");
        assert!(matches!(args[1].kind, ExprKind::Unit), "the `b` argument follows");
    }

    /// A group with **no** consistent assignment is left unmerged.
    ///
    /// Two members of the same arity whose parameters are different types
    /// disagree about what the group's parameters *are*, not merely about their
    /// order, so there is no slot list both can read. Merging it anyway is what
    /// put two types in one slot; leaving it costs the group its mutual
    /// tail-call elimination and keeps it correct, which is the right way
    /// round.
    #[test]
    fn a_group_with_no_consistent_slots_is_left_alone() {
        let mut p = program(vec![
            func("a", vec![0], vec![typed("x", a_ty())], call(1, vec![e(ExprKind::Unit)])),
            func("b", vec![0], vec![typed("y", b_ty())], call(0, vec![e(ExprKind::Unit)])),
        ]);
        rewrite(&mut p);
        assert_eq!(p.funcs.len(), 2, "nothing was merged");
        assert!(matches!(p.funcs[0].body().expect("a body").kind, ExprKind::CallFn { .. }));
        assert!(matches!(p.funcs[1].body().expect("a body").kind, ExprKind::CallFn { .. }));
    }

    /// An entry **binds and drops** every slot it owns and does not read.
    ///
    /// A merged group's entries own disjoint prefixes of the parameter list, so
    /// `middle::rc` no longer balances them against each other — an entry that
    /// released a slot beyond its own arity was releasing the `undef`
    /// `lower::pad` puts there. The obligation that balancing used to discharge
    /// by accident is real for the entry that *does* own the slot, so it is
    /// discharged here instead, where only that entry runs it.
    ///
    /// `wide`'s second parameter is never read by `wide`'s body, and `narrow`
    /// has no parameter at that slot at all: exactly one of the two entries
    /// binds it.
    #[test]
    fn an_entry_disposes_of_a_slot_it_owns_and_never_reads() {
        let mut p = program(vec![
            func(
                "narrow",
                vec![0],
                vec![typed("n", a_ty())],
                call(1, vec![e(ExprKind::Local(LocalId(0))), e(ExprKind::Unit)]),
            ),
            func(
                "wide",
                vec![0, 1],
                vec![typed("v", a_ty()), typed("w", b_ty())],
                call(0, vec![e(ExprKind::Local(LocalId(0)))]),
            ),
        ]);
        rewrite(&mut p);
        let ExprKind::Loop { entries } = &p.funcs[2].body().expect("a body").kind else {
            panic!("the merged function loops")
        };
        // Entry 0 is `narrow`, which owns one slot and reads it.
        assert!(matches!(entries[0].kind, ExprKind::Continue { .. }), "no disposal to write");
        // Entry 1 is `wide`, which owns both and reads only the first.
        let ExprKind::Block { stmts, tail } = &entries[1].kind else {
            panic!("`wide` binds the slot it never reads")
        };
        assert_eq!(stmts.len(), 1, "one slot, one binding");
        let Stmt::Let { value, .. } = &stmts[0] else { panic!("a binding") };
        assert!(matches!(value.kind, ExprKind::Local(LocalId(1))), "it names the second slot");
        assert!(matches!(
            tail.as_ref().expect("the body follows").kind,
            ExprKind::Continue { .. }
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
