//! An actor driven from inside `alloc.scoped`, on every native backend built
//! in (buri-lang/buri#223).
//!
//! `core/actor` copies what it hands the runtime out of every arena, so a scope
//! can unmap its pages while an actor still holds what was posted from inside
//! it. The copy shares any block an earlier crossing already settled outside
//! every arena, so a step that hands its state back untouched costs a few
//! blocks per message rather than a copy of the whole state.
//!
//! The first row counts that work through the allocation probe rather than
//! timing it. The others hold the sharing to soundness: every value a scope
//! built and an actor kept must still read right after the scope is gone and
//! later scopes have reused its pages. Every row runs under the heap check, so
//! a block kept past its scope reads as poison and a leaked share fails the
//! exit audit.

use crate::shared::{probed, Ran};
use std::time::Duration;

/// Runs a program to its end under the heap check, or fails after a minute.
///
/// Bounded because an actor's sender waits, and a regression there is a
/// program that hangs. A hang here is a failing row with a sentence.
fn ran_bounded(binary: &std::path::Path) -> Ran {
    let mut child = crate::shared::spawned(binary);
    let status = crate::shared::waited(&mut child, Duration::from_secs(60));
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut pipe, &mut stdout);
    }
    if let Some(mut pipe) = child.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr);
    }
    Ran { status: status.code().unwrap_or(-1), stdout, stderr }
}

/// One program on every native backend built in, each with the allocation
/// probe linked in.
fn run_each(name: &str, source: &str) -> Vec<(&'static str, Ran)> {
    crate::e2e::probed_backends()
        .into_iter()
        .map(|(backend, build)| (backend, ran_bounded(&build(name, source))))
        .collect()
}

/// The issue's workload: a step that hands its state back untouched, sent
/// three hundred messages from inside `alloc.scoped`.
///
/// The state is four hundred entries, each a string of its own. Copying it is
/// at least four hundred blocks, so a copy per message is at least 120,000.
/// Without that copy the program makes about two thousand blocks to build the
/// state, one copy of it when the actor starts inside the scope, and a few
/// blocks per message for the message, the answer and the state's wrapper.
#[test]
fn a_step_that_keeps_its_state_does_not_copy_it_inside_a_scope() {
    let source = r#"
from "core/actor" import * as actor;
from "core/actor" import { Actor, Address, Stepped };
from "core/alloc" import * as alloc;
from "platform/effect" import { Allocator, Stdout, Tasks };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/orderedmap" import * as orderedmap;
from "core/orderedmap" import { OrderedMap };
from "core/str" import * as str;

fn build<C: Allocator>(ctx: C, acc: OrderedMap<Int, Str>, i: Int, n: Int): OrderedMap<Int, Str> {
    if (i >= n) { acc } else { build(ctx, acc.insert(ctx, i, str.format(ctx, "value ${i}")), i + 1, n) }
}

fn send<C: Allocator + Tasks>(ctx: C, address: Address<C, OrderedMap<Int, Str>, Int, Int>, i: Int, acc: Int): Int {
    if (i >= 300) { acc } else { send(ctx, address, i + 1, acc + address.sendMessage(ctx, i).withDefault(0)) }
}

fn measure<C: Allocator + Tasks>(ctx: C, state: OrderedMap<Int, Str>): Int {
    let address = actor.start(ctx, Actor {
        state,
        step: fn(_c, s, _message) => Stepped { state: s, answer: 1 },
    });
    let total = send(ctx, address, 0, 0);
    let _ = address.stop(ctx).isOk();
    total
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
        Tasks: host.tasks,
    };
    let state = build(ctx, orderedmap.empty(), 0, 400);
    let answered = alloc.scoped(ctx, fn(s) => measure(s, state));
    let _ = io.println(ctx, "answered ${answered} of ${state.length()}").ignore();
    .Ok(())
}
"#;
    for (backend, r) in run_each("actor-scoped-keeps-state", source) {
        assert_eq!(r.status, 0, "{backend}: stdout:\n{}\nstderr:\n{}", r.stdout, r.stderr);
        assert_eq!(r.stdout, "answered 300 of 400\n", "{backend}: {}", r.stderr);
        let (blocks, live) = probed(&r.stderr);
        assert!(
            blocks < 20_000,
            "{backend}: three hundred messages to an actor that keeps a 400-entry state made \
             {blocks} blocks inside a scope, so each crossing copied the whole state"
        );
        assert_eq!(live, 0, "{backend}: {blocks} blocks allocated and {live} still live at exit");
    }
}

