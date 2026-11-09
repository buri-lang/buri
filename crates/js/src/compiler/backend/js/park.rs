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
use crate::compiler::semantics::layered::Dense;
use crate::compiler::semantics::typed::{self, Expr, ExprKind, Stmt};
use crate::compiler::semantics::types::{FuncIdx, LocalId, Ty, TyConId, TyKind};
use crate::diagnostics::Span;
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
        // `Listen` is absent because the checker refuses it on this backend
        // (`effect-not-on-backend`). Both client operations wait:
        // `connectSocket` resolves a name, opens a connection and finishes a
        // handshake, and `connectReceive` is where a client sits between
        // messages. By prefix for the filesystem's reason. `host.HostSockets`
        // is deliberately absent: a frame is enqueued rather than delivered,
        // which is the whole of what `socketSendText` promises.
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
///    argument it can receive is written at a `CallFunc` or a `Continue` naming
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
/// A struct field is the exception: a call through `hooks.seen` is worth the
/// values written into `seen` of that struct type, by every struct literal
/// and update, rather than every value of its type. A `ui.each` row that runs
/// a hook it was handed does not wait because a press handler of the same
/// type does. This rests on every struct being built by a literal or an
/// update in the program: the runtime hands structs back and forth but never
/// builds one around a function the program made.
///
/// **What is given up**, recorded rather than hidden: a parking function value
/// that shares a type with a callee position it can never reach makes that call
/// parkable. Enum payloads, array and map elements and returns stay under the
/// type: the runtime builds `Option`s and lists out of each other
/// (`$list_get`, `map`), so keying them by container would mean following the
/// runtime too.
///
/// [`ExprKind::CallTrait`] stays conservative. Monomorphization resolves every
/// one it can, and one that survives is a program the backends already turn
/// into an abort.
///
/// # The call site
///
/// A function handed a function value is only as parkable as the value it
/// was handed *at that call*. `fn tap<C>(ctx: C, f: fn(C) => ()) { f(ctx) }`
/// called from a handler with a callback that sleeps, and from a `ui.each`
/// row with one that only reads, is one instantiation, so its parameters
/// hold the union of both arguments. Printing that one body `async` makes
/// the row `async` too, and the runtime that builds the row gets a promise
/// where it wanted a node (the class of buri-lang/buri#325).
///
/// So each slot has two rows: [`Parking::parks`], over every argument any
/// call hands it, and the **sync** row, over a call whose function-typed
/// arguments none park. A call site takes the second whenever its own
/// arguments allow, and where the two rows differ the emitter prints the
/// slot twice: the `async` body, and a plain twin ([`Parking::twin`]) that
/// such a call names instead.
#[derive(Clone, Debug, Default)]
pub struct Parking {
    /// One row per [`Program::funcs`] slot: whether it can park, over every
    /// argument any call hands it.
    pub parks: Dense<FuncIdx, bool>,
    /// The same, for a call none of whose function-typed arguments park.
    parks_sync: Dense<FuncIdx, bool>,
    /// Per slot, the locals whose function value may park.
    parking: Dense<FuncIdx, LocalSet>,
    /// The same, in the sync body, where no parameter parks.
    parking_sync: Dense<FuncIdx, LocalSet>,
    /// Per slot, the locals whose function value this pass followed.
    resolved: Dense<FuncIdx, LocalSet>,
    /// Per slot, its parameters: followed in every sync body, since the sync
    /// body is only ever entered with arguments that do not park.
    params: Dense<FuncIdx, LocalSet>,
    /// The types of the parking function values this program builds.
    pub parking_types: HashSet<Ty>,
    /// The struct fields, by struct type and field index, that some literal
    /// or update writes a parking function value into.
    parking_fields: HashSet<(Ty, usize)>,
    /// The function values the page asks for an answer at once but which
    /// wait ([`answers_too_late`]).
    pub waits: Vec<Span>,
    /// [`monomorphize::Program::shapes`]'s effect table, so rule 1 above can be
    /// asked without a `Tables`.
    pub effects: monomorphize::Effects,
}

/// Which of a slot's two bodies is being asked about: `sync` is the body a
/// call enters when none of its function-typed arguments park.
#[derive(Clone, Copy, Debug)]
pub struct In {
    pub slot: FuncIdx,
    pub sync: bool,
}

/// What one call does: whether it is awaited, and whether it names the
/// callee's plain twin rather than its `async` body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    pub parks: bool,
    pub twin: bool,
}

