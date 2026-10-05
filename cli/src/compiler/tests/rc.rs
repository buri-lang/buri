//! `middle::rc` on whole snippets, through the real front end. Here rather
//! than beside the pass because a snippet compiles through `driver`, which is
//! in `buri`. `compile` and `find` are shared with the JavaScript backend's
//! `park` tests.
#![allow(
    clippy::arithmetic_side_effects,
    reason = "every counter here indexes a tree already in memory: a pre-order \
              node number bounded by the node count, a subtree size that is a \
              sum of subtree sizes, and a parameter index bounded by a \
              signature. The one subtraction is a `saturating_sub`."
)]

use crate::compiler::middle::ir;
use crate::compiler::middle::monomorphize::{self, Func, Program};
use crate::compiler::middle::rc::*;
use crate::compiler::semantics::typed::{self, Expr, ExprKind, Stmt};
use crate::compiler::semantics::types::{FuncIdx, LocalId, Ty, TyKind};
use crate::diagnostics::{Diagnostics, SourceMap};
use crate::hash::Map as HashMap;

pub(crate) fn compile(src: &str) -> Program {
    let mut map = SourceMap::new();
    let analysis = crate::compiler::driver::analyze_snippet(
        &mut map,
        "rc_test.buri",
        src,
        crate::compiler::modules::Role::Entry,
    );
    let errors: Vec<String> = analysis
        .diagnostics
        .items
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.message.clone())
        .collect();
    assert!(errors.is_empty(), "the snippet did not compile: {errors:?}");
    let entry = analysis.checked.entry.expect("the snippet exports `main`");
    let mut diags = Diagnostics::new();
    let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    // Deliberately *not* through `middle::run`: the inliner pastes a
    // one-call function into its caller and dead-code elimination then
    // takes the original away, so a test about a two-line function would
    // be a test about a function that is no longer there. This pass reads
    // the tree, and the tree is the same shape either way — the balance
    // tests below run over every function of the standard library that the
    // snippet reaches, which is where the interesting shapes come from.
    monomorphize::run(&analysis.checked, paths, &mut diags, monomorphize::Roots::Main(entry))
}

pub(crate) fn find(program: &Program, name: &str) -> FuncIdx {
    let i = program
        .funcs
        .iter()
        .position(|f| f.debug_name.ends_with(name))
        .unwrap_or_else(|| panic!("no function named {name}"));
    FuncIdx(i as u32)
}

// -- the escape question ------------------------------------------------

/// **A program that can reach a task boundary is marked, and one that
/// cannot is not.**
///
/// [`crosses_tasks`]'s two directions. The positive half is easy and the
/// negative half is the one worth having: this answer puts a whole program
/// on atomic reference counting, so a `true` reached by accident — an
/// import that pulls `core/tasks` in without calling it, a key spelled by
/// prefix that matches something else — is a cost every program pays.
///
/// The question is asked of the **post-monomorphization** program, so it
/// is reachability and not mention: a `Tasks` binding the entry never
/// calls through is not a function in `program.funcs`.
#[test]
fn only_a_program_that_can_reach_a_task_boundary_is_marked() {
    let plain = run(&compile(
        r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/str" import * as str;

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, str.format(ctx, "${1 + 1}")).ignore();
  .Ok(())
}
"#,
    ));
    assert!(
        !plain.crosses_tasks,
        "a program with no task boundary in it was put on atomic counting"
    );

    let program = compile(
        r#"
from "platform/effect" import { Allocator, Stdout, Tasks };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/str" import * as str;
from "core/tasks" import * as tasks;

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout, Tasks: host.tasks };
  let doubled = tasks.parallel(ctx, [1, 2, 3], fn(c, i, n) => n * 2);
  let _ = io.println(ctx, str.format(ctx, "${doubled.length()}")).ignore();
  .Ok(())
}
"#,
    );
    assert!(run(&program).crosses_tasks, "a program that fans out was not marked");
}

/// The key list is a **prefix**, and that is the direction an omission has
/// to be wrong in.
///
/// `crosses_tasks` is what decides whether a whole program's blocks are
/// counted atomically, and a key missing from it is a value the program is
/// promised nobody else can see — a promise kept by non-atomic counts on
/// both backends. So the surface is spelled once, and every row track F
/// adds to `host.HostTasks` is covered on the day it lands rather than on
/// the day somebody remembers.
#[test]
fn every_task_host_key_crosses_and_nothing_else_does() {
    assert!(crosses_tasks("host.HostTasks.parallel"));
    // The rows that do not exist yet, and are covered anyway.
    assert!(crosses_tasks("host.HostTasks.start"));
    assert!(crosses_tasks("host.HostTasks.send"));
    // `core/tasks`'s scopes: a spawned task waits in one until a round on
    // another thread picks it up.
    assert!(crosses_tasks("tasks.scopePush"));
    assert!(crosses_tasks("tasks.scopeTaskAt"));
    // The module's other keys are not the scope's: `tasks.parallel` is the
    // Buri wrapper and hands nothing over itself.
    assert!(!crosses_tasks("tasks.parallel"));

    // Everything that waits but hands nothing over: `suspends` and
    // `crosses_tasks` are different questions about the same list, and
    // `Tasks.parallel` is the one key on both.
    for key in [
        "host.HostFileSystem.readText",
        "host.HostNetwork.fetch",
        "host.HostClock.sleepMilliseconds",
        "host.HostStdin.readLine",
        "host.HostStdout.println",
        "host_testing.TestTasks.parallel",
    ] {
        assert!(!crosses_tasks(key), "{key} put its program on atomic counting");
    }
    assert!(
        crate::compiler::backend::js::park::suspends("host.HostTasks.parallel")
            && crosses_tasks("host.HostTasks.parallel")
    );
}

// -- the balance checker ------------------------------------------------

/// Replays a function's plan along **every** path and asserts the counts
/// balance: every owned local ends at zero, no count ever goes negative,
/// and every branch of a join agrees.
///
/// This walks forward, in evaluation order, which is the opposite of the
/// direction the analysis works in — so it is a check and not a rerun.
struct Balance<'a> {
    func: &'a Func,
    plan: &'a FuncPlan,
    sizes: Vec<u32>,
    counted: Syntactic,
    own: &'a [Vec<ir::Ownership>],
    /// A node whose `After` operations are held back, because the binding
    /// they belong to does not exist until the value has been computed.
    suppress: Option<NodeId>,
    /// What the state is when the function — and therefore the loop header
    /// — is entered: one count per owned counted parameter.
    header: State,
    /// Arm bodies whose pattern allocated before they were entered, and
    /// what it bound. `..rest` is the only such binding (VALUE-MODEL.md
    /// §4.2): the count exists because the pattern called the allocator,
    /// so there is no site to read it from and the replay has to know.
    fresh_at: HashMap<NodeId, Vec<LocalId>>,
    errors: Vec<String>,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
struct State {
    locals: Vec<(LocalId, i32)>,
    temps: Vec<(NodeId, i32)>,
    /// Every path out of the code just replayed was a jump, so this state
    /// never reaches the join it would otherwise be compared at.
    diverged: bool,
}

impl State {
    fn bump(&mut self, l: LocalId, by: i32) {
        match self.locals.iter_mut().find(|(k, _)| *k == l) {
            Some((_, v)) => *v += by,
            None => self.locals.push((l, by)),
        }
    }

    fn bump_temp(&mut self, n: NodeId, by: i32) {
        match self.temps.iter_mut().find(|(k, _)| *k == n) {
            Some((_, v)) => *v += by,
            None => self.temps.push((n, by)),
        }
    }

    fn normalize(&mut self) {
        self.locals.retain(|(_, v)| *v != 0);
        let _ = self.diverged;
        self.temps.retain(|(_, v)| *v != 0);
        self.locals.sort_by_key(|(l, _)| l.0);
        self.temps.sort_by_key(|(n, _)| n.0);
    }
}