/// What a scope built and an actor kept outlives the scope.
///
/// The state is settled outside every arena by the first messages, and then
/// the scope posts strings and a list it built, and the step puts values it
/// builds itself into part of the state. Some of those are bigger than an
/// arena block, so the scope unmaps their pages when it ends. The rest sit in
/// pooled blocks that the scopes after it write over. A kept pointer into
/// either would fault, read poison under the heap check, or read the wrong
/// string.
#[test]
fn what_a_scope_built_and_an_actor_kept_outlives_the_scope() {
    let source = r#"
from "core/actor" import * as actor;
from "core/actor" import { Actor, Address, Stepped };
from "core/alloc" import * as alloc;
from "core/alloc" import { Scoped };
from "platform/effect" import { Allocator, Stdout, Tasks };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/orderedmap" import * as orderedmap;
from "core/orderedmap" import { OrderedMap };
from "core/str" import * as str;

struct Held {
    names: [Str],
    groups: [[Str]],
    table: OrderedMap<Int, Str>,
}

enum Edit {
    Keep,
    Name(Str),
    Group([Str]),
    Stamp(Int),
    Get,
}

enum Edited {
    Done,
    Whole(Held),
}

fn big<C: Allocator>(ctx: C, unit: Str): Str {
    unit.repeat(ctx, 70000)
}

fn small<C: Allocator>(ctx: C, unit: Str): Str {
    unit.repeat(ctx, 30)
}

/// The value `.Stamp(k)` puts at `k`: big for an odd key, small for an even one.
fn stamp<C: Allocator>(ctx: C, k: Int): Str {
    if (k % 2 == 1) { big(ctx, str.format(ctx, "s${k}")) } else { small(ctx, str.format(ctx, "s${k}")) }
}

fn keeper<C: Allocator + Tasks>(initial: Held): Actor<C, Held, Edit, Edited> {
    Actor {
        state: initial,
        step: fn(c, held, edit) => {
            match (edit) {
                .Keep => Stepped { state: held, answer: .Done },
                .Name(s) => Stepped { state: Held { ..held, names: held.names.push(c, s) }, answer: .Done },
                .Group(g) => Stepped { state: Held { ..held, groups: held.groups.push(c, g) }, answer: .Done },
                .Stamp(k) => Stepped { state: Held { ..held, table: held.table.insert(c, k, stamp(c, k)) }, answer: .Done },
                .Get => Stepped { state: held, answer: .Whole(held) },
            }
        },
    }
}

fn original<C: Allocator>(ctx: C, k: Int): Str {
    str.format(ctx, "original ${k}")
}

fn filled<C: Allocator>(ctx: C, at: Int, n: Int, map: OrderedMap<Int, Str>): OrderedMap<Int, Str> {
    if (at >= n) { map } else { filled(ctx, at + 1, n, map.insert(ctx, at, original(ctx, at))) }
}

fn group<C: Allocator>(ctx: C): [Str] {
    [small(ctx, "g"), big(ctx, "h"), small(ctx, "i")]
}

fn sent<C: Allocator + Tasks>(ctx: C, a: Address<C, Held, Edit, Edited>, edits: [Edit], at: Int, ok: Int): Int {
    match (edits.get(at)) {
        .None => ok,
        .Some(e) => sent(ctx, a, edits, at + 1, ok + if (a.sendMessage(ctx, e).isOk()) { 1 } else { 0 }),
    }
}

struct Escaped<C> {
    scope: Scoped<C>,
    address: Address<Scoped<C>, Held, Edit, Edited>,
    answered: Int,
}

fn reads<C: Allocator>(ctx: C, table: OrderedMap<Int, Str>, k: Int): Str {
    match (table.get(k)) {
        .None => "missing",
        .Some(v) => if (v == stamp(ctx, k)) { "stamped" } else if (v == original(ctx, k)) { "original" } else { "wrong" },
    }
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
        Tasks: host.tasks,
    };
    let initial = filled(ctx, 0, 50, orderedmap.empty());
    let out = alloc.scoped(ctx, fn(c) => {
        let a = actor.start(c, keeper(Held { names: [], groups: [], table: initial }));
        let settled = sent(c, a, [.Keep, .Keep, .Keep], 0, 0);
        let edits = sent(c, a, [
            .Name(big(c, "n")),
            .Name(small(c, "m")),
            .Group(group(c)),
            .Stamp(3),
            .Stamp(4),
            .Keep,
            .Keep,
        ], 0, 0);
        Escaped { scope: c, address: a, answered: settled + edits }
    });
    let a = out.address;
    let answered = out.answered;
    // Scopes that take the released pages back and write over them.
    let churned = [1, 2, 3, 4, 5, 6, 7, 8].mapCtx(ctx, fn(k, n) => {
        alloc.scoped(k, fn(s) => "z".repeat(s, 40 + n).length())
    });
    let large = alloc.scoped(ctx, fn(s) => "y".repeat(s, 70000).length());
    let held = match (a.sendMessage(out.scope, .Get)) {
        .Ok(.Whole(h)) => h,
        _otherwise => Held { names: [], groups: [], table: orderedmap.empty() },
    };
    let names = held.names == [big(ctx, "n"), small(ctx, "m")];
    let groups = held.groups == [group(ctx)];
    let _ = io.println(ctx, "answered ${answered} churned ${churned.length()} ${large}").ignore();
    let _ = io.println(ctx, "names ${names} groups ${groups}").ignore();
    let _ = io.println(ctx, "table ${held.table.length()} ${reads(ctx, held.table, 3)} ${reads(ctx, held.table, 4)} ${reads(ctx, held.table, 10)}").ignore();
    let _ = io.println(ctx, "stopped ${a.stop(out.scope).isOk()}").ignore();
    .Ok(())
}
"#;
    for (backend, r) in run_each("actor-scoped-outlives", source) {
        assert_eq!(r.status, 0, "{backend}: stdout:\n{}\nstderr:\n{}", r.stdout, r.stderr);
        assert_eq!(
            r.stdout.lines().collect::<Vec<_>>(),
            vec![
                "answered 10 churned 8 70000",
                "names true groups true",
                "table 50 stamped stamped original",
                "stopped true",
            ],
            "{backend}: {}",
            r.stderr
        );
        let (blocks, live) = probed(&r.stderr);
        assert_eq!(live, 0, "{backend}: {blocks} blocks allocated and {live} still live at exit");
    }
}