impl Parking {
    /// Whether the function in this slot, or anything it calls, can reach a
    /// host operation that blocks, over every argument any call hands it.
    pub fn parks(&self, index: FuncIdx) -> bool {
        *self.parks.get(index)
    }

    /// Whether this slot is printed twice: an `async` body and a plain twin,
    /// for a call none of whose function-typed arguments park.
    pub fn twin(&self, index: FuncIdx) -> bool {
        self.parks(index) && !*self.parks_sync.get(index)
    }

    /// Whether the body this asks about is printed `async`.
    pub fn body_is_async(&self, at: In) -> bool {
        if at.sync {
            *self.parks_sync.get(at.slot)
        } else {
            self.parks(at.slot)
        }
    }

    /// A call to `callee` with these arguments, written in the body `at`.
    ///
    /// A call with a parking function value among its arguments enters the
    /// `async` body; every other call enters the sync one, which is the twin
    /// where the two differ and the one body where they do not.
    ///
    /// A callee that never parks answers without reading the arguments: its
    /// sync row is never above its own (see [`parkability`]), so both bodies
    /// are the one plain body.
    pub fn call(&self, at: In, callee: FuncIdx, args: &[Expr]) -> Call {
        if !self.parks(callee) {
            return Call {
                parks: false,
                twin: false,
            };
        }
        let handed = args
            .iter()
            .any(|a| is_fn_ty(&a.ty) && self.value_parks(at, a));
        if handed {
            Call {
                parks: self.parks(callee),
                twin: false,
            }
        } else {
            Call {
                parks: *self.parks_sync.get(callee),
                twin: self.twin(callee),
            }
        }
    }

    /// Whether a call **through** the function value this expression produces
    /// can park.
    ///
    /// `at` is the body the expression is written in, because a local means
    /// nothing without one.
    pub fn value_parks(&self, at: In, e: &Expr) -> bool {
        // Rule 1: the type. Asked first because it holds whatever else the
        // program contains, and because it is the common case — a comparator,
        // a predicate, a `fn(A) => B` — answered without looking anything up.
        if !self.effects.fn_takes_effect(&e.ty) {
            return false;
        }
        match &e.kind {
            // A function named as a value is its `async` body: nothing at the
            // call through it can say which arguments it will get.
            ExprKind::FuncRef(c) => self.parks(*c),
            ExprKind::Closure { func, .. } => self.parks(*func),
            ExprKind::Lambda { body, .. } => self.body_parks(at, body),
            ExprKind::Local(l) => {
                let parking = if at.sync {
                    &self.parking_sync
                } else {
                    &self.parking
                };
                if parking.get(at.slot).contains(*l) {
                    true
                } else if self.resolved.get(at.slot).contains(*l)
                    || (at.sync && self.params.get(at.slot).contains(*l))
                {
                    false
                } else {
                    self.unfollowed(&e.ty)
                }
            }
            // A function value is whatever the branch that produced it is.
            ExprKind::Block { tail: Some(t), .. } => self.value_parks(at, t),
            ExprKind::If { then, else_, .. } => {
                self.value_parks(at, then) || self.value_parks(at, else_)
            }
            ExprKind::Match { arms, .. } => arms.iter().any(|a| self.value_parks(at, &a.body)),
            // A function value out of a struct field is one some literal or
            // update wrote there.
            ExprKind::Field { base, index } => {
                self.parking_fields.contains(&(base.ty, index.index()))
            }
            _ => self.unfollowed(&e.ty),
        }
    }

    /// The answer for a function value this pass could not follow: whether the
    /// program builds a parking one of the same type. Every position asked
    /// about is a callee, an argument or a `let` of function type, so the type
    /// is always one to ask under.
    fn unfollowed(&self, ty: &Ty) -> bool {
        self.parking_types.contains(ty)
    }

    /// Whether running this body reaches something that waits.
    ///
    /// The walk stops at a lambda: building one runs none of its body. The
    /// lambda is printed as its own arrow, `async` when an `await` lands in
    /// it, and `has_await` does not look inside a nested arrow either. A
    /// function that only *holds* a handler that navigates is not `async`, so
    /// a `ui.each` row built by one hands back a node rather than a promise
    /// (buri-lang/buri#325). A call *through* a lambda asks
    /// [`Parking::value_parks`], which reads its body.
    fn body_parks(&self, at: In, body: &Expr) -> bool {
        let mut k = false;
        self.walk_runs(at, body, &mut k);
        k
    }

