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
//!    a tail call too. `tail_callees` below decides this, and
//!    `generate::tail` has to agree with it — a shape one of them counts and
//!    the other does not gives a `while (true)` nothing ever continues, which
//!    looks like elimination and is not.
//!  * **What the loop rebinds.** The loop assigns the parameters in place,
//!    which is exact for every read within the iteration that wrote them and
//!    wrong for a closure, which keeps the slot rather than the value. See
//!    `generate::snapshot_captures`.

use crate::compiler::semantics::typed::{Expr, ExprKind};
use crate::compiler::transform::monomorphize::Program;
use std::collections::{HashMap, HashSet};

pub struct Plan {
    /// Functions that tail-call themselves and nothing else in a cycle.
    pub self_loop: HashSet<usize>,
    /// Mutually tail-recursive groups, each merged into one function.
    pub groups: Vec<Vec<usize>>,
    /// Which group a function belongs to, and its index within it.
    pub member: HashMap<usize, (usize, usize)>,
}

impl Plan {
    pub fn group_of(&self, f: usize) -> Option<(usize, usize)> {
        self.member.get(&f).copied()
    }
}

/// Collects the functions called in tail position.
pub fn tail_callees(e: &Expr, out: &mut Vec<usize>) {
    match &e.kind {
        ExprKind::CallFn { func, .. } => out.push(func.index()),
        ExprKind::Block { tail, .. } => {
            if let Some(t) = tail {
                tail_callees(t, out);
            }
        }
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
    for (i, f) in program.funcs.iter().enumerate() {
        if let Some(body) = &f.body {
            let mut callees = Vec::new();
            tail_callees(body, &mut callees);
            callees.sort();
            callees.dedup();
            edges[i] = callees;
        }
    }

    let sccs = strongly_connected(&edges);
    let mut plan = Plan { self_loop: HashSet::new(), groups: Vec::new(), member: HashMap::new() };
    for scc in sccs {
        if scc.len() == 1 {
            let f = scc[0];
            if edges[f].contains(&f) {
                plan.self_loop.insert(f);
            }
            continue;
        }
        // A statically known group of functions that tail-call each other.
        let group = plan.groups.len();
        for (i, f) in scc.iter().enumerate() {
            plan.member.insert(*f, (group, i));
        }
        plan.groups.push(scc);
    }
    plan
}

/// Tarjan's algorithm, iterative so a deep graph does not exhaust the stack.
fn strongly_connected(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = edges.len();
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut counter = 0usize;
    let mut out = Vec::new();

    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        // (node, next edge to visit)
        let mut work: Vec<(usize, usize)> = vec![(root, 0)];
        while let Some((v, edge)) = work.pop() {
            if edge == 0 {
                index[v] = counter;
                low[v] = counter;
                counter += 1;
                stack.push(v);
                on_stack[v] = true;
            }
            let mut recursed = false;
            for (i, &w) in edges[v].iter().enumerate().skip(edge) {
                if index[w] == usize::MAX {
                    work.push((v, i + 1));
                    work.push((w, 0));
                    recursed = true;
                    break;
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
            }
            if recursed {
                continue;
            }
            if low[v] == index[v] {
                let mut scc = Vec::new();
                while let Some(w) = stack.pop() {
                    on_stack[w] = false;
                    scc.push(w);
                    if w == v {
                        break;
                    }
                }
                scc.sort();
                out.push(scc);
            }
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[v]);
            }
        }
    }
    out
}

/// The locals a group member's parameters occupy, for the merged function.
pub fn max_arity(program: &Program, group: &[usize]) -> usize {
    group.iter().map(|f| program.funcs[*f].params.len()).max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_self_loops_and_groups() {
        // 0 -> 0 (self), 1 <-> 2 (group), 3 -> 1 (not in a cycle)
        let edges = vec![vec![0], vec![2], vec![1], vec![1]];
        let sccs = strongly_connected(&edges);
        // Three components: {0}, {3}, and the pair {1, 2}.
        let mut sizes: Vec<usize> = sccs.iter().map(|s| s.len()).collect();
        sizes.sort();
        assert_eq!(sizes, vec![1, 1, 2]);
        let pair = sccs.iter().find(|s| s.len() == 2).unwrap();
        assert_eq!(pair, &vec![1, 2]);
    }

    #[test]
    fn a_chain_is_not_a_cycle() {
        let edges = vec![vec![1], vec![2], vec![]];
        let sccs = strongly_connected(&edges);
        assert!(sccs.iter().all(|s| s.len() == 1));
    }
}
