//! Parkability: which functions, and which function values, can reach a host
//! operation that blocks.
//!
//! This is the JavaScript backend's `async` question: a function that can
//! park is printed `async` and every call to it is awaited. Only
//! [`super::generate`] asks it. No native backend reads it, so the native
//! pipeline never computes it.
//!
//! The answer is per *instantiation*: it runs over the post-monomorphization
//! program, where one source function at two contexts is two `Func` slots.

use crate::compiler::backend::intrinsic_keys;
use crate::compiler::middle::monomorphize::{self, FuncKind, Program};
use crate::compiler::semantics::typed::{self, Expr, ExprKind, Stmt};
use crate::compiler::semantics::types::{LocalId, Ty};
use crate::hash::Set as HashSet;

/// Whether an intrinsic key names a host operation that **blocks**: the call
/// does not return until something outside this program — a disk, a socket, a
/// clock, a terminal — is ready.
///
/// This is the seed of the parkability column, and it is a list of *keys*
/// rather than of effects on purpose. `FileSystemRead` is an effect; `host.HostFileSystem`
/// and `host_testing.TestFileSystem` are two implementations of it, and only the first
/// one waits. A per-instantiation answer can tell them apart because they are
/// different `Func` slots, and that difference is the whole point of asking
/// the question here rather than at the signature.
///
/// Two keys wait on **this program** rather than on the world —
/// `Tasks.parallel` and the scheduler double beneath it — and they are here
/// for the consequence rather than for the cause: the call does not return
/// until a step has finished, and a step may itself sleep, dial a socket or
/// ask an actor. A double that answered before its step had is the one place
/// where a test could read less than the program did.
///
/// Everything absent is *not* suspending, so an omission is the direction that
/// costs correctness rather than performance. That is why the whole
/// `host.HostFileSystem`/`host.HostFileSystem` surface is in by prefix rather than
/// method by method, and why a new blocking host operation belongs here on the
/// day it is added. The prefix stops at `host.HostFileSystem` so that both halves of
/// the filesystem are covered by the one string.
///
/// **This list can only answer for a key whose wait is the key's own.** A
/// combinator handed the caller's context — `list.mapCtx` and the four beside
/// it — waits exactly when the *step it was given* waits, and no seed can say
/// which: the same key is a plain loop over a rendering and a wait over a
/// socket dial. That half is [`intrinsic_keys::ctx_step_key`], asked by
/// [`parkability`] against the argument edges it has already walked, and it is
/// the other seed of the same column.
pub fn suspends(key: &str) -> bool {
    key.starts_with("host.HostFileSystem")
        // Every `Listen` operation waits on something outside the program: a
        // bind resolves a name, an accept waits for a client — the longest wait
        // a program can make — and a respond writes to a socket a peer may be
        // reading slowly. By prefix for the filesystem's reason, and because a
        // fifth operation added here should not need an edit there to be
        // correct. `host.HostSockets` is deliberately absent: a frame is
        // enqueued rather than delivered, which is the whole of what
        // `socketSendText` promises.
        || key.starts_with("host.HostListen.")
        // The client half of the same story, and both of its operations wait:
        // `connectSocket` resolves a name, opens a connection and finishes a
        // handshake, and `connectReceive` is where a client sits between
        // messages — `listenReceive`'s wait from the other end. By prefix for
        // `HostListen`'s reason.
        || key.starts_with("host.HostWebSocketClient.")
        // Starting a program and waiting for it is the longest wait a process
        // can make on purpose. The effect has one method, so this is a
        // `matches!` arm rather than a prefix — but it is written as a prefix
        // for `HostFileSystem`'s reason, since a second method here would want the same
        // answer on the day it lands.
        || key.starts_with("host.HostSpawn.")
        || matches!(
            key,
            "host.HostNetwork.fetch"
                | "host.HostClock.sleepMilliseconds"
                | "host.HostStdin.readLine"
                | "host.HostStdin.readBytes"
                // `Tasks.parallel` is the one entry here that does not wait on
                // anything *outside* the program: it waits on the program's own
                // tasks. It belongs on the list all the same, and for the same
                // consequence — the call does not return until something else
                // has finished, so a caller of it is a function that may be in
                // the middle of a call while other work runs. On JavaScript
                // that is literally an `await`; on the natives it is what makes
                // the caller's frame outlive a scheduling decision.
                | "host.HostTasks.parallel"
                // The double waits for the same reason, and it is the one
                // `host_testing` key that does. It runs each step to
                // completion before starting the next, so a step that sleeps,
                // dials a socket or asks an actor makes the call outlive that
                // wait — and a test whose spawned task waits reads what the
                // task did rather than what it had got to.
                | "host_testing.TestTasks.parallel"
                // `core/actor`'s three waits, and they wait on the program's
                // own actors for `Tasks.parallel`'s reason rather than on the
                // world. `mailboxPush` waits for room in a full mailbox;
                // `mailboxClose` waits for the step in flight to put the state
                // back, which is what "`stop` lets the current message finish"
                // means; `stateTake` waits for a step another task is running,
                // so a sender that arrives mid-step gets its answer
                // (buri-lang/buri#205). The other six never wait, and listing
                // the family by prefix would have claimed otherwise.
                | "actor.mailboxPush"
                | "actor.mailboxClose"
                | "actor.stateTake"
        )
        // A `core/lazy` chunk node. It waits on a file the program has not
        // fetched yet, which is the longest wait in the list on a cold page —
        // and unlike everything above it, it is spelled as an inline
        // `ExprKind::Intrinsic` rather than reached through a declaration, so
        // `body_parks` is what asks.
        || intrinsic_keys::lazy_chunk_of(key).is_some()
}