    /// [`Parking::body_parks`]'s walk, outermost first, skipping lambda
    /// bodies.
    fn walk_runs(&self, at: In, e: &Expr, k: &mut bool) {
        if *k {
            return;
        }
        match &e.kind {
            ExprKind::Lambda { .. } => return,
            ExprKind::CallFunc { func, args } => *k = self.call(at, *func, args).parks,
            // A jump into another function's loop is a call.
            ExprKind::Continue {
                func: Some(c),
                args,
                ..
            } => *k = self.call(at, *c, args).parks,
            // A jump back to the top of this body's own loop rebinds its
            // parameters, and the sync body cannot turn into the `async` one
            // halfway round. A jump that hands a parameter a parking value
            // makes the two bodies one.
            ExprKind::Continue {
                func: None, args, ..
            } if at.sync => {
                if args
                    .iter()
                    .any(|a| is_fn_ty(&a.ty) && self.value_parks(at, a))
                {
                    *k = self.parks(at.slot);
                }
            }
            ExprKind::CallValue { callee, .. } => *k = self.value_parks(at, callee),
            ExprKind::CallTrait { .. } => *k = true,
            // A host call spelled as an inline node rather than reached
            // through an intrinsic *function*; the same seed answers both.
            ExprKind::Intrinsic { name, .. } => *k = suspends(name),
            _ => {}
        }
        typed::children(e, &mut |c| self.walk_runs(at, c, k));
    }
}

/// Whether a type is a function type, which is the only kind of value a
/// [`ExprKind::CallValue`] can reach and so the only kind [`Parking`] tracks.
fn is_fn_ty(t: &Ty) -> bool {
    matches!(t.kind(), TyKind::Fn(..))
}

/// A set of one function's locals, one bit per [`LocalId`]: dense, because a
/// function's locals are numbered from zero.
#[derive(Clone, Debug, Default)]
struct LocalSet(Vec<u64>);

impl LocalSet {
    fn contains(&self, l: LocalId) -> bool {
        let i = l.index();
        self.0.get(i / 64).is_some_and(|w| (w >> (i % 64)) & 1 == 1)
    }