impl Balance<'_> {
    fn counted_local(&mut self, l: LocalId) -> bool {
        let Some(local) = self.func.locals.get(l.index()) else { return false };
        let ty = local.ty;
        matches!(self.counted.counted(&ty), Answer::Yes)
    }

    fn child(&self, id: NodeId, k: usize) -> NodeId {
        let mut cur = id.0 + 1;
        for _ in 0..k {
            cur += self.sizes.get(cur as usize).copied().unwrap_or(1);
        }
        NodeId(cur)
    }

    fn sites(&mut self, id: NodeId, at: Position, st: &mut State) {
        if at == Position::After && self.suppress == Some(id) {
            return;
        }
        let ops: Vec<Site> = self.plan.at(id, at).copied().collect();
        for s in ops {
            let by = if s.op == RcOp::IncRef { 1 } else { -1 };
            match s.target {
                Target::Local(l) => st.bump(l, by),
                Target::Node(n) => st.bump_temp(n, by),
            }
        }
    }

    /// Runs the branches from one incoming state and requires that they
    /// agree — which is what "balanced along every path" means at a join.
    fn join(&mut self, branches: Vec<(&Expr, NodeId, Mode)>, st: &mut State) {
        let mut ends: Vec<State> = Vec::new();
        for (e, id, m) in branches {
            let mut copy = st.clone();
            self.walk(e, id, m, &mut copy);
            copy.normalize();
            ends.push(copy);
        }
        // A branch that jumped is checked against the loop header
        // instead, at the jump; it never arrives here.
        let arrivals: Vec<&State> = ends.iter().filter(|s| !s.diverged).collect();
        if let Some(first) = arrivals.first() {
            for other in arrivals.iter().skip(1) {
                if *other != *first {
                    self.errors.push(format!(
                        "branches disagree in {}: {first:?} vs {other:?}",
                        self.func.debug_name
                    ));
                }
            }
            *st = (*first).clone();
        } else if let Some(first) = ends.first() {
            // Everything jumped, so nothing falls through.
            *st = first.clone();
            st.diverged = true;
        }
    }

    fn walk(&mut self, e: &Expr, id: NodeId, mode: Mode, st: &mut State) {
        for l in self.fresh_at.get(&id).cloned().unwrap_or_default() {
            st.bump(l, 1);
        }
        self.sites(id, Position::Before, st);
        match &e.kind {
            ExprKind::Local(l) => {
                self.sites(id, Position::After, st);
                if mode == Mode::Own && self.counted_local(*l) {
                    st.bump(*l, -1);
                }
                return;
            }
            // Not descended into, exactly as the analysis does not: a
            // lambda's body is a scope of its own, and `middle::closures`
            // is what turns one into a function with a plan of its own.
            ExprKind::Lambda { .. } => {}
            ExprKind::Block { stmts, tail } => {
                let children = stmts.len() + usize::from(tail.is_some());
                for (k, s) in stmts.iter().enumerate() {
                    let sid = self.child(id, k);
                    match s {
                        Stmt::Let { pattern, value, .. } => {
                            let mut bound = Vec::new();
                            pattern.binds(&mut bound);
                            // `let _ = f(ctx);` binds nothing and so is a
                            // statement that discards its value: the scan
                            // reads it exactly as a `Stmt::Expr` and so
                            // does this.
                            if bound.is_empty() {
                                // `suppress` for the reason the bound case
                                // has it: the drop of what this statement
                                // discards is keyed on the value's own
                                // node, so it is applied once, here, and
                                // after the temporary it releases exists.
                                let held = self.suppress.replace(sid);
                                self.walk(value, sid, Mode::Borrow, st);
                                self.suppress = held;
                                let ty = value.ty;
                                if fresh(value)
                                    && matches!(self.counted.counted(&ty), Answer::Yes)
                                {
                                    st.bump_temp(sid, 1);
                                }
                                self.sites(sid, Position::After, st);
                                continue;
                            }
                            // The drop of a binding nothing reads is keyed
                            // on the value's node, and it happens *after*
                            // the binding exists.
                            let held = self.suppress.replace(sid);
                            self.walk(value, sid, Mode::Own, st);
                            self.suppress = held;
                            for b in bound {
                                if self.counted_local(b) {
                                    st.bump(b, 1);
                                }
                            }
                            // A binding nothing reads is dropped here.
                            self.sites(sid, Position::After, st);
                        }
                        Stmt::Expr(x) => {
                            self.walk(x, sid, Mode::Borrow, st);
                            let ty = x.ty;
                            if fresh(x) && matches!(self.counted.counted(&ty), Answer::Yes) {
                                st.bump_temp(sid, 1);
                            }
                            self.sites(sid, Position::After, st);
                        }
                    }
                }
                if let Some(t) = tail {
                    let tid = self.child(id, children.saturating_sub(1));
                    self.walk(t, tid, mode, st);
                }
            }
            ExprKind::If { cond, then, else_ } => {
                self.walk(cond, self.child(id, 0), Mode::Borrow, st);
                self.join(
                    vec![
                        (then, self.child(id, 1), mode),
                        (else_, self.child(id, 2), mode),
                    ],
                    st,
                );
            }
            ExprKind::Match { scrutinee, arms } => {
                let sid = self.child(id, 0);
                // The scrutinee's own mode is decided by the plan: a drop
                // of it at an arm entry is what "the match consumed it"
                // looks like from outside.
                let consumed = self.consumes_scrutinee(scrutinee);
                // The same promotion the scan makes for a compound scrutinee.
                let promoted = !consumed
                    && compound(scrutinee)
                    && matches!(
                        self.counted.counted(&scrutinee.ty.clone()),
                        Answer::Yes
                    );
                self.walk(
                    scrutinee,
                    sid,
                    if consumed || promoted { Mode::Own } else { Mode::Borrow },
                    st,
                );
                if promoted {
                    st.bump_temp(sid, 1);
                } else if !consumed
                    && fresh(scrutinee)
                    && matches!(self.counted.counted(&scrutinee.ty.clone()), Answer::Yes)
                {
                    // A scrutinee the match *built* is a temporary with a
                    // count and no name, exactly as a fresh argument of a
                    // `Continue` is: `Scan::match_` releases it before
                    // every back edge and after the arms. Crediting only
                    // the promoted case read `match (xs.get(at))` in a
                    // tail-recursive walk as one release too many.
                    st.bump_temp(sid, 1);
                }
                if consumed {
                    if let ExprKind::Local(l) = &scrutinee.kind {
                        // `Own` mode took the count; the arm's own decref
                        // is the one the plan wrote, so give it back here.
                        st.bump(*l, 1);
                    }
                }
                let mut k = 1usize;
                let mut branches: Vec<(&Expr, NodeId, Mode)> = Vec::new();
                for a in arms {
                    if a.guard.is_some() {
                        k += 1;
                    }
                    let bid = self.child(id, k);
                    let mut fresh_bound: Vec<LocalId> = Vec::new();
                    a.pattern.fresh_binds(&mut fresh_bound);
                    fresh_bound.retain(|b| self.counted_local(*b));
                    if !fresh_bound.is_empty() {
                        self.fresh_at.insert(bid, fresh_bound);
                    }
                    branches.push((&a.body, bid, mode));
                    k += 1;
                }
                self.join(branches, st);
            }
            ExprKind::Loop { entries } => {
                let branches: Vec<(&Expr, NodeId, Mode)> = entries
                    .iter()
                    .enumerate()
                    .map(|(k, x)| (x, self.child(id, k), mode))
                    .collect();
                self.join(branches, st);
            }
            ExprKind::Continue { func, args, .. } => {
                let row: Vec<ir::Ownership> = match func {
                    Some(f) => self.own.get(f.index()).cloned().unwrap_or_default(),
                    None => self.plan.params.clone(),
                };
                for (k, arg) in args.iter().enumerate() {
                    let aid = self.child(id, k);
                    let m = match row.get(k) {
                        Some(ir::Ownership::Borrow) => Mode::Borrow,
                        _ => Mode::Own,
                    };
                    self.walk(arg, aid, m, st);
                    if m == Mode::Borrow && fresh(arg) {
                        let ty = arg.ty;
                        if matches!(self.counted.counted(&ty), Answer::Yes) {
                            st.bump_temp(aid, 1);
                        }
                    }
                }
                // The jump installs the arguments in the loop's variables,
                // which are this function's parameters.
                if func.is_none() {
                    for (k, p) in self.func.params.iter().enumerate() {
                        if row.get(k).copied() == Some(ir::Ownership::Own)
                            && self.counted_local(*p)
                        {
                            st.bump(*p, 1);
                        }
                    }
                }
                self.sites(id, Position::After, st);
                let mut end = st.clone();
                end.normalize();
                // One traversal of the cycle proves the invariant: the
                // state at the back edge has to be the state the header
                // started in, or the next iteration starts richer or
                // poorer than this one did and the difference is a leak or
                // a double free per iteration.
                let want = if func.is_none() { self.header.clone() } else { State::default() };
                let mut want = want;
                want.normalize();
                if end.locals != want.locals || end.temps != want.temps {
                    self.errors.push(format!(
                        "the back edge in {} does not restore the header: {end:?} vs {want:?}",
                        self.func.debug_name
                    ));
                }
                st.diverged = true;
                return;
            }
            ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
                self.walk(lhs, self.child(id, 0), Mode::Borrow, st);
                // Both the taken and the skipped path.
                self.join(vec![(rhs, self.child(id, 1), Mode::Borrow)], st);
            }
            _ => {
                let kids = kids(e);
                let modes = child_modes(e, kids.len(), self.own);
                for (k, kid) in kids.iter().enumerate() {
                    let kid_id = self.child(id, k);
                    let m = modes.get(k).copied().unwrap_or(Mode::Borrow);
                    let counted = matches!(
                        self.counted.counted(&kid.ty.clone()),
                        Answer::Yes
                    );
                    // The same promotion `Scan::children` makes: a counted
                    // compound child is owned, and its value is a
                    // temporary the sites drop.
                    if m == Mode::Borrow && compound(kid) && counted {
                        self.walk(kid, kid_id, Mode::Own, st);
                        st.bump_temp(kid_id, 1);
                        continue;
                    }
                    self.walk(kid, kid_id, m, st);
                    if m == Mode::Borrow && fresh(kid) && counted {
                        st.bump_temp(kid_id, 1);
                    }
                }
            }
        }
        self.sites(id, Position::After, st);
        if mode == Mode::Own {
            // A value this node produced with a count of its own — a field
            // read in an owning position — is taken by whatever asked for
            // it, which is this node's parent.
            let taken: Vec<Site> = self
                .plan
                .at(id, Position::After)
                .filter(|s| s.op == RcOp::IncRef && s.target == Target::Node(id))
                .copied()
                .collect();
            for _ in taken {
                st.bump_temp(id, -1);
            }
        }
        for (_, v) in &st.locals {
            if *v < 0 {
                self.errors.push(format!("a count went negative in {}", self.func.debug_name));
                break;
            }
        }
    }

    /// Whether the plan says the match consumed its scrutinee: a `DecRef`
    /// of the scrutinee's local at an arm entry.
    fn consumes_scrutinee(&self, scrutinee: &Expr) -> bool {
        let ExprKind::Local(l) = &scrutinee.kind else { return false };
        self.plan
            .sites
            .iter()
            .any(|s| s.op == RcOp::DecRef && s.target == Target::Local(*l) && s.at == Position::Before)
    }
}

