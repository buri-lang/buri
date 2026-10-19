//! A `core/tasks` scope in a test that schedules through a `Tasks` of its own
//! runs as the program does, on every native backend built in.
//!
//! The `TestTasks` double promises that nothing runs at once, so a block that
//! makes one runs a scope's tasks after its body. A block that binds its own
//! `Tasks` makes no such promise, and its scopes run their tasks beside their
//! bodies, as the same code does in a native program. The program and the test
//! below print the order the scheduler handed the tasks out in, and the two
//! must agree.
//!
//! JavaScript runs a scope's tasks after its body everywhere, so it isn't run
//! here.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace(name: &str) -> PathBuf {
    crate::sweep::once();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-own-scheduler-{}", std::process::id()))
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(at: &Path, body: &str) {
    if let Some(dir) = at.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(at, body).unwrap();
}

/// A scheduler that runs two steps in order and three in reverse, and a scope
/// that spawns three tasks under it.
///
/// Where a scope runs its tasks after its body, it hands the scheduler all
/// three at once, so they run `c`, `b`, `a`. Where it runs them beside the
/// body, its workers take each task as it was spawned, two steps at a time, so
/// they run `a`, `b`, `c`.
const SCHEDULER: &str = r#"from "core/io" import * as io;
from "core/tasks" import * as tasks;
from "platform/effect" import { Allocator, Stdout, Tasks };

export struct Pairs {}

impl Tasks for Pairs {
    fn parallel<C, A, B>(self, ctx: C, items: [A], f: fn(C, Int, A) => B): [B] {
        match (items) {
            [] => [],
            [a] => [f(ctx, 0, a)],
            [a, b] => [f(ctx, 0, a), f(ctx, 1, b)],
            [a, b, c, ..] => {
                let third = f(ctx, 2, c);
                let second = f(ctx, 1, b);
                [f(ctx, 0, a), second, third]
            },
        }
    }
}

fn say<C: Stdout>(ctx: C, what: Str): () {
    let _ = io.println(ctx, what).ignore();
    ()
}

/// Spawns three tasks into one scope, after the body has said so.
export fn spawnThree<C: Allocator + Stdout + Tasks>(ctx: C): () {
    tasks.scope(ctx, fn(c, here) => {
        let _ = say(c, "body");
        let _ = tasks.spawn(c, here, fn(c2) => say(c2, "a"));
        let _ = tasks.spawn(c, here, fn(c2) => say(c2, "b"));
        let _ = tasks.spawn(c, here, fn(c2) => say(c2, "c"));
        ()
    })
}
"#;

#[test]
fn a_test_with_its_own_scheduler_runs_a_scope_as_the_program_does() {
    let repo = workspace("pairs");
    write(&repo.join("REPO.buri"), "");
    write(
        &repo.join("lib/pairs/BUILD.buri"),
        "library {\n    sources: [\"pairs.buri\"]\n\n    test {\n        \
         sources: [\"test/pairs.buri\"]\n        backends: [NATIVE]\n    }\n}\n",
    );
    write(
        &repo.join("lib/pairs/lib.buri"),
        "from \"//lib/pairs/pairs.buri\" export { Pairs, spawnThree };\n",
    );
    write(&repo.join("lib/pairs/pairs.buri"), SCHEDULER);
    write(
        &repo.join("lib/pairs/test/pairs.buri"),
        r#"from "core/testing/assert" import * as assert;
from "platform/effect" import { Allocator, Stdout, Tasks };
from "platform/effect/testing" import { alloc, stdout };
from "//lib/pairs" import { Pairs, spawnThree };

test "each task is handed out as it was spawned" {
    let out = stdout();
    let ctx = context {
        Allocator: alloc(),
        Stdout: out,
        Tasks: Pairs {},
    };
    spawnThree(ctx);
    assert.equal(out.captured(), "body\na\nb\nc\n");
}
"#,
    );
    write(
        &repo.join("cmd/pairs/BUILD.buri"),
        "binary {\n    outputs: [\n        { platform: \"native\" },\n    ]\n}\n",
    );
    write(
        &repo.join("cmd/pairs/main.buri"),
        r#"from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout, Tasks };
from "//lib/pairs" import { Pairs, spawnThree };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
        Tasks: Pairs {},
    };
    let _ = spawnThree(ctx);
    .Ok(())
}
"#,
    );

    for (backend, flags) in crate::e2e::build_modes() {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
        cmd.current_dir(&repo)
            .arg("run")
            .args(flags)
            .arg("//cmd/pairs");
        let ran = crate::shared::ran_command(crate::shared::heap_checked(&mut cmd));
        assert_eq!(
            (ran.status, ran.stdout.as_str()),
            (0, "body\na\nb\nc\n"),
            "{backend}: the program:\n{}",
            ran.stderr
        );

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
        cmd.current_dir(&repo)
            .arg("test")
            .args(flags)
            .arg("//lib/pairs");
        let ran = crate::shared::ran_command(crate::shared::heap_checked(&mut cmd));
        assert!(
            ran.status == 0 && ran.stdout.contains("1 passed, 0 failed"),
            "{backend}: the test:\n{}\n{}",
            ran.stdout,
            ran.stderr
        );
    }
}