// ---------------------------------------------------------------------------
// Parkability
// ---------------------------------------------------------------------------

/// Which functions can park, and which **function values** can — because a
/// call through one is the case the column is hardest to answer for.
///
/// The graph is the *post-monomorphization* one, so the column is per
/// instantiation, which is what it needs: `fs.readText` at a context binding
/// `host.HostFileSystem` and `fs.readText` at one binding `host_testing.TestFileSystem` are
/// two `Func` slots reached from two `Key::Fn` entries, and only the first
/// reaches a call that waits.
///
/// # The indirect call
///
/// This column used to answer `true` at every [`ExprKind::CallValue`], the way
/// `middle::rc`'s purity column still does, which reads *"every `map` may
/// park"* — the callback of `list.map` is a code pointer with no name, so the
/// worst function in the program was the answer. That is not free on the
/// JavaScript backend, where the column decides which functions are printed
/// `async`: `async` is
/// not a property a caller may ignore, an `async` function returns a promise
/// whether or not it ever waits, and this compiler hands function values to
/// JavaScript that cannot await one — a `view` given to `mount`, the row
/// callbacks inside `ui.each`, a sort comparator, the callback of
/// `$list_map`. (`$list_mapCtx`'s callback is the one that *can* now, and only
/// where this column says so: the runtime carries a second, awaiting body and
/// the emitter picks between them.) So the imprecision was not merely a cost:
/// it was a whole backend's reason for computing the question a second time
/// (`reports/can-park-indirect.md`), and this is that second analysis folded
/// back into the first.
///
/// Two rules answer a callee position, in this order.
///
/// **1. The type** (`concurrency-design.md`'s B2). A function value that
/// receives no effect-carrying argument cannot park. It cannot capture a
/// capability — SPEC 10.6 forbids it, which is the same guarantee
/// `middle::closures` rests on — and it cannot construct one, because SPEC
/// 11.3 builds a context only in `main`'s body, a test, or a test-only module,
/// and SPEC 10.4 spells out that none of those is a function anybody calls. So
/// everything a function value can reach that waits arrived through one of its
/// own parameters, and [`monomorphize::Effects::fn_takes_effect`] is the whole
/// question at `fn(A) => B`. This is what stops `list.map` parking.
///
/// **2. The value**, where the type leaves the question open. `fn(C, A) => B`
/// is exactly `list.mapCtx`'s shape and exactly the shape a callback that
/// sleeps has, so the type cannot separate them and the values have to be
/// followed:
///
///  * **`parking[f]`** — the locals of `f` (its parameters and its `let`s)
///    whose function value may park. A parameter is followed when the function
///    owning it is never used as a *value* ([`address_taken`]): then every
///    argument it can receive is written at a `CallFn` or a `Continue` naming
///    it, and those are edges this pass walks. A `let` is followed when it
///    binds one name to one expression ([`fn_lets`]).
///  * **`resolved[f]`** — the locals it followed. A fn-typed local outside
///    this set is one it lost track of.
///  * **`parking_types`** — the types of the parking function values the
///    program builds, which is what a position it could not follow is worth: a
///    callee of type `T` can only ever hold a value of type `T`, so the sound
///    answer is whether the program builds a parking value of that same type.
///
/// Keying the fallback by type rather than by a program-wide flag is not a
/// refinement, it is the difference between right and wrong, and the
/// monorepo's page is the proof: `Prop.read`'s third arm calls a
/// `fn(Scope) => T` out of an enum payload while an `onPress` handler
/// elsewhere in the same program fetches. Under a flag `Prop.read` became
/// `async`, and `Prop.read` is reached from a style thunk the *runtime* calls,
/// which cannot await — the page died on `{} is not iterable`. Under the type
/// the handler is a `fn(C, Bool) => ()` and the arm's callee is a
/// `fn(Scope) => T`; they never meet. Soundness rests on the program being
/// well typed, which everything downstream of the checker already assumes.
///
/// **What is given up**, recorded rather than hidden: a parking function value
/// that shares a type with a callee position it can never reach makes that call
/// parkable. The precise alternative — values through struct fields, enum
/// payloads, array elements and returns — is a real higher-order flow analysis
/// and buys nothing today, since nothing in this tree stores a parking callback
/// anywhere.
///
/// [`ExprKind::CallTrait`] stays conservative. Monomorphization resolves every
/// one it can, and one that survives is a program the backends already turn
/// into an abort.
#[derive(Clone, Debug, Default)]
pub struct Parking {
    /// One row per [`Program::funcs`] slot: whether it can park.
    parks: Vec<bool>,
    /// Per slot, the locals whose function value may park.
    parking: Vec<LocalSet>,
    /// Per slot, the locals whose function value this pass followed.
    resolved: Vec<LocalSet>,
    /// The types of the parking function values this program builds.
    parking_types: HashSet<Ty>,
    /// Whether the program builds one at all — the answer where there is not
    /// even a function type to ask under, which a well-typed program does not
    /// reach.
    any_parking_value: bool,
    /// [`monomorphize::Program::shapes`]'s effect table, so rule 1 above can be
    /// asked without a `Tables`.
    effects: monomorphize::Effects,
}