/// Every function in a program, checked.
fn check_balance(program: &Program) -> Vec<String> {
    let mut counted = Syntactic::new(program);
    let plan = analyze(program, &mut counted, &Options::default());
    let own: Vec<Vec<ir::Ownership>> =
        plan.funcs.iter().map(|f| f.params.clone()).collect();
    let mut errors = Vec::new();
    for (i, f) in program.funcs.iter().enumerate() {
        let Some(body) = f.body() else { continue };
        let Some(fp) = plan.funcs.get(i) else { continue };
        let mut sizes = Vec::new();
        subtree_sizes(body, &mut sizes);
        let mut b = Balance {
            func: f,
            plan: fp,
            sizes,
            counted: Syntactic::new(program),
            own: &own,
            suppress: None,
            header: State::default(),
            fresh_at: HashMap::default(),
            errors: Vec::new(),
        };
        let mut st = State::default();
        for (k, p) in f.params.iter().enumerate() {
            if fp.params.get(k).copied() == Some(ir::Ownership::Own) && b.counted_local(*p) {
                st.bump(*p, 1);
            }
        }
        st.normalize();
        b.header = st.clone();
        b.walk(body, NodeId(0), Mode::Own, &mut st);
        st.normalize();
        if st.diverged {
            // Every path jumped, and each jump was checked against the
            // header where it happened.
            errors.extend(b.errors);
            continue;
        }
        if !st.locals.is_empty() || !st.temps.is_empty() {
            b.errors.push(format!("{} ends holding {st:?}", f.debug_name));
        }
        errors.extend(b.errors);
    }
    errors
}

/// The shape MEMORY.md §5.2 is about: an owned scrutinee, a payload kept
/// past the match, a borrow across a call, and two branches that use
/// different values.
const TREE: &str = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;

enum Tree { Leaf, Node(Str, [Tree]) }

export fn label(t: Tree, other: Str): Str {
  match (t) {
.Leaf => other,
.Node(name, kids) => if (kids.length() > 0) { name } else { other },
  }
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let t = Tree.Node("root", [Tree.Leaf]);
  let _ = io.println(ctx, label(t, "none")).ignore();
  .Ok(())
}
"#;

const PROGRAM: &str = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;

struct P { name: Str, n: Int }

/// Reads and returns nothing of its argument: borrowed.
export fn size(p: P): Int {
  p.n
}

/// Stores its argument in a constructed value: owned.
export fn wrap(p: P): [P] {
  [p]
}

/// Uses one argument twice, which is where an increment comes from.
export fn twice(s: Str): [Str] {
  [s, s]
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let p = P { name: "a", n: 1 };
  let n = size(p);
  let xs = wrap(p);
  let ys = twice("b");
  let _ = io.println(ctx, "${n} ${xs.length()} ${ys.length()}").ignore();
  .Ok(())
}
"#;

/// The same, plus the two passes on the native branch that *change node
/// kinds*: `tail_calls` turns a tail-recursive body into a `Loop` of
/// `Continue`s, and `closures` turns a `Lambda` into a `Closure`. This is
/// the tree `middle::native` hands to `rc`, and the shapes below only exist
/// in it.
fn compile_native(src: &str) -> Program {
    let mut program = compile(src);
    crate::compiler::middle::tail_calls::rewrite(&mut program);
    crate::compiler::middle::closures::run(&mut program);
    program
}

/// A tail-recursive function churning an aggregate per iteration — the
/// shape the LLVM backend's live-block test leaked three blocks an
/// iteration on.
const CHURN: &str = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;

struct Row { name: Str, tags: [Str] }

export fn churn<C: Allocator>(ctx: C, n: Int, acc: [Str]): [Str] {
  if (n <= 0) {
acc
  } else {
let row = Row { name: "x", tags: ["a", "b"] };
let next = acc.push(ctx, row.name);
churn(ctx, n - 1, next)
  }
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let out = churn(ctx, 3, []);
  let _ = io.println(ctx, "${out.length()}").ignore();
  .Ok(())
}
"#;

fn loop_body(program: &Program, name: &str) -> FuncIdx {
    let f = find(program, name);
    let body = program.funcs.get(f.index()).and_then(|x| x.body());
    assert!(
        matches!(body.map(|b| &b.kind), Some(ExprKind::Loop { .. })),
        "{name} was expected to be a loop after `tail_calls`"
    );
    f
}

/// The numbering is defined as `typed::walk`'s pre-order, and a loop is
/// where it stopped being: `kids` did not descend into `Loop` or
/// `Continue`, so a whole loop body counted as one node and every site
/// keyed after it named the wrong expression. `lower` builds its own table
/// from [`preorder`], so the two agreed with each other and both were
/// wrong about the tree.
#[test]
fn preorder_agrees_across_a_loop() {
    let program = compile_native(CHURN);
    let mut loops = 0;
    for f in &program.funcs {
        let Some(body) = f.body() else { continue };
        if matches!(body.kind, ExprKind::Loop { .. }) {
            loops += 1;
        }
        let mut by_walk: Vec<String> = Vec::new();
        typed::walk(body, &mut |e| by_walk.push(format!("{:?}", std::ptr::from_ref(e))));
        let mut by_preorder: Vec<String> = Vec::new();
        preorder(body, &mut |_, e| by_preorder.push(format!("{:?}", std::ptr::from_ref(e))));
        assert_eq!(by_walk, by_preorder, "{} numbers differently", f.debug_name);
        let mut sizes = Vec::new();
        let total = subtree_sizes(body, &mut sizes);
        assert_eq!(total as usize, by_walk.len(), "{} sizes short", f.debug_name);
    }
    assert!(loops > 0, "the snippet has a tail-recursive function");
}

/// A tail-recursive drain: every iteration builds *both* of its loop
/// variables, and neither is the caller's to keep alive.
const DRAIN: &str = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/list" import * as list;

export fn drain<C: Allocator>(ctx: C, xs: [Int], acc: [Int]): [Int] {
  match (xs.first()) {
.Some(v) => drain(ctx, xs.drop(ctx, 1), acc.push(ctx, v)),
.None => acc,
  }
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let out = drain(ctx, [1, 2, 3, 4], []);
  let _ = io.println(ctx, "${out.length()}").ignore();
  .Ok(())
}
"#;

/// A loop variable a jump *rebuilds* is owned, and a borrowed parameter is
/// one the caller keeps alive across the whole call.
///
/// `xs` is read and never stored, so ownership inference called it
/// borrowed — and a borrowed argument that is a fresh value has nobody to
/// drop it, so `Scan::drop_temporary` put the drop before the back edge and
/// the next iteration read a freed list. `drain([1, 2, 3, 4], [])` answered
/// `[1, 2, 3, 0]` natively and `[1, 2, 3, 4]` on JavaScript.
#[test]
fn a_jump_owns_what_it_did_not_pass_through() {
    let program = compile_native(DRAIN);
    let i = loop_body(&program, "drain");
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());
    let fp = plan.func(i).expect("a plan");
    // `ctx` is handed straight through and stays the caller's; `xs` and
    // `acc` are rebuilt, so the loop variable takes the count.
    assert_eq!(fp.params.get(1).copied(), Some(ir::Ownership::Own));
    assert_eq!(fp.params.get(2).copied(), Some(ir::Ownership::Own));
    // Nothing the jump carries is dropped before the jump.
    let f = program.funcs.get(i.index()).expect("a function");
    let body = f.body().expect("a body");
    let mut sizes: Vec<u32> = Vec::new();
    subtree_sizes(body, &mut sizes);
    let mut carried: Vec<NodeId> = Vec::new();
    preorder(body, &mut |id, e| {
        let ExprKind::Continue { args, .. } = &e.kind else { return };
        let mut cur = id.0 + 1;
        for _ in 0..args.len() {
            carried.push(NodeId(cur));
            cur += sizes.get(cur as usize).copied().unwrap_or(1);
        }
    });
    assert!(!carried.is_empty(), "the snippet has a jump");
    for n in carried {
        assert!(
            !fp.sites
                .iter()
                .any(|s| s.op == RcOp::DecRef && s.target == Target::Node(n)),
            "n{} is carried into the next iteration and dropped before it",
            n.0
        );
    }
    assert_eq!(check_balance(&program), Vec::<String>::new());
}

