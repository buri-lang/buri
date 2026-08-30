//! The middle end: everything between the front end and a backend.
//!
//! This was `transform/`, and the rename is the point rather than a tidy-up. A
//! `transform` module is a place passes go; a middle end is the half of a
//! compiler between type checking and code generation, and it holds things
//! that are not transformations of anything — the value model (`layout`) is a
//! table, and `ir` is a representation. `design/native/ARCHITECTURE.md` §2 is
//! where that is argued.
//!
//! # Two layers
//!
//! **Layer A — the tree.** `monomorphize` through `closures`, operating on
//! `typed::Expr` bodies. *Every* backend consumes this. It is whole-program:
//! monomorphization makes the call graph exact, so a decision taken here is a
//! fact rather than an estimate.
//!
//! **Layer B — the CFG.** [`ir`], a per-function control-flow graph of basic
//! blocks with block parameters, produced by [`lower`]. Only the native
//! backends consume it. JavaScript deliberately does not get one: going from a
//! CFG back to structured JavaScript needs a relooper, and a relooper would
//! turn a backend that prints readable code into one that prints a state
//! machine, for no gain — everything JavaScript needs from the shared work is
//! in layer A.
//!
//! ```text
//! monomorphize -> inline -> dce -> tail_calls -> decision
//!                                                   |
//!                 +---------------------------------+
//!                 |                                 |
//!                js       derives -> fuse -> closures -> rc -> layout -> lower -> ir
//!                                                                               |
//!                                                             +-----------------+-------------+
//!                                                             |                               |
//!                                                         stencil                            llvm
//! ```
//!
//! The branch is real: closure conversion is a *pessimisation* in JavaScript,
//! where an arrow function closing over its scope is exactly what the engine
//! wants, so the JS backend is handed the tree before `closures` runs and the
//! native backends after it. `derives` is on the native branch only —
//! JavaScript walks a type descriptor at run time — and `fuse` is there too so
//! that the unfused JavaScript stays the reference the agreement tests compare
//! both natives against (`fuse.rs`'s header).
//!
//! `rc` is the one that is on both, and it is on them for different halves of
//! itself. The native branch runs it here for the whole thing. The JavaScript
//! backend calls `rc::sharing` for itself, out of `backend::js::generate`,
//! and reads only where a *second reference* comes into existence: a garbage
//! collector needs no releases, and what it cannot supply is the `rc == 1` test
//! an in-place update asks. MEMORY.md §5.5.
//!
//! # The module list is deliberately complete
//!
//! Every module of the middle end is declared here, in one list, so the
//! pipeline above can be read against it: a pass no arrow mentions, or an arrow
//! with no module behind it, is visible from this file alone. See
//! `design/native/BUILD-AND-WATCH.md` §5.

pub mod closures;
pub mod dce;
pub mod decision;
pub mod derives;
pub mod fuse;
pub mod inline;
pub mod ir;
pub mod layout;
pub mod lower;
pub mod monomorphize;
pub mod rc;
pub mod tail_calls;

use crate::compiler::middle::monomorphize::Program;

/// What the shared half of the middle end is allowed to do.
///
/// One field, and every caller in the toolchain passes `default()`: inlining
/// is off only where a test wants to read a body it can predict. A struct
/// rather than a `bool` parameter because the passes below take the whole of
/// it, and because a second thing a caller may switch off belongs beside the
/// first rather than in a second parameter list.
#[derive(Default)]
pub struct Options {
    pub inline: inline::Options,
}

/// Layer A: the passes every backend's input has been through.
///
/// Called once per compilation, on the monomorphized program, before any
/// backend sees it. Returning nothing and mutating in place is deliberate:
/// function indices never move (`inline.rs`'s own invariant), and a pipeline
/// that returned a new `Program` would make that promise harder to keep than
/// to state.
pub fn run(program: &mut Program, opts: &Options) {
    inline::run(program, &opts.inline);
    // Inlining creates *new* dead functions — a body inlined at its single
    // call site leaves the original unreachable — so reachability is
    // recomputed after it rather than only at monomorphization.
    dce::run(program);
    // `tail_calls` and `decision` are decisions about the *program*, not about
    // an emitter, which is why they run here for every backend rather than
    // inside the one that has them today.
    tail_calls::rewrite(program);
    decision::run(program);
}