impl Parking {
    /// Whether the function in this slot, or anything it calls, can reach a
    /// host operation that blocks.
    ///
    /// A slot this pass has no row for answers `true`, which is the direction
    /// that over-approximates.
    pub fn parks(&self, index: usize) -> bool {
        self.parks.get(index).copied().unwrap_or(true)
    }

    /// Whether a call **through** the function value this expression produces
    /// can park.
    ///
    /// `fi` is the slot whose body the expression is written in, because a
    /// local means nothing without one.
    pub fn value_parks(&self, fi: usize, e: &Expr) -> bool {
        // Rule 1: the type. Asked first because it holds whatever else the
        // program contains, and because it is the common case — a comparator,
        // a predicate, a `fn(A) => B` — answered without looking anything up.
        if !self.effects.fn_takes_effect(&e.ty) {
            return false;
        }
        match &e.kind {
            // A callee still naming a declaration means monomorphization did
            // not run, which the backends turn into an abort; `true` is the
            // answer that cannot be wrong in the meantime.
            ExprKind::FnRef(c) => c.func().is_none_or(|i| self.parks(i.index())),
            ExprKind::Closure { func, .. } => self.parks(func.index()),
            ExprKind::Lambda { body, .. } => self.body_parks(fi, body),
            ExprKind::Local(l) => {
                if self.parking.get(fi).is_some_and(|s| s.contains(*l)) {
                    true
                } else if self.resolved.get(fi).is_some_and(|s| s.contains(*l)) {
                    false
                } else {
                    self.unfollowed(&e.ty)
                }
            }
            // A function value is whatever the branch that produced it is.
            ExprKind::Block { tail: Some(t), .. } => self.value_parks(fi, t),
            ExprKind::If { then, else_, .. } => {
                self.value_parks(fi, then) || self.value_parks(fi, else_)
            }
            ExprKind::Match { arms, .. } => arms.iter().any(|a| self.value_parks(fi, &a.body)),
            _ => self.unfollowed(&e.ty),
        }
    }

    /// The answer for a function value this pass could not follow: whether the
    /// program builds a parking one of the same type.
    fn unfollowed(&self, ty: &Ty) -> bool {
        if is_fn_ty(ty) {
            self.parking_types.contains(ty)
        } else {
            self.any_parking_value
        }
    }

    /// Whether running this body reaches something that waits.
    ///
    /// The walk descends into lambda bodies, so a function holding a lambda
    /// that parks parks too — which it must, on the backend that prints
    /// `async`, because the `await` inside the arrow is inside this function's
    /// own text.
    fn body_parks(&self, fi: usize, body: &Expr) -> bool {
        let mut k = false;
        typed::walk(body, &mut |e| match &e.kind {
            ExprKind::CallFn { func, .. } => match func.func() {
                Some(c) => k = k || self.parks(c.index()),
                None => k = true,
            },
            // A jump into another function's loop is a call.
            ExprKind::Continue { func: Some(c), .. } => k = k || self.parks(c.index()),
            ExprKind::CallValue { callee, .. } => k = k || self.value_parks(fi, callee),
            ExprKind::CallTrait { .. } => k = true,
            // A host call spelled as an inline node rather than reached
            // through an intrinsic *function*; the same seed answers both.
            ExprKind::Intrinsic { name, .. } => k = k || suspends(name),
            _ => {}
        });
        k
    }
}

/// Whether a type is a function type, which is the only kind of value a
/// [`ExprKind::CallValue`] can reach and so the only kind [`Parking`] tracks.
fn is_fn_ty(t: &Ty) -> bool {
    matches!(t, Ty::Fn(..))
}

/// The `let`s of one body that bind a single name to a single function value.
///
/// A destructuring pattern is deliberately not one: the local it binds stays
/// unresolved, and an unresolved fn-typed local is worth what
/// [`Parking::unfollowed`] says of its type.
fn fn_lets(body: &Expr) -> Vec<(LocalId, &Expr)> {
    let mut out = Vec::new();
    typed::walk(body, &mut |e| {
        let ExprKind::Block { stmts, .. } = &e.kind else { return };
        for s in stmts {
            let Stmt::Let { pattern, value, .. } = s else { continue };
            let typed::PatKind::Bind { local, sub: None } = &pattern.kind else { continue };
            if is_fn_ty(&value.ty) {
                out.push((*local, value));
            }
        }
    });
    out
}