/// The leak repro, as a property of the plan: what an iteration builds and
/// does not carry through the jump is dropped before the back edge, and the
/// counts at the back edge are the counts at the header.
#[test]
fn a_loop_drops_what_it_does_not_carry_before_the_back_edge() {
    let program = compile_native(CHURN);
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());
    let f = loop_body(&program, "churn");
    let fp = plan.func(f).expect("a plan");
    assert!(
        !fp.sites.is_empty(),
        "a loop body with an allocation per iteration has reference operations in it"
    );
    // `row` is built each iteration and only its `name` is carried on, so
    // the row itself dies inside the loop.
    let func = program.funcs.get(f.index()).expect("a function");
    let named: Vec<String> = fp
        .sites
        .iter()
        .filter_map(|s| match s.target {
            Target::Local(l) => func.locals.get(l.index()).map(|x| {
                format!("{} {}", if s.op == RcOp::IncRef { "inc" } else { "dec" }, x.name)
            }),
            Target::Node(_) => None,
        })
        .collect();
    assert!(named.iter().any(|x| x == "dec row"), "{named:?}");
    assert_eq!(check_balance(&program), Vec::<String>::new());
}

/// VALUE-MODEL.md §4.2: `..rest` binds a block the arm allocated, so the
/// arm drops it and takes no count out of the scrutinee — whether the
/// scrutinee is borrowed (`tell`) or consumed (`take`).
#[test]
fn a_rest_binding_is_dropped_and_never_increfed() {
    let src = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

export fn tell<C: Allocator>(ctx: C, xs: [Str]): Int {
  match (xs) {
[] => 0,
[_h, ..rest] => rest.length() + xs.length(),
  }
}

export fn take<C: Allocator>(ctx: C, xs: [Str]): [Str] {
  match (xs) {
[] => [],
[_h, ..rest] => rest.push(ctx, "z"),
  }
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "${tell(ctx, ["a", "b"])} ${take(ctx, ["a", "b"]).length()}").ignore();
  .Ok(())
}
"#;
    let program = compile_native(src);
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());
    for name in ["tell", "take"] {
        let i = find(&program, name);
        let fp = plan.func(i).expect("a plan");
        let func = program.funcs.get(i.index()).expect("a function");
        let rest = func
            .locals
            .iter()
            .position(|l| l.name == "rest")
            .map(|k| LocalId(k as u32))
            .unwrap_or_else(|| panic!("{name} binds a local named rest"));
        let ops: Vec<RcOp> = fp
            .sites
            .iter()
            .filter(|s| s.target == Target::Local(rest))
            .map(|s| s.op)
            .collect();
        assert_eq!(ops, vec![RcOp::DecRef], "{name}: {ops:?}");
    }
    assert_eq!(check_balance(&program), Vec::<String>::new());
}

/// The plan-order invariant, at the key the `66cb95fb` crash was planned at.
///
/// The three sites below are what `lower`'s site loop was handed for the
/// four-line reproduction — a struct holding a counted list, a field of it
/// bound to a `let` nothing reads — printed off the pre-fix planner and
/// recorded in `reports/llvm-parallel-listen-fix.md` §3, in the order they
/// were pushed. Two of the three name one block: the drop of the unread
/// binding is keyed on the `Local`, and the increment that gave that
/// binding its count is keyed on the `Node` that projected it.
///
/// The planner no longer pushes them in that order, and this test does,
/// which is the point of it. [`order_sites`] is what makes the shape
/// impossible rather than merely absent: it is asked to sort the bad order
/// and the result has to be sound. A planner that reintroduces the push
/// order — a new statement form, a new `Target` kind — cannot reintroduce
/// the bug through it.
#[test]
fn every_increment_at_a_key_runs_before_every_decrement() {
    let site = |op, target| Site { node: NodeId(148), at: Position::After, op, target };
    let mut sites = vec![
        site(RcOp::DecRef, Target::Local(LocalId(65))),
        site(RcOp::IncRef, Target::Node(NodeId(148))),
        site(RcOp::DecRef, Target::Local(LocalId(54))),
    ];
    order_sites(&mut sites);
    // What `lower` does three lines before it emits: both target kinds
    // resolve to a value, and these two resolve to the same one.
    let value = |t: Target| match t {
        Target::Local(LocalId(65)) | Target::Node(NodeId(148)) => 104u32,
        _ => 101,
    };
    let ops: Vec<(RcOp, u32)> = sites.iter().map(|s| (s.op, value(s.target))).collect();
    // Not a vacuous key: one value really is both released and retained at
    // it, so the ordering is the whole of what keeps it sound.
    assert_eq!(ops.iter().filter(|(_, v)| *v == 104).count(), 2);
    assert_eq!(
        release_then_retain(&ops),
        None,
        "the drop of the unread binding runs before the increment that gave it its \
         count — a free, and then a write through the freed header: {ops:?}"
    );
}

/// A binding nothing reads is dropped **after** the operations that gave it
/// its count, not before them.
///
/// `let x = <init>;` scans the initializer at the *statement's own node*,
/// so the initializer's sites and the drop of an unread `x` land at the
/// same `(node, After)` key — and a plan's sites run in the order they were
/// pushed. Pushing the drop first made the pair *release then retain*: on a
/// value whose count was one, the release frees the block and the retain
/// then writes into a header the allocator is already using as free-list
/// storage. The crash is an unrelated allocation later, which is what made
/// it look like a backend fault for a wave
/// (`reports/llvm-parallel-listen-fix.md`).
///
/// Both shapes that put an `incref` at that key are here: a **projection**
/// out of an aggregate (`Scan::projected`), and a **second read** of a
/// local something after it still uses (`ExprKind::Local` under
/// `Mode::Own`). Neither may be preceded at its own key by the drop.
#[test]
fn an_unread_binding_is_dropped_after_its_own_incref() {
    let src = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;

struct Pair { n: Int, tags: [Str] }

/// The projection shape: `stale` is words copied out of `pair`, and nothing
/// reads it.
export fn projected(pair: Pair): Int {
  let stale = pair.tags;
  pair.n
}

/// The alias shape: `stale` is a second name for `tags`, which is read again
/// after it. Returning `tags` makes it owned: a borrowed one binds `stale`
/// with no count at all.
export fn aliased(tags: [Str]): [Str] {
  let stale = tags;
  tags
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let p = Pair { n: 1, tags: ["a"] };
  let _ = io.println(ctx, "${projected(p)} ${aliased(["b"]).length()}").ignore();
  .Ok(())
}
"#;
    let program = compile_native(src);
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());
    for name in ["projected", "aliased"] {
        let i = find(&program, name);
        let fp = plan.func(i).expect("a plan");
        let func = program.funcs.get(i.index()).expect("a function");
        let stale = func
            .locals
            .iter()
            .position(|l| l.name == "stale")
            .map(|k| LocalId(k as u32))
            .unwrap_or_else(|| panic!("{name} binds a local named stale"));
        let drop = fp
            .sites
            .iter()
            .position(|s| s.target == Target::Local(stale) && s.op == RcOp::DecRef)
            .unwrap_or_else(|| panic!("{name}: the unread binding is never dropped"));
        let site = fp.sites[drop];
        // The whole claim: at the drop's own key, an increment runs first.
        // A key with no increment at all would pass this vacuously, so the
        // increment is asserted to exist as well.
        let increments: Vec<usize> = fp
            .sites
            .iter()
            .enumerate()
            .filter(|(_, s)| s.node == site.node && s.at == site.at && s.op == RcOp::IncRef)
            .map(|(k, _)| k)
            .collect();
        assert!(
            !increments.is_empty(),
            "{name}: nothing gives the binding a count, so the drop has nothing to give back"
        );
        assert!(
            increments.iter().all(|k| *k < drop),
            "{name}: the drop at {drop} runs before the increments at {increments:?} \
             on node {:?} — release then retain, which frees the block and then \
             writes through the freed header",
            site.node
        );
    }
    assert_eq!(check_balance(&program), Vec::<String>::new());
}

/// A merged mutually recursive group: one `Loop` with an entry per member,
/// and a `Continue` that names the function it re-enters.
#[test]
fn a_merged_group_balances_at_every_entry() {
    let src = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;

export fn even(n: Int, s: Str, t: Str): Str {
  if (n <= 0) { s } else { odd(n - 1, t, s) }
}

export fn odd(n: Int, s: Str, t: Str): Str {
  if (n <= 0) { t } else { even(n - 1, s, t) }
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, even(4, "a", "b")).ignore();
  .Ok(())
}
"#;
    let program = compile_native(src);
    let merged = program
        .funcs
        .iter()
        .filter_map(|f| f.body())
        .filter_map(|b| match &b.kind {
            ExprKind::Loop { entries } => Some(entries.len()),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    assert!(merged >= 2, "the two functions were merged into one loop");
    assert_eq!(check_balance(&program), Vec::<String>::new());
}

/// A closure built inside a loop: the environment takes a count of every
/// value it captures, once per iteration, and gives it back.
#[test]
fn a_closure_in_a_loop_captures_by_incrementing() {
    let src = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;

export fn tag<C: Allocator>(ctx: C, n: Int, prefix: Str, acc: [Str]): [Str] {
  if (n <= 0) {
acc
  } else {
let named = acc.map(ctx, fn(x) => prefix);
tag(ctx, n - 1, prefix, named)
  }
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let out = tag(ctx, 2, "p", ["a"]);
  let _ = io.println(ctx, "${out.length()}").ignore();
  .Ok(())
}
"#;
    let program = compile_native(src);
    let has_closure = program.funcs.iter().filter_map(|f| f.body()).any(|b| {
        let mut found = false;
        typed::walk(b, &mut |e| {
            if matches!(e.kind, ExprKind::Closure { .. }) {
                found = true;
            }
        });
        found
    });
    assert!(has_closure, "`closures` lifted the lambda");
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());
    let f = loop_body(&program, "tag");
    let func = program.funcs.get(f.index()).expect("a function");
    let fp = plan.func(f).expect("a plan");
    // The environment captures `prefix`, which the next iteration also
    // needs, so the capture increments rather than transfers.
    let incs: Vec<String> = fp
        .sites
        .iter()
        .filter(|s| s.op == RcOp::IncRef)
        .filter_map(|s| match s.target {
            Target::Local(l) => func.locals.get(l.index()).map(|x| x.name.to_string()),
            Target::Node(_) => None,
        })
        .collect();
    assert!(incs.iter().any(|x| x == "prefix"), "{incs:?}");
    assert_eq!(check_balance(&program), Vec::<String>::new());
}