/// The extra passes the native branch runs, after [`run`] and before
/// [`lower`].
///
/// Separate from [`run`] because JavaScript must not run them: `derives`
/// replaces a run-time descriptor walk that JavaScript wants, `closures`
/// replaces lexical capture that JavaScript already has, and `fuse` deletes an
/// intermediate list whose cost is `malloc` plus a copy here and a bump pointer
/// in a nursery there — while leaving JavaScript as the unfused reference the
/// agreement tests compare both natives against (`fuse.rs`'s header).
///
/// **The [`rc::Plan`] is returned rather than dropped.** `rc` is an analysis —
/// `rc::run` takes the program by shared reference — and the plan it produces
/// is exactly what [`lower::run`] recomputes for itself a moment later. Computing it here and throwing it away was a whole-program
/// ownership, purity and placement analysis run twice per native build, 55 ms
/// of it at a hundred thousand lines. The pipeline in this module's header is
/// unchanged; `rc` still runs between `closures` and `lower`, and it runs
/// *there*, on the program in exactly the state `lower` will see, because
/// nothing between here and the backend takes the program by `&mut`.
pub fn native(program: &mut Program) -> rc::Plan {
    derives::run(program);
    // After `derives` so that a generated body's own combinator chains fuse,
    // and before `closures` because fusion composes the *lambdas* and
    // `closures` is what turns a lambda into a lifted function.
    fuse::run(program);
    closures::run(program);
    rc::run(program)
}

/// What Tarjan's algorithm needs to know about one node, as one row rather than
/// three vectors that had to stay the same length.
#[derive(Clone, Copy)]
struct Node {
    /// The order the node was first reached in, or `usize::MAX` before that.
    index: usize,
    /// The lowest `index` reachable from it without leaving the current stack.
    low: usize,
    on_stack: bool,
}

/// The strongly connected components of a graph given as an adjacency list.
///
/// Two passes want the same answer and want it for the same reason: a cycle in
/// the call graph is a body the inliner must not paste into itself
/// (`inline`), and a cycle in the *tail*-call graph is a group that has to be
/// merged into one dispatching function (`tail_calls`). They had a copy each,
/// which is two chances to get an iterative Tarjan subtly wrong.
///
/// Iterative rather than recursive so a deep graph cannot exhaust the stack.
/// Each group is sorted, and the groups come out in a deterministic order,
/// because build output is compared byte for byte
/// (`two_checkouts_of_one_tree_build_identical_bytes`).
///
/// An edge naming a node the graph does not have is skipped: nothing in this
/// compiler produces one, and dropping it is what the bounds check that used to
/// stand in front of the caller's own push did.
#[allow(
    clippy::arithmetic_side_effects,
    reason = "the counter and the edge cursor are bounded by the node and edge counts of a graph already held in memory"
)]
fn strongly_connected(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = edges.len();
    let mut nodes = vec![Node { index: usize::MAX, low: 0, on_stack: false }; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut next = 0usize;
    let mut out: Vec<Vec<usize>> = Vec::new();

    for root in 0..n {
        match nodes.get(root) {
            Some(r) if r.index == usize::MAX => {}
            _ => continue,
        }
        // (node, how many of its edges have been taken)
        let mut work: Vec<(usize, usize)> = vec![(root, 0)];
        while let Some((v, taken)) = work.pop() {
            if taken == 0 {
                let Some(node) = nodes.get_mut(v) else { continue };
                node.index = next;
                node.low = next;
                node.on_stack = true;
                next += 1;
                stack.push(v);
            }

            // `low` is accumulated in a local and written back once, so that
            // reading a successor's row and updating this one do not overlap.
            let Some(&Node { index, mut low, .. }) = nodes.get(v) else { continue };
            let mut descended = false;
            for (k, &w) in
                edges.get(v).map(Vec::as_slice).unwrap_or_default().iter().enumerate().skip(taken)
            {
                let Some(succ) = nodes.get(w) else { continue };
                if succ.index == usize::MAX {
                    work.push((v, k + 1));
                    work.push((w, 0));
                    descended = true;
                    break;
                } else if succ.on_stack {
                    low = low.min(succ.index);
                }
            }
            if let Some(node) = nodes.get_mut(v) {
                node.low = low;
            }
            if descended {
                continue;
            }

            if low == index {
                let mut group = Vec::new();
                while let Some(w) = stack.pop() {
                    if let Some(node) = nodes.get_mut(w) {
                        node.on_stack = false;
                    }
                    group.push(w);
                    if w == v {
                        break;
                    }
                }
                group.sort_unstable();
                out.push(group);
            }
            if let Some(&(parent, _)) = work.last() {
                if let Some(node) = nodes.get_mut(parent) {
                    node.low = node.low.min(low);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::strongly_connected;

    #[test]
    fn cycles_are_found_and_singletons_are_not_called_recursive() {
        // 0 -> 1 -> 2 -> 1, and 3 alone.
        let edges = vec![vec![1], vec![2], vec![1], vec![]];
        let mut groups = strongly_connected(&edges);
        groups.sort();
        let cycles: Vec<Vec<usize>> = groups.into_iter().filter(|g| g.len() > 1).collect();
        assert_eq!(cycles, vec![vec![1, 2]]);
    }

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

    /// An edge out of the graph is dropped rather than indexed.
    #[test]
    fn an_edge_to_a_node_that_does_not_exist_is_skipped() {
        let edges = vec![vec![7], vec![0]];
        let sccs = strongly_connected(&edges);
        assert_eq!(sccs.len(), 2);
    }
}