/// The function slots whose **address is taken**: named by an `FnRef` or a
/// `Closure` rather than called.
///
/// One of these can be called from a position no `CallFn` names, so nothing can
/// be proved about what its parameters hold. Every other slot receives
/// arguments only where this pass can see them.
fn address_taken(program: &Program) -> Vec<bool> {
    let mut out = vec![false; program.funcs.len()];
    for f in &program.funcs {
        let Some(body) = f.body() else { continue };
        typed::walk(body, &mut |e| {
            let taken = match &e.kind {
                ExprKind::FnRef(c) => c.func(),
                ExprKind::Closure { func, .. } => Some(*func),
                _ => None,
            };
            if let Some(slot) = taken.and_then(|i| out.get_mut(i.index())) {
                *slot = true;
            }
        });
    }
    out
}

/// A set of one function's locals, one bit per [`LocalId`]: dense, because a
/// function's locals are numbered from zero.
#[derive(Clone, Debug, Default)]
struct LocalSet(Vec<u64>);

impl LocalSet {
    fn contains(&self, l: LocalId) -> bool {
        let i = l.0 as usize;
        self.0.get(i / 64).is_some_and(|w| (w >> (i % 64)) & 1 == 1)
    }

    /// Adds `l`, answering whether it was not already there.
    fn insert(&mut self, l: LocalId) -> bool {
        let i = l.0 as usize;
        let word = i / 64;
        if self.0.len() <= word {
            self.0.resize(word.saturating_add(1), 0);
        }
        let bit = 1u64 << (i % 64);
        match self.0.get_mut(word) {
            Some(w) if *w & bit == 0 => {
                *w |= bit;
                true
            }
            _ => false,
        }
    }
}