/// A tail call that reads the value being matched on runs **before** that
/// value is released.
///
/// `ui/node`'s `nodeLines` is the shape: a `match` on a node's one field,
/// arms that jump back into the walk, and one of them —
/// `.Computed(build) => nodeLines(ctx, state, build(scope), depth)` — whose
/// jump argument calls a closure that lives inside the matched node. The
/// root is deliberately held alive across the arms ([`Scan::match_`]'s
/// `kept`), but an arm ending in a `Continue` is scanned against an empty
/// liveness, so it reported the root as dead where an arm that fell through
/// reported it live, and [`Scan::balance`] settled that difference with a
/// drop at the jumping arm's **entry**. It freed the closure's environment
/// before the closure ran, and the retain inside the closure then wrote
/// through a header the allocator was already using as free-list storage —
/// a `SIGSEGV` inside `buri_rt_alloc`, arbitrarily far from the mistake.
/// Issue #65.
///
/// So: no drop of the root at an arm's entry, and one at each back edge's
/// own key, which is past every argument that reads it.
#[test]
fn a_jumping_arm_drops_the_matched_value_after_its_arguments() {
    let src = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/str" import * as str;

enum Held { Ready(Str), Deferred(fn(Int) => Held) }

struct Box(Held);

/// `nodeLines`'s shape: a match on the value's one field, and an arm that calls
/// a closure out of the payload and jumps back with what it answered.
export fn forced<C: Allocator>(ctx: C, held: Box, depth: Int): Str {
  match (held.0) {
.Ready(s) => str.format(ctx, "${s}/${depth}"),
.Deferred(build) => forced(ctx, Box(build(depth)), depth + 1),
  }
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let name = str.format(ctx, "leaf");
  let held = Box(.Deferred(fn(_i) => .Ready(name)));
  let _ = io.println(ctx, forced(ctx, held, 0)).ignore();
  .Ok(())
}
"#;
    let program = compile_native(src);
    let i = loop_body(&program, "forced");
    let func = program.funcs.get(i.index()).expect("a function");
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());
    let fp = plan.func(i).expect("a plan");
    let held = func
        .locals
        .iter()
        .position(|l| l.name == "held")
        .map(|k| LocalId(k as u32))
        .expect("`forced` takes a parameter named held");
    let drops: Vec<(Position, NodeId)> = fp
        .sites
        .iter()
        .filter(|s| s.op == RcOp::DecRef && s.target == Target::Local(held))
        .map(|s| (s.at, s.node))
        .collect();
    assert!(!drops.is_empty(), "the matched value is released somewhere");
    // The whole claim: never at an arm's entry. A `Before` drop of the root
    // runs ahead of the arm that reads the payload pointing into it.
    assert!(
        drops.iter().all(|(at, _)| *at == Position::After),
        "the matched value is dropped at an arm's entry: {drops:?}"
    );
    assert_eq!(check_balance(&program), Vec::<String>::new());
}

/// The numbering the plan is keyed by is `typed::walk`'s, node for node.
#[test]
fn preorder_matches_typed_walk() {
    let program = compile(PROGRAM);
    for f in &program.funcs {
        let Some(body) = f.body() else { continue };
        let mut by_walk: Vec<String> = Vec::new();
        typed::walk(body, &mut |e| by_walk.push(format!("{:?}", std::ptr::from_ref(e))));
        let mut by_preorder: Vec<String> = Vec::new();
        let mut ids: Vec<u32> = Vec::new();
        preorder(body, &mut |id, e| {
            ids.push(id.0);
            by_preorder.push(format!("{:?}", std::ptr::from_ref(e)));
        });
        assert_eq!(by_walk, by_preorder, "{} numbers differently", f.debug_name);
        assert_eq!(ids, (0..ids.len() as u32).collect::<Vec<u32>>());
        // And the subtree sizes agree with the node count.
        let mut sizes = Vec::new();
        let total = subtree_sizes(body, &mut sizes);
        assert_eq!(total as usize, by_walk.len());
        assert_eq!(sizes.len(), by_walk.len());
    }
}

/// MEMORY.md §5.2's rule, on the three shapes it names.
#[test]
fn a_parameter_is_borrowed_unless_the_body_takes_it() {
    let program = compile(PROGRAM);
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());
    let borrowed = plan.func(find(&program, "size")).expect("a plan").params.clone();
    assert_eq!(borrowed, vec![ir::Ownership::Borrow], "reading a field borrows");
    let owned = plan.func(find(&program, "wrap")).expect("a plan").params.clone();
    assert_eq!(owned, vec![ir::Ownership::Own], "storing takes the count");
}

/// A value used twice is incremented once: the second use is the one that
/// transfers.
#[test]
fn a_second_use_is_an_increment() {
    let program = compile(PROGRAM);
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());
    let twice = plan.func(find(&program, "twice")).expect("a plan");
    let incs = twice.sites.iter().filter(|s| s.op == RcOp::IncRef).count();
    let decs = twice.sites.iter().filter(|s| s.op == RcOp::DecRef).count();
    assert_eq!((incs, decs), (1, 0), "{:?}", twice.sites);
}

/// **A capture is read for a second time however dead it looks**, and a
/// lambda's own parameter is not ([`Scan::enter_lambda`]).
///
/// Under [`sharing`] a lambda's body is scanned inside its enclosing
/// function, because nothing has lifted it into a function of its own yet.
/// The enclosing scope's liveness then says `xs` is dead after the closure
/// is *built* — which is true of the enclosing scope and says nothing
/// about the closure, whose body runs once per element. Taking the count
/// on that reading is what let `$list_slice` truncate `xs` in place on the
/// first call, so `mapCtx(fn(c, i) => xs.slice(c, 0, i).len())` answered
/// `0, 0, 0`.
///
/// The other half is what the fix must not cost: `acc` is the fold's own
/// parameter, a fresh value on every call, and it stays owned — or growing
/// a list in a loop copies it once per iteration, which is
/// `language::sharing::growing_a_list_in_a_loop_is_linear`.
#[test]
fn a_capture_is_marked_and_a_lambda_parameter_is_not() {
    let program = compile(
        r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let xs = list.range(ctx, 0, 3)
.foldCtx(ctx, fn(c, acc: [Int], i) => acc.push(c, i), list.empty());
  let lengths = list.range(ctx, 0, 3).mapCtx(ctx, fn(c, i) => xs.slice(c, 0, i).length());
  io.println(ctx, str.format(ctx, "${lengths.length()}")).mapErr(fn(_e) => "no")
}
"#,
    );
    let idx = find(&program, "main");
    let func = program.funcs.get(idx.index()).expect("a function");
    let plan = sharing(&program);
    let marked: Vec<&str> = plan
        .func(idx)
        .expect("a plan")
        .sites
        .iter()
        .filter(|s| s.op == RcOp::IncRef)
        .filter_map(|s| match s.target {
            Target::Local(l) => func.locals.get(l.index()).map(|x| x.name.as_str()),
            Target::Node(_) => None,
        })
        .collect();
    assert!(marked.contains(&"xs"), "the capture was spent by the closure: {marked:?}");
    assert!(!marked.contains(&"acc"), "the fold's accumulator stopped writing through");
}

/// The whole program's counts balance, on every path.
#[test]
fn every_count_balances_on_every_path() {
    let program = compile(PROGRAM);
    assert_eq!(check_balance(&program), Vec::<String>::new());
}

/// Including the shapes that make it hard: a branch that uses a value the
/// other branch does not, a match that consumes what it matched, and a
/// short-circuit whose right operand may not run.
#[test]
fn branches_and_short_circuits_balance_too() {
    let program = compile(TREE);
    assert_eq!(check_balance(&program), Vec::<String>::new());
}

/// A chain of short circuits costs one scan per operand, not one per path
/// through them.
///
/// [`Scan::short_circuit`] used to scan its right operand **twice** — a
/// probe whose sites were thrown away, then the real scan — and a nested
/// `&&` inside that operand doubled again, so `n` links cost 2ⁿ scans.
/// `middle/derives.rs`'s `eq_fields` right-nests exactly one link per field
/// and says so in its own doc comment, which is how
/// `cli/tests/conformance/lib/proto/test/binary.buri` came to take minutes
/// to compile on the native path.
///
/// Sixty links is 10¹⁸ scans if the probe is ever put back, so this test
/// does not fail slowly: it does not finish. That is the point of the
/// number — a chain short enough to fail *quickly* would not be a
/// regression test for an exponential.
#[test]
fn a_chain_of_short_circuits_is_scanned_once_per_operand() {
    const LINKS: usize = 60;
    let mut chain = format!("(a{n} == b{n})", n = LINKS - 1);
    for i in (0..LINKS - 1).rev() {
        chain = format!("((a{i} == b{i}) && {chain})");
    }
    let params: Vec<String> = (0..LINKS).map(|i| format!("a{i}: Str, b{i}: Str")).collect();
    let args: Vec<String> = (0..LINKS).map(|i| format!("\"x{i}\", \"x{i}\"")).collect();
    let src = format!(
        r#"
from "platform/effect" import {{ Allocator, Stdout }};
from "node" import {{ NodeHost }};
from "core/io" import * as io;

export fn same({params}): Bool {{
  {chain}
}}

export fn main(host: NodeHost): Result<(), Str> {{
  let ctx = context {{ Allocator: host.alloc, Stdout: host.stdout }};
  let _ = io.println(ctx, "${{same({args})}}").ignore();
  .Ok(())
}}
"#,
        params = params.join(", "),
        args = args.join(", "),
    );
    let program = compile(&src);
    assert_eq!(check_balance(&program), Vec::<String>::new());
}

