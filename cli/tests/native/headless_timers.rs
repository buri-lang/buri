//! `platform/effect`'s `after` under `headless()`, on JavaScript and on every
//! native backend this toolchain has built in (buri-lang/buri#256).
//!
//! `elapse` moves a virtual clock and fires what came due: in due order, ties
//! in the order they were scheduled, a timer scheduled by a firing timer joining
//! the same pass when it too is due, and each on its own graph turn. Every
//! backend must agree on all of it.
//!
//! Each run is a real `buri test` over a repository, under the heap check.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace(name: &str) -> PathBuf {
    crate::sweep::once();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-headless-timers-{}", std::process::id()))
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

#[test]
fn headless_timers_fire_on_the_virtual_clock() {
    let repo = workspace("timers");
    write(&repo.join("REPO.buri"), "");
    write(
        &repo.join("timer/BUILD.buri"),
        "library {\n    sources: [\"timer.buri\"]\n\n    test {\n        \
         sources: [\"test/timer_test.buri\"]\n    }\n}\n",
    );
    write(&repo.join("timer/lib.buri"), "from \"//timer/timer.buri\" export { flash };\n");
    // The issue's repro: a library that schedules on the graph.
    write(
        &repo.join("timer/timer.buri"),
        r#"from "core/time" import * as time;
from "platform/effect" import { after, Ui };
from "ui/signal" import { Signal };

// Sets the signal to true now and back to false 100 ms later.
export fn flash<C: Ui>(ctx: C, shown: Signal<Bool>): () {
    let _ = shown.set(ctx, true);
    let _ = after(ctx, time.milliseconds(100), fn(c) => shown.set(c, false));
}
"#,
    );
    write(
        &repo.join("timer/test/timer_test.buri"),
        r#"from "core/fs" import { FileSystemRead, readText };
from "core/path" import * as path;
from "core/testing/assert" import * as assert;
from "core/time" import * as time;
from "platform/effect" import { after, Allocator, cancel, Ui, Watch };
from "platform/effect/testing" import { alloc, elapse, fs, headless, observer, recorder };
from "ui/signal" import { signal, watch };
from "//timer" import { flash };

test "the flag drops back after 100 ms" {
    let ctx = context { Ui: headless(), Watch: observer() };
    let shown = signal(ctx, false);
    flash(ctx, shown);
    assert.equal(shown.get(ctx), true);
    elapse(99);
    assert.equal(shown.get(ctx), true);
    elapse(1);
    assert.equal(shown.get(ctx), false);
}

test "timers fire in due order, and ties in the order they were scheduled" {
    let ctx = context { Ui: headless(), Watch: observer() };
    let log = recorder();
    let _ = after(ctx, time.milliseconds(300), fn(_c) => log.record("c at 300"));
    let _ = after(ctx, time.milliseconds(100), fn(c) => {
        log.record("a at 100");
        let _ = after(c, time.milliseconds(50), fn(_c) => log.record("a's child at 150"));
    });
    let _ = after(ctx, time.milliseconds(100), fn(c) => {
        log.record("b at 100");
        let _ = after(c, time.milliseconds(500), fn(_c) => log.record("b's child at 600"));
    });
    let dropped = after(ctx, time.milliseconds(50), fn(_c) => log.record("cancelled"));
    let _ = after(ctx, time.milliseconds(0), fn(_c) => log.record("zero"));
    cancel(ctx, dropped);
    assert.equal(log.recorded(), []);
    elapse(200);
    assert.equal(log.recorded(), ["zero", "a at 100", "b at 100", "a's child at 150"]);
    elapse(200);
    assert.equal(
        log.recorded(),
        ["zero", "a at 100", "b at 100", "a's child at 150", "c at 300"],
    );
    elapse(199);
    assert.equal(log.recorded().length(), 5);
    elapse(1);
    assert.equal(
        log.recorded(),
        ["zero", "a at 100", "b at 100", "a's child at 150", "c at 300", "b's child at 600"],
    );
    // Cancelling one that already fired is a no-op.
    cancel(ctx, dropped);
}

test "a zero delay waits for elapse, and each timer settles the graph on its own turn" {
    let ctx = context { Ui: headless(), Watch: observer() };
    let count = signal(ctx, 0);
    let seen = recorder();
    watch(ctx, fn(s) => {
        let _ = seen.note(count.get(s));
    });
    let _ = after(ctx, time.milliseconds(0), fn(c) => {
        let _ = count.set(c, count.get(c) + 1);
        let _ = count.set(c, count.get(c) + 1);
    });
    let _ = after(ctx, time.milliseconds(0), fn(c) => {
        let _ = count.set(c, count.get(c) + 10);
        let _ = count.set(c, count.get(c) + 10);
    });
    assert.equal(seen.noted(), [0]);
    elapse(0);
    assert.equal(seen.noted(), [0, 2, 22]);
}

test "a timer reads a file through the context it was scheduled with" {
    let ctx = context {
        Allocator: alloc(),
        Ui: headless(),
        Watch: observer(),
        FileSystemRead: fs().files([("a.txt", "from disk")]),
    };
    let seen = signal(ctx, "nothing yet");
    let _ = after(ctx, time.milliseconds(10), fn(c) => {
        let read = match (readText(c, path.of(c, "a.txt"))) {
            .Ok(text) => text,
            .Err(_) => "unreadable",
        };
        let _ = seen.set(c, read);
    });
    elapse(10);
    assert.equal(seen.get(ctx), "from disk");
}

test "a timer still pending at the end of a test is given back" {
    let ctx = context { Allocator: alloc(), Ui: headless(), Watch: observer() };
    let held = ["a", "list", "the", "timer", "keeps"];
    let _ = after(ctx, time.milliseconds(1000000), fn(_c) => {
        let _ = held.length();
    });
}
"#,
    );

    let mut modes = crate::e2e::build_modes();
    modes.push(("js", &["--output=js"]));
    for (backend, flags) in modes {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
        cmd.current_dir(&repo).arg("test").args(flags).arg("//timer");
        let ran = crate::shared::ran_command(crate::shared::heap_checked(&mut cmd));
        assert!(
            ran.status == 0 && ran.stdout.contains("5 passed, 0 failed"),
            "{backend}: headless timers:\n{}\n{}",
            ran.stdout,
            ran.stderr
        );
    }
}