/// [`Parking`], computed.
pub fn parkability(program: &Program) -> Parking {
    let n = program.funcs.len();
    let mut w = Parking {
        // An unbuilt body is lowered to an abort, which is the one thing that
        // certainly does not wait, so only a suspending intrinsic seeds `true`.
        parks: program
            .funcs
            .iter()
            .map(|f| match &f.kind {
                // A method a platform's `js` file implements may suspend, and
                // nothing in the file says whether it does: always awaited.
                FuncKind::Intrinsic(key) => {
                    suspends(key) || program.hosted.js_implemented.contains(key)
                }
                FuncKind::Unbuilt | FuncKind::Body(_) => false,
            })
            .collect(),
        parking: vec![LocalSet::default(); n],
        resolved: vec![LocalSet::default(); n],
        parking_types: HashSet::default(),
        any_parking_value: false,
        effects: program.shapes.effects.clone(),
    };

    // What can be followed, decided once: the tree does not move.
    let addressed = address_taken(program);
    let lets: Vec<Vec<(LocalId, &Expr)>> =
        program.funcs.iter().map(|f| f.body().map(fn_lets).unwrap_or_default()).collect();
    for (i, f) in program.funcs.iter().enumerate() {
        let Some(slot) = w.resolved.get_mut(i) else { continue };
        if !addressed.get(i).copied().unwrap_or(true) {
            for p in &f.params {
                slot.insert(*p);
            }
        }
        for (local, _) in lets.get(i).into_iter().flatten() {
            slot.insert(*local);
        }
    }

    // Arguments, into the parameters they are bound to, and every node that
    // *is* a function value — an `FnRef`, a `Closure` or a `Lambda` — with the
    // slot it is written in. Both are read every round and neither moves.
    let mut edges: Vec<(usize, usize, &Vec<Expr>)> = Vec::new();
    let mut values: Vec<(usize, &Expr)> = Vec::new();
    for (i, f) in program.funcs.iter().enumerate() {
        let Some(body) = f.body() else { continue };
        typed::walk(body, &mut |e| match &e.kind {
            ExprKind::CallFn { func, args } => {
                if let Some(c) = func.func() {
                    edges.push((c.index(), i, args));
                }
            }
            // A `Continue` rebinds the parameters of the function it enters, in
            // order — the dispatch index the backend prepends is not one of
            // them.
            ExprKind::Continue { func, args, .. } => {
                edges.push((func.map_or(i, |c| c.index()), i, args));
            }
            ExprKind::FnRef(_) | ExprKind::Closure { .. } | ExprKind::Lambda { .. } => {
                values.push((i, e));
            }
            _ => {}
        });
    }

    // Monotone in every column — a row only ever climbs — so the loop
    // terminates in at most one pass per edge.
    let mut changed = true;
    while changed {
        changed = false;

        for &(target, from, args) in &edges {
            let Some(tf) = program.funcs.get(target) else { continue };
            for (j, a) in args.iter().enumerate() {
                if !is_fn_ty(&a.ty) || !w.value_parks(from, a) {
                    continue;
                }
                let Some(p) = tf.params.get(j).copied() else { continue };
                if w.parking.get_mut(target).is_some_and(|s| s.insert(p)) {
                    changed = true;
                }
            }
        }

        // The `let`s that hold one.
        for (i, binds) in lets.iter().enumerate() {
            for (local, value) in binds {
                if !w.value_parks(i, value) {
                    continue;
                }
                if w.parking.get_mut(i).is_some_and(|s| s.insert(*local)) {
                    changed = true;
                }
            }
        }

        // The parking function values the program builds, collected by type,
        // which is what every position the two sets above did not follow is
        // worth.
        let mut found: Vec<&Ty> = Vec::new();
        for &(i, e) in &values {
            let parks = match &e.kind {
                ExprKind::FnRef(c) => c.func().is_none_or(|x| w.parks(x.index())),
                ExprKind::Closure { func, .. } => w.parks(func.index()),
                ExprKind::Lambda { body, .. } => w.body_parks(i, body),
                _ => false,
            };
            if parks {
                found.push(&e.ty);
            }
        }
        for ty in found {
            if w.parking_types.insert(ty.clone()) {
                changed = true;
            }
            if !w.any_parking_value {
                w.any_parking_value = true;
                changed = true;
            }
        }

        // And the column itself. A function that *is* a suspending intrinsic
        // has no body and is never reached here; one with a body starts at
        // `false` and only ever climbs, so the seed is not lost.
        for (i, f) in program.funcs.iter().enumerate() {
            if w.parks.get(i).copied() == Some(true) {
                continue;
            }
            // A combinator that **runs a step it was handed the context for**
            // waits exactly when that step waits, and no seed can say which:
            // `list.mapCtx` over a rendering is a loop, and over a socket dial
            // it is a wait. So the answer is read off the argument edges the
            // loop above already walked — the step that actually arrived at
            // this instantiation — and a key with a parking step joins the
            // column beside the keys whose wait is their own.
            //
            // Per instantiation, so `mapCtx` at one element type does not
            // colour `mapCtx` at another. Two call sites that share an
            // instantiation and disagree share the answer, which is the
            // direction that costs a microtask rather than a result.
            if let FuncKind::Intrinsic(key) = &f.kind {
                let stepped = intrinsic_keys::ctx_step_key(key)
                    && f.params.iter().any(|p| w.parking.get(i).is_some_and(|s| s.contains(*p)));
                if stepped {
                    if let Some(slot) = w.parks.get_mut(i) {
                        *slot = true;
                    }
                    changed = true;
                    continue;
                }
            }
            let Some(body) = f.body() else { continue };
            if w.body_parks(i, body) {
                if let Some(slot) = w.parks.get_mut(i) {
                    *slot = true;
                }
                changed = true;
            }
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::middle::rc::tests::{compile, find};
    use crate::compiler::semantics::types::{self, FuncIdx};
    use crate::diagnostics::Span;
    use crate::hash::Map as HashMap;
    use monomorphize::Func;

    /// One program with all three callee shapes in it, and the source the
    /// precision rows below are asked of.
    ///
    /// `sleepy` and `quick` are the same signature, the same context and the
    /// same indirect call; the only difference between them is the callback
    /// each is handed. `applyN` is the third shape — the one the *type* rules
    /// out on its own.
    const PRECISION: &str = r#"
from "platform/effect" import { Allocator, Clock, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/time" import * as time;

fn sleepy<C: Clock>(ctx: C, n: Int, body: fn(C) => Int): Int {
  if (n <= 0) { 0 } else { body(ctx) + sleepy(ctx, n - 1, body) }
}

fn quick<C: Clock>(ctx: C, n: Int, body: fn(C) => Int): Int {
  if (n <= 0) { 0 } else { body(ctx) + quick(ctx, n - 1, body) }
}

fn applyN(n: Int, x: Int, f: fn(Int) => Int): Int {
  if (n <= 0) { x } else { applyN(n - 1, f(x), f) }
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Clock: host.clock, Stdout: host.stdout };
  let slow = sleepy(ctx, 2, fn(c) => {
    let _ = time.sleep(c, time.milliseconds(1));
    5
  });
  let fast = quick(ctx, 2, fn(c) => 5);
  let plain = applyN(3, 1, fn(x) => x + 1);
  let _ = io.println(ctx, "${slow} ${fast} ${plain}").ignore();
  .Ok(())
}
"#;

    /// The golden of `the_parking_count_of_a_representative_program_is_a_golden`.
    const GOLDEN_PARKING: usize = 5;
    // Twelve since `sleepMs` went away: the snippet now writes
    // `time.sleep(c, time.milliseconds(1))`, which is a `Duration`
    // constructor and its two saturating helpers where a bare `Int` used to
    // cross. Thirteen since `main` took a host, for the function that builds
    // it — and that one parks too, because it calls `main`, which is what
    // keeps a host call awaited all the way out to the epilogue.
    const GOLDEN_FUNCS: usize = 13;
    const GOLDEN_NAMES: [&str; GOLDEN_PARKING] = [
        "core/time:sleep",
        "platform/host:HostClock.sleepMilliseconds",
        "rc_test.buri:main",
        "rc_test.buri:main, building its host",
        "rc_test.buri:sleepy",
    ];

    // -- the column --------------------------------------------------------
    //
    // Hand-built programs rather than snippets, because the question is about
    // two *instantiations* of one source function and a snippet cannot put
    // them side by side without dragging the whole hermetic test context in
    // with them. What monomorphization guarantees, and what these stand in
    // for: a `Key::Fn(id, targs)` per context, one `Func` slot each, and the
    // callee of every `CallFn` already a `FuncIdx`.

    fn parked(program: &Program) -> Vec<bool> {
        parkability(program).parks
    }

    fn intrinsic_func(name: &str, key: &str) -> Func {
        Func {
            symbol: name.to_string(),
            debug_name: name.to_string(),
            params: Vec::new(),
            locals: Vec::new(),
            kind: FuncKind::Intrinsic(key.to_string()),
            ret: Ty::Unit,
            desc: None,
            span: Span::default(),
        }
    }

    fn body_func(name: &str, body: Expr) -> Func {
        Func {
            symbol: name.to_string(),
            debug_name: name.to_string(),
            params: Vec::new(),
            locals: Vec::new(),
            kind: FuncKind::Body(body),
            ret: Ty::Unit,
            desc: None,
            span: Span::default(),
        }
    }

    fn call_to(to: u32) -> Expr {
        Expr::new(
            ExprKind::CallFn { func: typed::Callee::Func(FuncIdx(to)), args: Vec::new() },
            Ty::Unit,
            Span::default(),
        )
    }

    fn fn_ty(params: Vec<Ty>, ret: Ty) -> Ty {
        Ty::Fn(params, Box::new(ret))
    }

    /// A call to `to` whose *result* is a function value of type `ty` — a
    /// callee position [`Parking`] cannot follow, so one answered by the type.
    fn call_to_ty(to: u32, ty: Ty) -> Expr {
        Expr::new(
            ExprKind::CallFn { func: typed::Callee::Func(FuncIdx(to)), args: Vec::new() },
            ty,
            Span::default(),
        )
    }

    fn call_value(callee: Expr) -> Expr {
        Expr::new(
            ExprKind::CallValue { callee: Box::new(callee), args: Vec::new() },
            Ty::Unit,
            Span::default(),
        )
    }

    /// A function value of type `ty` whose body is `body`.
    fn lambda_of(ty: Ty, body: Expr) -> Expr {
        Expr::new(
            ExprKind::Lambda {
                params: vec![LocalId(0)],
                body: Box::new(body),
                captures: Vec::new(),
            },
            ty,
            Span::default(),
        )
    }

    /// A body that calls each of these in turn.
    fn calls(to: &[u32]) -> Expr {
        Expr::new(
            ExprKind::Tuple(to.iter().map(|t| call_to(*t)).collect()),
            Ty::Unit,
            Span::default(),
        )
    }

    fn hand_built(funcs: Vec<Func>) -> Program {
        Program {
            funcs,
            roots: monomorphize::ProgramRoots::Main(FuncIdx(0)),
            descriptors: Vec::new(),
            desc_modules: Vec::new(),
            desc_index: HashMap::default(),
            cell_equal: HashMap::default(),
            ctx_layouts: HashMap::default(),
            shapes: Default::default(),
            stylesheet: String::new(),
            inline_styles: false,
            icons: false,
            themes: false,
            chunks: Vec::new(),
            hosted: Default::default(),
        }
    }

    /// Naive iteration has to go round a cycle more than once, and a cycle
    /// that reaches nothing blocking has to *stay* at `false` rather than
    /// climbing because it is a cycle.
    #[test]
    fn parkability_is_a_fixpoint_across_a_cycle() {
        // 0 main -> 1 and 5; 1 <-> 2, and 2 -> 3 -> 4, the blocking intrinsic.
        // 5 <-> 6 is a second cycle, reaching only 7, which does not block.
        let program = hand_built(vec![
            body_func("main", calls(&[1, 5])),
            body_func("a", calls(&[2])),
            body_func("b", calls(&[1, 3])),
            body_func("c", calls(&[4])),
            intrinsic_func("readFile", "host.HostFileSystem.readFile"),
            body_func("x", calls(&[6])),
            body_func("y", calls(&[5, 7])),
            intrinsic_func("nowMilliseconds", "host.HostClock.nowMilliseconds"),
        ]);
        assert_eq!(
            parked(&program),
            vec![true, true, true, true, true, false, false, false],
            "the blocking half is `true` all the way back to `main`, and reading \
             the clock does not make the other half wait"
        );
    }

    /// The case the column exists for: one source function, two contexts, two
    /// answers.
    ///
    /// `fs.readText<C: Allocator + FileSystemRead>` at a context binding
    /// `host.HostFileSystem` reaches
    /// a call that waits on a disk; the same source at the hermetic test
    /// context reaches `host_testing.TestFileSystem`, which is a page of memory.
    /// Monomorphization has already made them two `Func` slots, so the
    /// fixpoint separates them with no further analysis.
    #[test]
    fn one_source_function_at_two_contexts_gets_two_answers() {
        let program = hand_built(vec![
            body_func("main", calls(&[1, 2])),
            body_func("fs:readText<HostFileSystem>", calls(&[3])),
            body_func("fs:readText<TestFileSystem>", calls(&[4])),
            intrinsic_func("HostFileSystem.readFile", "host.HostFileSystem.readFile"),
            intrinsic_func("TestFileSystem.readFile", "host_testing.TestFileSystem.readFile"),
        ]);
        let parks = parked(&program);
        assert!(parks[1], "`readText` at `host.HostFileSystem` waits on the disk");
        assert!(!parks[2], "`readText` at the test context reaches only memory");
        assert!(parks[0], "and a caller of both waits, because one half of it does");
    }

    /// Every key in the seed list, and the near misses beside them: reading
    /// the clock is not sleeping on it, and the whole `HostFileSystem` surface is in
    /// by prefix rather than by enumeration.
    #[test]
    fn the_seed_list_is_the_blocking_host_calls_and_nothing_else() {
        for key in [
            "host.HostFileSystem.readFile",
            "host.HostFileSystem.writeFile",
            "host.HostFileSystem.syncFile",
            "host.HostNetwork.fetch",
            "host.HostClock.sleepMilliseconds",
            "host.HostStdin.readLine",
            "host.HostStdin.readBytes",
            "host.HostWebSocketClient.connectSocket",
            "host.HostWebSocketClient.connectReceive",
            // `core/actor`'s three, and they are the family's *only* three.
            "actor.mailboxPush",
            "actor.mailboxClose",
            "actor.stateTake",
            // The scheduler double, which is the one `host_testing` key that
            // waits: it runs a step to completion, and a spawned task that
            // sleeps or asks an actor waits inside one.
            "host_testing.TestTasks.parallel",
        ] {
            assert!(suspends(key), "{key} blocks");
        }
        for key in [
            "host.HostClock.nowMilliseconds",
            "host.HostStdout.println",
            "host.HostRandom.nextInt",
            "host_testing.TestFileSystem.readFile",
            "host_testing.TestClock.sleepMilliseconds",
            // The client double reaches no network, so neither of its two
            // methods waits — `host_testing.TestFileSystem` one line up, for its reason.
            "host_testing.TestWebSocketClient.connectSocket",
            "host_testing.TestWebSocketClient.connectReceive",
            "derivePrimHash",
            // The other six `actor.*` keys. Listing them is the half a
            // prefix rule would have got wrong: none of them waits for
            // anything, so none of them makes its caller `async`.
            "actor.mailboxOpen",
            "actor.mailboxPop",
            "actor.statePut",
            "actor.replyOpen",
            "actor.replyPut",
            "actor.replyTake",
        ] {
            assert!(!suspends(key), "{key} does not block");
        }
    }

    /// An indirect call is answered by what the value at that position can
    /// reach, and this pins **both** directions of it (B2).
    ///
    /// Slot 1 calls through a `fn(()) => ()`. Nothing that type can hold is
    /// able to wait: a function value cannot capture a capability (SPEC 10.6)
    /// and cannot construct one (SPEC 11.3, 10.4), so everything it can reach
    /// that blocks arrived through one of its own parameters, and this one has
    /// none that carries an effect. It is `false` **whatever else the program
    /// contains** — and the program contains a callback that really does sleep.
    ///
    /// Slot 2 calls through a `fn(C) => ()` at the same unfollowable position.
    /// That is `list.mapCtx`'s shape and it is also the shape of the callback
    /// slot 4 builds, so the type cannot separate them: the answer is the
    /// program's own set of parking function values, and it is `true`.
    ///
    /// Before this refinement both were `true`, which is what "every `map` may
    /// park" meant.
    #[test]
    fn a_call_through_a_function_value_is_answered_by_what_it_can_reach() {
        let plain = fn_ty(vec![Ty::Unit], Ty::Unit);
        let carrying = fn_ty(vec![Ty::Ctx(types::CtxTypeId(0))], Ty::Unit);
        let program = hand_built(vec![
            body_func("main", calls(&[1, 2])),
            body_func("through a plain callback", call_value(call_to_ty(3, plain))),
            body_func(
                "through a callback taking the context",
                call_value(call_to_ty(3, carrying.clone())),
            ),
            body_func("maker", Expr::new(ExprKind::Unit, Ty::Unit, Span::default())),
            body_func("builder", lambda_of(carrying, call_to(5))),
            intrinsic_func("sleepMilliseconds", "host.HostClock.sleepMilliseconds"),
        ]);
        assert_eq!(
            parked(&program),
            vec![true, false, true, false, true, true],
            "a callee that takes no capability cannot park; one that takes the \
             context is worth whatever function value of its type the program \
             builds"
        );
    }

    /// The same two directions of one compiled program, over the two shapes the
    /// design row names: a `map`-shaped wrapper and a `mapCtx`-shaped one.
    ///
    /// `applyN` is `list.map`: its callback is a `fn(Int) => Int`, so no
    /// instantiation of it can ever park, and this is the whole of what B2
    /// bought. `sleepy` and `quick` are the same `mapCtx` shape at the same
    /// context with the same signature — two `Func` slots that the *type*
    /// cannot tell apart — and only the one whose callback sleeps parks.
    #[test]
    fn a_map_shaped_wrapper_cannot_park_and_a_map_ctx_shaped_one_may() {
        let program = compile(PRECISION);
        let parking = parkability(&program);
        let park = |name: &str| parking.parks(find(&program, name).index());
        assert!(
            !park("applyN"),
            "a callback that takes no context cannot reach anything that waits"
        );
        assert!(park("sleepy"), "and one that sleeps on the context does");
        assert!(
            !park("quick"),
            "while the same shape handed a callback that does not sleep does not"
        );
    }

    /// A golden count over that program, so a regression in either direction is
    /// visible rather than merely slower.
    ///
    /// **Six of nine before this refinement, four of nine after.** The two that
    /// stopped parking are `applyN` and `quick`, and `main` still does because
    /// it really can sleep. Re-bless the numbers when the standard library the
    /// snippet reaches changes; a jump back towards the total is the refinement
    /// coming undone.
    #[test]
    fn the_parking_count_of_a_representative_program_is_a_golden() {
        let program = compile(PRECISION);
        let parks = parked(&program);
        let mut names: Vec<&str> = program
            .funcs
            .iter()
            .zip(&parks)
            .filter(|(_, p)| **p)
            .map(|(f, _)| f.debug_name.as_str())
            .collect();
        names.sort_unstable();
        assert_eq!(
            (names.len(), parks.len()),
            (GOLDEN_PARKING, GOLDEN_FUNCS),
            "the parking functions of the precision snippet, out of all of them: \
             {names:?}"
        );
        assert_eq!(names, GOLDEN_NAMES, "and these are the ones that park");
    }

    /// The invariant rule 1 of [`Parking`] rests on, asked of a real program:
    /// **every parking function value takes something effect-carrying.**
    ///
    /// It is what licenses answering a callee position by its type alone. If a
    /// future language change let a function value reach a capability by some
    /// other route — a context constructed outside `main`, a capture the
    /// checker stopped refusing — this is the row that goes red, rather than a
    /// dropped `await` in an artifact.
    #[test]
    fn every_parking_function_value_takes_something_effect_carrying() {
        let program = compile(PRECISION);
        let parking = parkability(&program);
        let loose: Vec<String> = parking
            .parking_types
            .iter()
            .filter(|t| !parking.effects.fn_takes_effect(t))
            .map(|t| format!("{t:?}"))
            .collect();
        assert!(
            loose.is_empty(),
            "these function values park and take no capability, so answering a \
             callee position by its type would miss them: {loose:?}"
        );
        assert!(
            !parking.parking_types.is_empty(),
            "the snippet does build a parking function value, or the claim above \
             is vacuous"
        );
    }

    /// A host call written as an intrinsic *node* rather than reached as an
    /// intrinsic *function* counts too — the two spellings must not disagree.
    #[test]
    fn an_inline_intrinsic_node_seeds_the_column() {
        let node = Expr::new(
            ExprKind::Intrinsic {
                name: "host.HostNetwork.fetch".to_string(),
                targs: Vec::new(),
                args: Vec::new(),
            },
            Ty::Unit,
            Span::default(),
        );
        let program = hand_built(vec![body_func("main", node)]);
        assert_eq!(parked(&program), vec![true]);
    }

    /// The same question of a real program: arithmetic waits on nothing, and a
    /// function that reads a file waits.
    #[test]
    fn a_compiled_program_agrees() {
        let src = r#"
from "platform/effect" import { Allocator, Stdout };
from "core/fs" import { FileSystemRead, Path };
from "core/fs" import * as fs;
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/path" import * as filepath;

export fn double(n: Int): Int { n * 2 }

export fn load<C: Allocator + FileSystemRead>(ctx: C, at: Path): Str {
  match (fs.readText(ctx, at)) { .Ok(text) => text, .Err(_) => "" }
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout, FileSystemRead: host.fs };
  let text = load(ctx, filepath.of(ctx, "a.txt"));
  let _ = io.println(ctx, "${text}${double(2)}").ignore();
  .Ok(())
}
"#;
        let program = compile(src);
        let parking = parkability(&program);
        assert!(
            !parking.parks(find(&program, "double").index()),
            "multiplication waits on nothing"
        );
        assert!(
            parking.parks(find(&program, "load").index()),
            "and a read of the host filesystem does"
        );
    }
}