/// Fanned-out steps inside a scope post what the scope built to an actor whose
/// state is already settled, and the actor keeps it past the scope.
///
/// Under `--release` each fanned-out step runs on a thread that is inside no
/// arena. The string it is handed lives in the scope's pages, and the list it
/// builds around that string is on the heap, so a block outside every arena
/// points into one. The crossing must copy that list and the string under it,
/// not share the list because of where the list itself lives.
#[test]
fn fanned_out_steps_inside_a_scope_post_to_a_settled_actor() {
    let source = r#"
from "core/actor" import * as actor;
from "core/actor" import { Actor, Address, Stepped };
from "core/alloc" import * as alloc;
from "core/alloc" import { Scoped };
from "platform/effect" import { Allocator, Stdout, Tasks };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/str" import * as str;
from "core/tasks" import * as tasks;

enum Keep {
    Nothing,
    Add([Str]),
    Get,
}

enum Kept {
    Added,
    Held([[Str]]),
}

fn keeper<C: Allocator + Tasks>(): Actor<C, [[Str]], Keep, Kept> {
    Actor {
        state: [],
        step: fn(c, held, message) => {
            match (message) {
                .Nothing => Stepped { state: held, answer: .Added },
                .Add(s) => Stepped { state: held.push(c, s), answer: .Added },
                .Get => Stepped { state: held, answer: .Held(held) },
            }
        },
    }
}

fn big<C: Allocator>(ctx: C, unit: Str): Str {
    unit.repeat(ctx, 70000)
}

fn built<C: Allocator>(ctx: C): [Str] {
    [0, 1, 2, 3, 4, 5, 6, 7].mapCtx(ctx, fn(k, n) => {
        if (n % 2 == 0) { big(k, str.format(k, "${n}")) } else { str.format(k, "small ${n}") }
    })
}

fn wrapped<C: Allocator>(ctx: C, item: Str): [Str] {
    [item, str.format(ctx, "${item.length()}")]
}

struct Escaped<C> {
    scope: Scoped<C>,
    address: Address<Scoped<C>, [[Str]], Keep, Kept>,
    seeded: Int,
}

fn seeding<C: Allocator + Tasks>(ctx: C, address: Address<C, [[Str]], Keep, Kept>, left: Int, ok: Int): Int {
    if (left <= 0) { ok } else {
        seeding(ctx, address, left - 1, ok + if (address.sendMessage(ctx, .Add(wrapped(ctx, "seed"))).isOk()) { 1 } else { 0 })
    }
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
        Tasks: host.tasks,
    };
    // The first scope starts the actor and settles its state.
    let out = alloc.scoped(ctx, fn(c) => {
        let address = actor.start(c, keeper());
        Escaped { scope: c, address: address, seeded: seeding(c, address, 3, 0) }
    });
    let address = out.address;
    let first = out.seeded;
    let posted = alloc.scoped(ctx, fn(c) => {
        let fanned = tasks.parallel(c, built(c), fn(c2, _i, item) => {
            address.sendMessage(c2, .Add(wrapped(c2, item))).isOk()
        });
        let kept = address.sendMessage(c, .Nothing).isOk();
        fanned.count(fn(ok) => ok) + if (kept) { 1 } else { 0 }
    });
    // Scopes that take the released pages back, small ones first.
    let churned = [1, 2, 3, 4, 5, 6, 7, 8].mapCtx(ctx, fn(k, n) => {
        alloc.scoped(k, fn(s) => "z".repeat(s, 40 + n).length())
    });
    let large = alloc.scoped(ctx, fn(s) => "y".repeat(s, 70000).length());
    let held = match (address.sendMessage(out.scope, .Get)) {
        .Ok(.Held(list)) => list,
        _otherwise => [],
    };
    let want = built(ctx).mapCtx(ctx, fn(k, w) => wrapped(k, w));
    let seed = wrapped(ctx, "seed");
    let seeds = held.count(fn(h) => h == seed);
    let _ = io.println(ctx, "first ${first} posted ${posted} churned ${churned.length()} ${large}").ignore();
    let _ = io.println(ctx, "kept ${want.count(fn(w) => held.any(fn(h) => h == w))} of ${held.length()} seeds ${seeds}").ignore();
    let _ = io.println(ctx, "stopped ${address.stop(out.scope).isOk()}").ignore();
    .Ok(())
}
"#;
    for (backend, r) in run_each("actor-scoped-fanned-out", source) {
        assert_eq!(r.status, 0, "{backend}: stdout:\n{}\nstderr:\n{}", r.stdout, r.stderr);
        assert_eq!(
            r.stdout.lines().collect::<Vec<_>>(),
            vec!["first 3 posted 9 churned 8 70000", "kept 8 of 11 seeds 3", "stopped true"],
            "{backend}: {}",
            r.stderr
        );
        let (blocks, live) = probed(&r.stderr);
        assert_eq!(live, 0, "{backend}: {blocks} blocks allocated and {live} still live at exit");
    }
}