/// A scrutinee a short circuit is keeping alive is **not** consumed by the
/// `match` that reads it, and its payload is a borrowed view.
///
/// The deferral is what decides this: the right operand holds `o`'s last
/// use, so `short_circuit` keeps `o` live across the whole expression and
/// drops it afterwards — and a `match` whose scrutinee is still live after
/// it takes no count out of it ([`Scan::match_`]'s `token`).
///
/// The discarded probe scan used to decide it the *other* way and leave the
/// decision behind: it ran against the liveness *before* the deferral, so
/// its `match` did own the scrutinee, and `owns` put the payload binding
/// into [`Scan::owned`] — where the real scan then found it. The result was
/// a `DecRef` of `s` with no `IncRef` anywhere, against a count `o`'s own
/// drop was already going to release: one block, released twice.
#[test]
fn a_deferred_scrutinee_is_not_consumed_by_the_match_that_reads_it() {
    const SRC: &str = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let o: Option<Str> = .Some("s".concat(ctx, "x"));
  let flag = 1 < 2;
  let ok = flag && match (o) {
.Some(s) => s.length() > 0,
.None => false,
  };
  let _ = io.println(ctx, "${ok}").ignore();
  .Ok(())
}
"#;
    let program = compile(SRC);
    let i = find(&program, "main");
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());
    let fp = plan.func(i).expect("a plan");
    let sites = named_sites(&program, i, fp);
    assert_eq!(check_balance(&program), Vec::<String>::new(), "{sites:?}");
    assert!(
        !sites.iter().any(|s| s.starts_with("dec s ")),
        "`s` is a view into `o`, which the deferral drops: {sites:?}"
    );
    assert!(
        sites.iter().any(|s| s.starts_with("dec o ")),
        "`o` itself is still dropped: {sites:?}"
    );
}

/// One plan's sites as `"<op> <what> <before|after> n<node>"`, in plan
/// order — a form an expectation can be written in.
fn named_sites(program: &Program, i: FuncIdx, fp: &FuncPlan) -> Vec<String> {
    let f = program.funcs.get(i.index()).expect("a function");
    fp.sites
        .iter()
        .map(|s| {
            let what = match s.target {
                Target::Local(l) => f
                    .locals
                    .get(l.index())
                    .map(|x| x.name.to_string())
                    .unwrap_or_else(|| format!("l{}", l.0)),
                Target::Node(n) => format!("n{}", n.0),
            };
            let op = if s.op == RcOp::IncRef { "inc" } else { "dec" };
            let at = if s.at == Position::Before { "before" } else { "after" };
            format!("{op} {what} {at} n{}", s.node.0)
        })
        .collect()
}

/// The placement itself, on the shape the design argues about: the payloads
/// that survive the arm are incremented out of the value, the value is
/// dropped there, a borrow across a call is dropped after the call, and the
/// branch that does not use a value drops it on entry.
#[test]
fn a_consuming_match_dups_what_it_keeps_and_drops_what_it_matched() {
    let program = compile(TREE);
    let i = find(&program, "label");
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());
    let fp = plan.func(i).expect("a plan");
    assert_eq!(fp.params, vec![ir::Ownership::Own, ir::Ownership::Own]);
    assert_eq!(
        named_sites(&program, i, fp),
        vec![
            // `.Leaf => other`: the matched value dies at the arm entry.
            "dec t before n3",
            // `.Node(name, kids)`: what the arm keeps is incremented out of
            // it first, and then it dies.
            "inc name before n4",
            "inc kids before n4",
            "dec t before n4",
            // `kids.len()` borrows, so the drop lands after the call.
            "dec kids after n6",
            // The branch that returns `name` has no use for `other`.
            "dec other before n9",
            // And the branch that returns `other` has none for `name`.
            "dec name before n11",
        ]
    );
}

/// A projection is words copied out of its base, so the base has to reach
/// the construct that reads them.
///
/// `two(one(p.a), p.b)`: `p`'s last mention is `p.b`, which is evaluated
/// **after** `one(p.a)`. The drop belongs after `two`, and it used to land
/// after `one` — early enough that `p.b` read a block `p`'s drop glue had
/// already freed. `sortcheck/cmd/q2` is the same three lines with a
/// `concat` in place of `two`, and it printed `[0, 0, 0]`.
#[test]
fn a_projection_outlives_the_siblings_evaluated_after_it() {
    const SRC: &str = r#"
struct Pair { a: [Int], b: [Int] }

fn one(xs: [Int]): Int { xs.length() }
fn two(n: Int, ys: [Int]): Int { n + ys.length() }

from "node" import { NodeHost };

export fn main(host: NodeHost): Result<(), Str> {
  let p = Pair { a: [1], b: [2, 3] };
  let n = two(one(p.a), p.b);
  .Ok(())
}
"#;
    let program = compile(SRC);
    let i = find(&program, "main");
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());
    let fp = plan.func(i).expect("a plan");
    // `n7` is the call to `two`; `n8` is the call to `one` inside it.
    assert_eq!(named_sites(&program, i, fp), vec!["dec p after n7"]);
    assert_eq!(check_balance(&program), Vec::<String>::new());
}

/// A consumed scrutinee is disposed of **once per arm**, and the arm that
/// still reads it does so at its own last use rather than at its entry.
///
/// This is `core/testing/assert`'s `some` written out: the `.None` arm
/// hands the `Option` itself to the failure report, which is what
/// `assert.ok` does not do with its `Result` — and why the two behaved
/// differently. The `.Some` arm used to carry two drops (the match's own,
/// and one `Scan::balance` added because the other arm put the scrutinee in
/// the union), so `assert.some` freed the payload it was answering with.
#[test]
fn a_consumed_scrutinee_is_dropped_once_on_every_arm() {
    const SRC: &str = r#"
fn label(what: Str, got: Option<Str>): Str { what }

fn make(s: Str): Option<Str> { .Some(s) }

export fn some(o: Option<Str>): Str {
  match (o) {
.Some(v) => v,
.None => label("some", o),
  }
}

from "node" import { NodeHost };

export fn main(host: NodeHost): Result<(), Str> {
  let _ = some(make("x"));
  .Ok(())
}
"#;
    let program = compile(SRC);
    let i = find(&program, "some");
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());
    let fp = plan.func(i).expect("a plan");
    assert_eq!(fp.params, vec![ir::Ownership::Own]);
    assert_eq!(
        named_sites(&program, i, fp),
        vec![
            // `.Some(v) => v`: the payload is increfed out and the value
            // dies at the entry, because the arm is done with it.
            "inc v before n3",
            "dec o before n3",
            // `.None => label("some", o)`: `label` borrows, so the drop is
            // after the call — and there is no second one at the entry.
            "dec o after n4",
        ]
    );
    assert_eq!(check_balance(&program), Vec::<String>::new());
}

/// A value the match itself built has no binding and no owner.
///
/// The arms take a count for whatever they keep out of it — `owns` is
/// false, so every payload use is an increment — and after the arms nothing
/// names it. `match (q.pop(ctx))` leaked the `Option` and the two lists the
/// queue it answered still pointed at, once per iteration of a drain.
#[test]
fn a_scrutinee_the_match_built_is_dropped_after_the_arms() {
    const SRC: &str = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/list" import * as list;

fn two<C: Allocator>(ctx: C, n: Int): ([Int], [Int]) {
  (list.range(ctx, 0, n), list.range(ctx, 0, n + 1))
}

export fn sizes<C: Allocator>(ctx: C, n: Int): Int {
  match (two(ctx, n)) {
(a, b) => a.length() + b.length(),
  }
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "${sizes(ctx, 2)}").ignore();
  .Ok(())
}
"#;
    let program = compile(SRC);
    let i = find(&program, "sizes");
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());
    let fp = plan.func(i).expect("a plan");
    // `n1` is the `match`, `n2` its scrutinee, and the drop is after the
    // arms have read what they keep out of it.
    assert_eq!(named_sites(&program, i, fp), vec!["dec n2 after n1"]);
}