    /// Adds `l`, answering whether it was not already there.
    fn insert(&mut self, l: LocalId) -> bool {
        let i = l.index();
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

/// The `async` body of a slot.
fn full(slot: FuncIdx) -> In {
    In { slot, sync: false }
}

/// The sync body of a slot.
fn sync(slot: FuncIdx) -> In {
    In { slot, sync: true }
}

/// [`Parking`], computed. `tree` names the type constructors of the
/// user interface tree the page reads, for [`Parking::waits`].
pub fn parkability(program: &Program, tree: impl Fn(TyConId) -> bool) -> Parking {
    let mut w = Parking {
        // An unbuilt body is lowered to an abort, which is the one thing that
        // certainly does not wait, so only a suspending intrinsic seeds `true`.
        parks: program.funcs.map(|f| match &f.kind {
            // A method a platform's `js` file implements suspends when the
            // file writes it `async`, which is the one thing a file can say
            // about it. One written without answers at once, and a caller the
            // runtime calls without waiting, such as a `ui.each` row, has to
            // stay a function that answers a value (buri-lang/buri#330).
            FuncKind::Intrinsic(key) => {
                let hosted = &program.hosted;
                suspends(key)
                    || (hosted.js_implemented.contains(key) && !hosted.synchronous.contains(key))
            }
            FuncKind::Unbuilt | FuncKind::Body(_) => false,
        }),
        parks_sync: Dense::new(),
        parking: program.funcs.map(|_| LocalSet::default()),
        parking_sync: program.funcs.map(|_| LocalSet::default()),
        resolved: program.funcs.map(|_| LocalSet::default()),
        params: program.funcs.map(|f| {
            let mut set = LocalSet::default();
            for p in &f.params {
                set.insert(*p);
            }
            set
        }),
        parking_types: HashSet::default(),
        parking_fields: HashSet::default(),
        waits: Vec::new(),
        effects: program.shapes.effects.clone(),
    };
    // The sync row starts from the same seed: what a key waits on is its own.
    w.parks_sync = w.parks.clone();

    // One walk of every body collects what the loop below reads, none of
    // which moves:
    //
    // - arguments, with the parameters they are bound to;
    // - every node that *is* a function value — an `FuncRef`, a `Closure` or
    //   a `Lambda` — with the slot it is written in;
    // - what a struct literal or update writes into each field of function
    //   type, with the struct's type and the field's index;
    // - every value written into a `tree` value, for [`answers_too_late`];
    // - the slots whose **address is taken**: named by an `FuncRef` or a
    //   `Closure` rather than called. One of these can be called from a
    //   position no `CallFunc` names, so nothing can be proved about what its
    //   parameters hold. Every other slot receives arguments only where this
    //   pass can see them;
    // - each body's `let`s that bind a single name to a single function
    //   value. A destructuring pattern is deliberately not one: the local it
    //   binds stays unresolved, and an unresolved fn-typed local is worth
    //   what [`Parking::unfollowed`] says of its type.
    let mut edges: Vec<(FuncIdx, FuncIdx, &Vec<Expr>)> = Vec::new();
    let mut values: Vec<(FuncIdx, &Expr)> = Vec::new();
    let mut writes: Vec<(FuncIdx, (Ty, usize), &Expr)> = Vec::new();
    let mut written: Vec<(FuncIdx, &Expr)> = Vec::new();
    let mut addressed = program.funcs.map(|_| false);
    let mut lets: Lets = program.funcs.map(|_| Vec::new());
    for (i, f) in program.funcs.entries() {
        let Some(body) = f.body() else { continue };
        let mut binds = Vec::new();
        typed::walk(body, &mut |e| match &e.kind {
            ExprKind::StructLit { con, fields, .. } => {
                if tree(*con) {
                    written.extend(fields.iter().map(|v| (i, v)));
                }
                for (k, v) in fields.iter().enumerate() {
                    if is_fn_ty(&v.ty) {
                        writes.push((i, (e.ty, k), v));
                    }
                }
            }
            ExprKind::StructUpdate { con, updates, .. } => {
                if tree(*con) {
                    written.extend(updates.iter().map(|(_, v)| (i, v)));
                }
                for (k, v) in updates {
                    if is_fn_ty(&v.ty) {
                        writes.push((i, (e.ty, k.index()), v));
                    }
                }
            }
            ExprKind::EnumLit { con, args, .. } if tree(*con) => {
                written.extend(args.iter().map(|v| (i, v)));
            }
            ExprKind::CallFunc { func, args } => edges.push((*func, i, args)),
            // A `Continue` rebinds the parameters of the function it enters, in
            // order — the dispatch index the backend prepends is not one of
            // them.
            ExprKind::Continue { func, args, .. } => {
                edges.push((func.unwrap_or(i), i, args));
            }
            ExprKind::FuncRef(c) | ExprKind::Closure { func: c, .. } => {
                *addressed.get_mut(*c) = true;
                values.push((i, e));
            }
            ExprKind::Lambda { .. } => values.push((i, e)),
            ExprKind::Block { stmts, .. } => {
                for s in stmts {
                    let Stmt::Let { pattern, value, .. } = s else {
                        continue;
                    };
                    let typed::PatKind::Bind { local, sub: None } = &pattern.kind else {
                        continue;
                    };
                    if is_fn_ty(&value.ty) {
                        binds.push((*local, value));
                    }
                }
            }
            _ => {}
        });
        *lets.get_mut(i) = binds;
    }

    // What can be followed, decided once.
    for (((f, slot), addressed), lets) in program
        .funcs
        .iter()
        .zip(w.resolved.iter_mut())
        .zip(&addressed)
        .zip(&lets)
    {
        if !*addressed {
            for p in &f.params {
                slot.insert(*p);
            }
        }
        for (local, _) in lets {
            slot.insert(*local);
        }
    }

    // Monotone in every column — a row only ever climbs — so the loop
    // terminates in at most one pass per edge.
    let mut changed = true;
    while changed {
        changed = false;

        for &(target, from, args) in &edges {
            let tf = program.funcs.get(target);
            // A call passes one argument per parameter, and so does a jump.
            for (a, &p) in args.iter().zip(&tf.params) {
                if !is_fn_ty(&a.ty) || !w.value_parks(full(from), a) {
                    continue;
                }
                if w.parking.get_mut(target).insert(p) {
                    changed = true;
                }
            }
        }

        // The `let`s that hold one, in both bodies. The sync body sees no
        // more parking values than the `async` one, so a `let` that does not
        // park there does not park in the sync body either.
        for (i, binds) in lets.entries() {
            for (local, value) in binds {
                if !w.value_parks(full(i), value) {
                    continue;
                }
                if w.parking.get_mut(i).insert(*local) {
                    changed = true;
                }
                if w.value_parks(sync(i), value) && w.parking_sync.get_mut(i).insert(*local) {
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
                ExprKind::FuncRef(c) => w.parks(*c),
                ExprKind::Closure { func, .. } => w.parks(*func),
                ExprKind::Lambda { body, .. } => w.body_parks(full(i), body),
                _ => false,
            };
            if parks {
                found.push(&e.ty);
            }
        }
        for ty in found {
            if w.parking_types.insert(*ty) {
                changed = true;
            }
        }
        for &(i, key, v) in &writes {
            if w.value_parks(full(i), v) && w.parking_fields.insert(key) {
                changed = true;
            }
        }

        // And the column itself. A function that *is* a suspending intrinsic
        // has no body and is never reached here; one with a body starts at
        // `false` and only ever climbs, so the seed is not lost.
        for (i, f) in program.funcs.entries() {
            if !*w.parks.get(i) && parks_here(&w, i, f) {
                *w.parks.get_mut(i) = true;
                changed = true;
            }
            // The sync body, where no parameter parks: a combinator that runs
            // a step is only its own seed there. It sees no more parking
            // values than the `async` body, so it is asked only once that one
            // parks, and the sync row never rises above the other.
            if *w.parks.get(i) && !*w.parks_sync.get(i) {
                if let FuncKind::Body(body) = &f.kind {
                    if w.body_parks(sync(i), body) {
                        *w.parks_sync.get_mut(i) = true;
                        changed = true;
                    }
                }
            }
        }
    }
    w.waits = answers_too_late(program, written, &edges, &lets, &w);
    w
}

/// Whether the `async` body of slot `i`, `f`, parks as [`parkability`] has
/// it so far.
fn parks_here(w: &Parking, i: FuncIdx, f: &monomorphize::Func) -> bool {
    // A combinator that **runs a step it was handed the context for** waits
    // exactly when that step waits, and no seed can say which: `list.mapCtx`
    // over a rendering is a loop, and over a socket dial it is a wait. So the
    // answer is read off the argument edges the loop already walked — the step
    // that actually arrived at this instantiation — and a key with a parking
    // step joins the column beside the keys whose wait is their own.
    //
    // Per instantiation, so `mapCtx` at one element type does not colour
    // `mapCtx` at another. Two call sites that share an instantiation and
    // disagree do not share the answer: the one whose step does not park
    // calls the plain twin, whose sync row is the seed alone (see "The call
    // site").
    match &f.kind {
        FuncKind::Intrinsic(key) => {
            intrinsic_keys::ctx_step_key(key)
                && f.params.iter().any(|p| w.parking.get(i).contains(*p))
        }
        FuncKind::Body(body) => w.body_parks(full(i), body),
        FuncKind::Unbuilt => false,
    }
}

/// The function values the page asks for an answer at once but which wait,
/// each as the span to blame: in the program's own code wherever the wait
/// came from there.
///
/// The page calls two kinds of function inside a call of its own and reads
/// the answer straight away. It builds a `ui.each` or `ui.rebuild` row while
/// it draws the list, and asks an `onKey` whether it claimed the key while
/// the browser is still dispatching it. A row that waits hands back a promise
/// where a node belongs, and an `onKey` that waits claims its key after the
/// browser has handled it. A page that drew and handled keys later than
/// asked would be a different page, so the program is refused instead
/// (`answer-waits`).
///
/// Both are a function written into a `ui/node` value whose type takes a
/// context and answers something other than `()`, and that is the rule. The
/// page runs a function answering `()`, such as a press handler, through
/// `$ui_flush`, which waits for whatever it hands back. One that takes no
/// context can't wait at all (rule 1).
///
/// A function that writes the row it was handed, as `ui.each` does, waits
/// only when a caller hands it one that waits. So the blame follows that
/// argument back, through every function that only passed it on, to the
/// function value that waits itself.
fn answers_too_late<'p>(
    program: &'p Program,
    written: Vec<(FuncIdx, &'p Expr)>,
    edges: &[(FuncIdx, FuncIdx, &'p Vec<Expr>)],
    lets: &Lets<'p>,
    w: &Parking,
) -> Vec<Span> {
    let answers = |t: Ty| match t.kind() {
        TyKind::Fn(_, ret) => *ret.kind() != TyKind::Unit && w.effects.fn_takes_effect(&t),
        _ => false,
    };
    // The callers only matter to a value that waits, which almost no program
    // writes, so they are built when the first one turns up.
    let mut searched: Option<Callers> = None;
    let mut blamed = Vec::new();
    for (i, v) in written {
        // An optional handler is asked about through the `.Some` that holds
        // it. One that arrives already wrapped is worth what its type is, and
        // is asked about where the program's own code writes it, since a
        // library only passes on what it was handed.
        let v = match (&v.kind, v.ty.kind()) {
            (ExprKind::EnumLit { args, .. }, TyKind::Con(con, _))
                if Some(*con) == program.shapes.option =>
            {
                match args.first() {
                    Some(handler) => handler,
                    None => continue,
                }
            }
            (_, TyKind::Con(con, [handler])) if Some(*con) == program.shapes.option => {
                if answers(*handler)
                    && !v.span.file.is_standard()
                    && w.parking_types.contains(handler)
                {
                    blamed.push(v.span);
                }
                continue;
            }
            _ => v,
        };
        if !answers(v.ty) || !w.value_parks(full(i), v) {
            continue;
        }
        let callers = searched.get_or_insert_with(|| {
            let mut callers: Callers = program.funcs.map(|_| Vec::new());
            for &(target, from, args) in edges {
                callers.get_mut(target).push((from, args));
            }
            callers
        });
        let mut blame = Blame {
            program,
            w,
            callers,
            lets,
            seen: HashSet::default(),
            blamed: &mut blamed,
        };
        blame.value(i, v);
    }
    let mut once = HashSet::default();
    blamed.retain(|s| once.insert(*s));
    blamed
}

/// Each function's calls to it, with their arguments.
type Callers<'p> = Dense<FuncIdx, Vec<(FuncIdx, &'p Vec<Expr>)>>;
/// Each function's `let`s of a single name to a single function value.
type Lets<'p> = Dense<FuncIdx, Vec<(LocalId, &'p Expr)>>;

/// [`answers_too_late`]'s search for the function values to blame.
struct Blame<'a, 'p> {
    program: &'p Program,
    w: &'a Parking,
    callers: &'a Callers<'p>,
    lets: &'a Lets<'p>,
    seen: HashSet<FuncIdx>,
    blamed: &'a mut Vec<Span>,
}

impl Blame<'_, '_> {
    /// Blames the function value `v`, written in `slot`, if it waits. Where it
    /// waits only on a parameter, the blame goes to what each call handed
    /// that parameter, and where it waits on a `let`, to what the `let` holds.
    /// So it lands on the function value that waits itself, wherever a helper
    /// passed it on or was inlined.
    fn value(&mut self, slot: FuncIdx, v: &Expr) {
        if !self.w.value_parks(full(slot), v) {
            return;
        }
        let before = self.blamed.len();
        if self.w.value_parks(sync(slot), v) {
            for (local, held) in self.lets.get(slot) {
                if self.w.parking_sync.get(slot).contains(*local) && reads(v, *local) {
                    self.value(slot, held);
                }
            }
        } else if !self.seen.insert(slot) {
            // A loop back into a function already being followed: its other
            // callers are already being asked.
            return;
        } else {
            let params = handed(self.program, self.w, slot, v);
            for &(from, args) in self.callers.get(slot) {
                for a in params.iter().filter_map(|&k| args.get(k)) {
                    self.value(from, a);
                }
            }
        }
        if self.blamed.len() == before {
            self.blamed.push(v.span);
        }
    }
}

/// Whether `e` reads `local` anywhere, a lambda's body included.
fn reads(e: &Expr, local: LocalId) -> bool {
    let mut found = false;
    typed::walk(e, &mut |x| {
        found |= matches!(x.kind, ExprKind::Local(l) if l == local)
    });
    found
}

/// The positions of the parameters of `slot` that `e` reads and that a call
/// hands a function value that waits.
fn handed(program: &Program, w: &Parking, slot: FuncIdx, e: &Expr) -> Vec<usize> {
    let params = &program.funcs.get(slot).params;
    let mut out = Vec::new();
    typed::walk(e, &mut |x| {
        if let ExprKind::Local(l) = x.kind {
            if let Some(k) = params.iter().position(|p| *p == l) {
                if w.parking.get(slot).contains(l) {
                    out.push(k);
                }
            }
        }
    });
    out
}
