//! The JavaScript backend's `park` on whole snippets, through the real front
//! end. Here rather than beside the pass because a snippet compiles through
//! `driver`, which is in `buri`.

use super::rc::{compile, find};
use crate::compiler::backend::js::park::*;
use crate::compiler::middle::monomorphize::{self, Func, FuncKind, Program};
use crate::compiler::semantics::typed::{self, Expr, ExprKind};
use crate::compiler::semantics::types::{self, FuncIdx, LocalId, Ty};
use crate::diagnostics::Span;
use crate::hash::Map as HashMap;

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
        ret: Ty::UNIT,
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
        ret: Ty::UNIT,
        desc: None,
        span: Span::default(),
    }
}

fn call_to(to: u32) -> Expr {
    Expr::new(
        ExprKind::CallFn { func: typed::Callee::Func(FuncIdx(to)), args: Vec::new() },
        Ty::UNIT,
        Span::default(),
    )
}

fn fn_ty(params: Vec<Ty>, ret: Ty) -> Ty {
    Ty::func(params, ret)
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
        Ty::UNIT,
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
        Ty::UNIT,
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
        inline_animations: false,
        icons: false,
        tooltips: false,
        themes: false,
        chunks: Vec::new(),
        hosted: Default::default(),
        instances: Default::default(),
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
    let plain = fn_ty(vec![Ty::UNIT], Ty::UNIT);
    let carrying = fn_ty(vec![Ty::ctx(types::CtxTypeId(0))], Ty::UNIT);
    let program = hand_built(vec![
        body_func("main", calls(&[1, 2])),
        body_func("through a plain callback", call_value(call_to_ty(3, plain))),
        body_func(
            "through a callback taking the context",
            call_value(call_to_ty(3, carrying)),
        ),
        body_func("maker", Expr::new(ExprKind::Unit, Ty::UNIT, Span::default())),
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
        Ty::UNIT,
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