/// A fresh value reached through a branch is still fresh.
///
/// `middle::inline` replaces a call with the callee's body, so the thing a
/// borrowing construct is handed stops being an `ExprKind::CallFn` — and
/// `fresh` said no, and nothing dropped it. `"[${show(ctx, xs)}]"` was the
/// program: one call, so the inliner pasted it in, and the string leaked.
#[test]
fn a_fresh_value_behind_a_branch_is_still_dropped() {
    const SRC: &str = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/str" import * as str;

fn size(s: Str): Int { s.length() }

export fn shown<C: Allocator>(ctx: C, n: Int): Int {
  size(if (n > 0) { str.format(ctx, "v${n}") } else { str.format(ctx, "z") })
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "${shown(ctx, 2)}").ignore();
  .Ok(())
}
"#;
    let program = compile(SRC);
    let i = find(&program, "shown");
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());
    let fp = plan.func(i).expect("a plan");
    // `n1` is the call to `size`, `n2` the `if` it borrows. The rest are
    // the two templates' own holes.
    assert!(
        named_sites(&program, i, fp).contains(&String::from("dec n2 after n1")),
        "{:?}",
        named_sites(&program, i, fp)
    );
}

/// A bigger program, so that the balance is checked over the standard
/// library the snippet reaches rather than only over what the snippet
/// writes: lists, strings, options, closures and a fold.
#[test]
fn the_standard_library_balances_too() {
    let src = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/list" import * as list;

struct Row { name: Str, tags: [Str] }

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let rows = [
Row { name: "a", tags: ["x", "y"] },
Row { name: "b", tags: [] },
  ];
  let names = rows.map(ctx, fn(r) => r.name);
  let joined = names.join(ctx, ", ");
  let first: Option<Row> = rows.first();
  let shown = match (first) {
.Some(r) => r.name,
.None => "none",
  };
  let total = rows.fold(fn(acc: Int, r: Row) => acc + r.tags.length(), 0);
  let _ = io.println(ctx, "${joined} ${shown} ${total}").ignore();
  .Ok(())
}
"#;
    let program = compile(src);
    assert!(program.funcs.len() > 5, "the snippet reaches the library");
    assert_eq!(check_balance(&program), Vec::<String>::new());
}


/// A match that consumes its scrutinee pairs the dying value with the
/// construction in the arm — MEMORY.md §5.3's reuse, in its analysis form.
#[test]
fn a_dying_value_is_paired_with_a_construction() {
    let src = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;

enum Pair { One(Str), Two(Str, Str) }

export fn swap(p: Pair): Pair {
  match (p) {
.One(a) => .One(a),
.Two(a, b) => .Two(b, a),
  }
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let p = Pair.Two("a", "b");
  let q = swap(p);
  let _ = io.println(ctx, match (q) { .One(a) => a, .Two(a, _) => a }).ignore();
  .Ok(())
}
"#;
    let program = compile(src);
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());
    let swap = plan.func(find(&program, "swap")).expect("a plan");
    assert_eq!(swap.params, vec![ir::Ownership::Own], "the match consumes it");
    assert_eq!(swap.reuse.len(), 2, "one per arm: {:?}", swap.reuse);
    assert!(swap.reuse.iter().any(|r| r.fields == 2), "{:?}", swap.reuse);
    // Turning the pairing off leaves everything else where it was.
    let mut counted = Syntactic::new(&program);
    let without = analyze(&program, &mut counted, &Options { reuse: false, sharing: false });
    let swap_off = without.func(find(&program, "swap")).expect("a plan");
    assert!(swap_off.reuse.is_empty());
    assert_eq!(swap_off.sites, swap.sites);
}

/// A scrutinee that is **still live after the match** is not dying, so
/// there is nothing to reuse and nothing is paired.
///
/// This is the edge that makes reuse sound rather than fast: writing into
/// a cell something else still reads is the one way MEMORY.md §5.3's
/// mutation becomes observable, and the guard against it is the same
/// `!live.contains(l)` that decides the drop.
#[test]
fn a_value_used_after_the_construction_is_not_paired() {
    let src = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;

enum Pair { One(Str), Two(Str, Str) }

export fn first(p: Pair): Str {
  match (p) {
.One(a) => a,
.Two(a, _b) => a,
  }
}

export fn swapped(p: Pair, other: Pair): Pair {
  let q: Pair = match (p) {
.One(a) => .One(a),
.Two(a, b) => .Two(b, a),
  };
  // `p` is read *after* the construction, so the match above did not consume
  // it and its cell is not dying at the point the arm built a new one.
  let n = match (p) { .One(_a) => 1, .Two(_a, _b) => 2 };
  if (n == 1) { q } else { other }
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, first(swapped(Pair.Two("a", "b"), Pair.One("c")))).ignore();
  .Ok(())
}
"#;
    let program = compile(src);
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());
    let swapped = plan.func(find(&program, "swapped")).expect("a plan");
    assert!(
        swapped.reuse.is_empty(),
        "a scrutinee read after the arms was paired anyway: {:?}",
        swapped.reuse
    );
}

/// An arm whose body is **not a construction** pairs nothing, and an arm
/// whose construction has a different field count is recorded with *its
/// own* count rather than the scrutinee's.
///
/// [`Reuse::fields`] is the shape half of MEMORY.md §5.3's condition, and
/// `lower` compares size classes with it. A pairing that reported the
/// wrong count would be a write into a block too small for it, so the
/// number is asserted per arm rather than in aggregate.
#[test]
fn only_a_construction_pairs_and_it_carries_its_own_shape() {
    let src = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;

enum Shape { Nil, One(Str), Two(Str, Str) }

export fn reshape(s: Shape, fallback: Str): Shape {
  match (s) {
.Nil => .Nil,
// A construction of one field, out of a scrutinee whose live variant
// carries one.
.One(a) => .One(a),
// A construction of *two* fields out of the same scrutinee type.
.Two(a, b) => .Two(b, a),
  }
}

export fn pick(s: Shape, d: Str): Str {
  match (s) {
// Not a construction at all: every arm answers a binding.
.Nil => d,
.One(a) => a,
.Two(a, _b) => a,
  }
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let a = reshape(Shape.Two("a", "b"), "z");
  let _ = io.println(ctx, pick(a, "z")).ignore();
  .Ok(())
}
"#;
    let program = compile(src);
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());
    let reshape = plan.func(find(&program, "reshape")).expect("a plan");
    let mut shapes: Vec<usize> = reshape.reuse.iter().map(|r| r.fields).collect();
    shapes.sort_unstable();
    assert_eq!(
        shapes,
        vec![0, 1, 2],
        "each arm pairs with its own field count: {:?}",
        reshape.reuse
    );
    // Every pairing names the scrutinee and no other local.
    let scrutinee = reshape.reuse.first().map(|r| r.token);
    assert!(
        reshape.reuse.iter().all(|r| Some(r.token) == scrutinee),
        "a pairing named something other than the dying scrutinee: {:?}",
        reshape.reuse
    );
    // `pick` consumes its scrutinee just as `reshape` does, and pairs
    // nothing: no arm of it builds anything, so there is no allocation for
    // the dying cell to become.
    let pick = plan.func(find(&program, "pick")).expect("a plan");
    assert!(
        pick.reuse.is_empty(),
        "an arm that is not a construction was paired: {:?}",
        pick.reuse
    );
}

/// The purity column, which is the other half of what `ir::Facts` wants.
#[test]
fn purity_is_a_fixpoint_over_the_call_graph() {
    let src = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;

export fn double(n: Int): Int { n * 2 }

export fn quadruple(n: Int): Int { double(double(n)) }

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "${quadruple(2)}").ignore();
  .Ok(())
}
"#;
    let program = compile(src);
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());
    assert_eq!(
        plan.func(find(&program, "double")).map(|f| f.purity),
        Some(ir::Purity::Pure)
    );
    assert_eq!(
        plan.func(find(&program, "quadruple")).map(|f| f.purity),
        Some(ir::Purity::Pure),
        "purity propagates through a call"
    );
    assert_eq!(
        plan.func(find(&program, "main")).map(|f| f.purity),
        Some(ir::Purity::Effectful),
        "printing is not pure"
    );
    assert_eq!(plan.func(find(&program, "double")).map(|f| f.can_abort), Some(false));
}

/// A type the classifier cannot answer for carries no operations and is named,
/// which is the difference between a leak and a wrong answer.
#[test]
fn what_the_classifier_cannot_answer_is_recorded_rather_than_guessed() {
    #[derive(Clone)]
    struct Nothing;
    impl Counted for Nothing {
        fn counted(&mut self, _ty: &Ty) -> Answer {
            Answer::Unknown
        }
    }
    let program = compile(PROGRAM);
    let plan = analyze(&program, &mut Nothing, &Options::default());
    let main = plan.func(find(&program, "main")).expect("a plan");
    assert!(main.sites.is_empty(), "nothing classified, nothing emitted");
    assert!(!main.unclassified.is_empty(), "and every type is named");
}

/// The two shapes [`Syntactic`] could not classify from bodies alone, and
/// the leak each one was.
///
/// A `Ty::Ctx` is written down nowhere, so no literal names one; an
/// `Option<Str>` that only ever arrives *from* `list.get` is constructed by
/// no `.Some(..)` either. Both were `Answer::Unknown`, which this pass
/// reads as "not counted" and for which it emits nothing at all — so the
/// string `list.get` retained into the payload was never released.
/// `monomorphize::Shapes` is what answers them now.
#[test]
fn a_context_and_an_option_no_literal_builds_are_both_counted() {
    const SRC: &str = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

export fn showFirst<C: Allocator>(ctx: C, o: Option<Str>): Str {
  match (o) { .Some(v) => str.format(ctx, "S${v}"), .None => "N" }
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let built = list.range(ctx, 0, 3).mapCtx(ctx, fn(c, i) => str.format(c, "n${i}"));
  let _ = io.println(ctx, showFirst(ctx, built.get(1))).ignore();
  .Ok(())
}
"#;
    let program = compile(SRC);
    let mut counted = Syntactic::new(&program);
    let f = program.funcs.get(find(&program, "showFirst").index()).expect("a function");
    let ctx_ty = f.locals.first().map(|l| l.ty).expect("the context parameter");
    let opt_ty = f.locals.get(1).map(|l| l.ty).expect("the `Option<Str>` parameter");
    assert!(matches!(ctx_ty.kind(), TyKind::Ctx(_)), "the first parameter is the context");
    // `host.alloc` and `host.stdout` are zero-sized markers, so the answer
    // here is `No` — which is the point: the defect was `Unknown`, which
    // means "no operations at all" for a type that may well hold a
    // closure, and it was `Unknown` for *every* context in the program.
    assert_ne!(counted.counted(&ctx_ty), Answer::Unknown);
    assert_eq!(counted.counted(&opt_ty), Answer::Yes);

    // And nothing in the whole program is left unanswered, which is the
    // property the leak was a symptom of rather than one function's plan.
    let plan = analyze(&program, &mut Syntactic::new(&program), &Options::default());
    let unanswered: Vec<&Ty> = plan.funcs.iter().flat_map(|f| f.unclassified.iter()).collect();
    assert_eq!(unanswered, Vec::<&Ty>::new());
}

/// A `match` on a value it built, where one arm takes the back edge and
/// another falls through.
///
/// The drop of the scrutinee went either before every jump — only where
/// *every* arm jumped — or after the arms, and a `match` that both
/// continues and falls out has paths of each kind. So the recursive arm
/// disposed of nothing, and a drain leaked its `Option` once an iteration.
/// Both keys are emitted now; a path takes exactly one of them.
#[test]
fn a_fresh_scrutinee_is_dropped_on_the_arm_that_jumps_too() {
    const SRC: &str = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/list" import * as list;

fn takeOne<C: Allocator>(ctx: C, xs: [Int]): Option<(Int, [Int])> {
  match (xs.first()) {
.Some(v) => .Some((v, xs.drop(ctx, 1))),
.None => .None,
  }
}

export fn drain<C: Allocator>(ctx: C, xs: [Int], acc: [Int]): [Int] {
  match (takeOne(ctx, xs)) {
.Some(t) => {
  let (v, rest) = t;
  drain(ctx, rest, acc.push(ctx, v))
},
.None => acc,
  }
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "${drain(ctx, [1, 2], []).length()}").ignore();
  .Ok(())
}
"#;
    let program = compile_native(SRC);
    let i = loop_body(&program, "drain");
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());
    let fp = plan.func(i).expect("a plan");
    assert_eq!(
        named_sites(&program, i, fp),
        vec![
            // `n3` is `takeOne(ctx, xs)`, the value the match built. It is
            // dropped after the match — the `.None` arm's path — *and*
            // before the back edge at `n11`, which is the `.Some` arm's.
            // Before, only the first of the two was there.
            "dec n3 after n2",
            "dec xs after n3",
            // `t` points into `n3`, so `let (v, rest) = t` binds borrowed
            // names and only `rest`, handed to the jump, takes a count.
            "inc rest after n10",
            "dec acc after n11",
            "dec n3 after n11",
        ]
    );
}

/// **Calling a closure does not consume it.**
///
/// `child_modes` gave every child of an [`ExprKind::CallValue`] `Own`, and
/// [`kids`] puts the *callee* first — so the closure a call was made
/// through was treated as handed over to the call. Nothing on the other
/// side takes it: [`ir::Inst::CallIndirect`] is a load of `code` and a pass
/// of `env`, and what frees an environment is the closure value's own drop.
/// So every closure a program ever called leaked its environment, and a
/// closure it merely built and dropped did not — which is why the shape hid
/// in the corpus for so long.
///
/// `collect_consuming` had it right all along: its `CallValue` arm consumes
/// `args` and not `callee`. The two functions describe one convention and
/// disagreed about it.
///
/// The plan for `twice`, before and after:
///
/// ```text
/// before:  inc g after n5
/// after:   dec g after n7
/// ```
///
/// — an increment with no decrement anywhere, against a drop at the last
/// use and no increment at all, because the first call no longer takes a
/// count it was never going to give back. This is
/// `codegen/tail_calls.buri`'s seventeen live blocks and
/// `semantics/evaluation.buri`'s twenty.
#[test]
fn a_called_closure_is_not_consumed_by_the_call() {
    const SRC: &str = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;

export fn twice<C: Allocator>(ctx: C, n: Int): Int {
  let g: fn(Int) => Int = fn(x) => x + n;
  g(100) + g(200)
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "${twice(ctx, 1)}").ignore();
  .Ok(())
}
"#;
    let program = compile_native(SRC);
    let i = find(&program, "twice");
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());
    let fp = plan.func(i).expect("a plan");
    // `n5` is the first call and `n7` the second. One drop, at the last
    // use, and nothing else: a call reads the closure and gives it back.
    assert_eq!(named_sites(&program, i, fp), vec!["dec g after n7"]);
}

/// **A projection of a temporary releases the temporary.**
///
/// A struct is a stack value in both backends, so `mk(ctx).a` allocated
/// nothing for the `Pair` and everything for its two `[Str]` fields — and
/// the moment one field was copied out, the other had no name anywhere and
/// the copy carried the count `mk` had taken for it. `Scan::project` is
/// written for a base this function can *name*, whose owner decides when it
/// dies; a base with no name has no owner, so the projection is where it
/// dies.
///
/// Two positions, because they fail differently:
///
/// ```text
/// firstLen — borrowed by `len`   before: (nothing at all)
///                                 after: dec n2 after n1
///                                        inc n2 after n2
///                                        dec n3 after n2
///
/// keep     — returned            before: inc n1 after n1
///                                 after: inc n1 after n1
///                                        dec n2 after n1
/// ```
///
/// The increment always precedes the release, so the field the projection
/// hands on never reaches zero in between; `keep`'s parent takes the
/// reference and `firstLen`'s borrows it, which is why only the second has
/// a drop of the projection itself. This is `crypto/sha256.buri`'s five
/// live blocks — `crypto.sha256Text(ctx, "x").0`, once per digest.
#[test]
fn a_projection_of_a_temporary_releases_it() {
    const SRC: &str = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/list" import * as list;

struct Pair { a: [Str], b: [Str] }

fn mk<C: Allocator>(ctx: C): Pair { Pair { a: ["x".repeat(ctx, 8)], b: ["y".repeat(ctx, 8)] } }

export fn firstLen<C: Allocator>(ctx: C): Int { mk(ctx).a.length() }

export fn keep<C: Allocator>(ctx: C): [Str] { mk(ctx).a }

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "${firstLen(ctx)} ${keep(ctx).length()}").ignore();
  .Ok(())
}
"#;
    let program = compile_native(SRC);
    let mut counted = Syntactic::new(&program);
    let plan = analyze(&program, &mut counted, &Options::default());

    // `n1` is `len`, `n2` the projection, `n3` the `mk` call: the field is
    // increfed, the `Pair` released, and the borrowed projection dropped
    // after the call that read it.
    let i = find(&program, "firstLen");
    assert_eq!(
        named_sites(&program, i, plan.func(i).expect("a plan")),
        vec!["dec n2 after n1", "inc n2 after n2", "dec n3 after n2"]
    );

    // Returned instead: `n1` is the projection and `n2` the `mk` call. The
    // increment was already there — it is the release that was missing, and
    // without it `b` was freed by nobody and `a` came back one count high.
    let i = find(&program, "keep");
    assert_eq!(
        named_sites(&program, i, plan.func(i).expect("a plan")),
        vec!["inc n1 after n1", "dec n2 after n1"]
    );
}

/// This pass reads the tree and does not write it, which is the whole of
/// its isolation from the JavaScript backend: even if `middle::run` ever
/// called it, no artifact could move.
#[test]
fn the_tree_is_unchanged() {
    let program = compile(PROGRAM);
    let before: Vec<String> =
        program.funcs.iter().map(|f| format!("{:?}", f.body())).collect();
    let plan = run(&program);
    let after: Vec<String> =
        program.funcs.iter().map(|f| format!("{:?}", f.body())).collect();
    assert_eq!(before, after);
    assert_eq!(plan.funcs.len(), program.funcs.len());
}

/// Every `core/actor` key hands a value to whichever task drives the actor
/// next, so every one of them is a crossing — which is the direction an
/// omission has to be wrong in, since a block nobody marked is a block two
/// threads count without an atomic.
#[test]
fn an_actor_is_a_place_a_value_waits_for_another_task() {
    for key in [
        "actor.mailboxOpen",
        "actor.mailboxPush",
        "actor.mailboxPop",
        "actor.mailboxClose",
        "actor.stateTake",
        "actor.statePut",
        "actor.replyOpen",
        "actor.replyPut",
        "actor.replyTake",
    ] {
        assert!(crosses_tasks(key), "{key} hands a value to another task");
    }
    // The module's own Buri half does not: `core/actor`'s functions are
    // ordinary code and it is the intrinsics underneath them that cross.
    assert!(!crosses_tasks("alloc.copyOut"));
    assert!(!crosses_tasks("list.push"));
}
