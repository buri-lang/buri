//! Steps that wait run side by side, on whichever native backend this
//! toolchain has (buri-lang/buri#280).
//!
//! Each row's steps meet at a barrier: a step only finishes once every other
//! step has arrived. Steps that run one at a time never meet, so the first one
//! gives up after its bound and the row fails on what the program printed. No
//! row reads a clock.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::shared::{heap_checked, probed, Ran, ALLOC_PROBE};

/// The barrier, as a shell script: `barrier.sh DIR NAME COUNT` marks `NAME` as
/// arrived and waits until `COUNT` have. It gives up after about 20 s, and
/// once one waiter has given up the rest stop at once.
const BARRIER: &str = r#"d="$1"
touch "$d/arrived/$2"
n=0
while [ "$(ls "$d/arrived" | wc -l | tr -d ' ')" -lt "$3" ]; do
  [ -e "$d/failed" ] && exit 2
  n=$((n + 1))
  if [ "$n" -gt 2000 ]; then
    touch "$d/failed"
    exit 1
  fi
  sleep 0.01
done
"#;

/// A fresh scratch directory for `row`, with the barrier script and an empty
/// `arrived/` in it.
fn scratch(row: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("buri-side-{row}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("arrived")).unwrap();
    std::fs::write(dir.join("barrier.sh"), BARRIER).unwrap();
    dir
}

/// `source` with `DIR` replaced by `dir`.
fn at(source: &str, dir: &Path) -> String {
    source.replace("DIR", &dir.display().to_string())
}

/// Builds and runs `source` under the heap check. A run that hasn't finished
/// after two minutes is killed and fails the row: its steps are stuck waiting
/// for each other.
fn ran_within(row: &str, source: &str) -> Option<Ran> {
    let binary = crate::e2e::built_probed(row, source, ALLOC_PROBE)?;
    let mut child = heap_checked(&mut Command::new(&binary))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(120);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{row}: still running after 120 s, so its steps never met");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let out = child.wait_with_output().unwrap();
    let ran = Ran {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    };
    assert_eq!(ran.status, 0, "{row}: stderr: {}", ran.stderr);
    let (_, live) = probed(&ran.stderr);
    assert_eq!(live, 0, "{row}: blocks still live at exit");
    Some(ran)
}

/// Four steps that each run a child process, which waits for all four.
#[test]
fn steps_that_run_a_process_run_side_by_side() {
    let dir = scratch("run");
    let source = at(
        r#"
from "core/io" import * as io;
from "core/str" import * as str;
from "core/list" import * as list;
from "core/process" import * as process;
from "core/process" import { Spawn };
from "core/tasks" import * as tasks;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout, Tasks };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Spawn: host.spawn, Stdout: host.stdout, Tasks: host.tasks };
    let codes = tasks.parallel(ctx, list.range(ctx, 0, 4), fn(c, i, _x) => {
        match (process.run(c, process.command("sh", ["DIR/barrier.sh", "DIR", str.format(c, "${i}"), "4"]))) {
            .Ok(out) => out.code,
            .Err(_e) => -1,
        }
    });
    let _ = io.println(ctx, "${codes.mapCtx(ctx, fn(c, code) => str.format(c, "${code}")).join(ctx, " ")}").ignore();
    .Ok(())
}
"#,
        &dir,
    );
    let Some(r) = ran_within("side-run", &source) else { return };
    assert_eq!(r.stdout, "0 0 0 0\n");
}

/// The same, through `process.start` and `child.wait`.
#[test]
fn steps_that_wait_on_a_started_child_run_side_by_side() {
    let dir = scratch("wait");
    let source = at(
        r#"
from "core/io" import * as io;
from "core/str" import * as str;
from "core/list" import * as list;
from "core/process" import * as process;
from "core/process" import { Spawn };
from "core/tasks" import * as tasks;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout, Tasks };

fn waited<C: Allocator + Spawn>(ctx: C, i: Int): Int {
    match (process.start(ctx, process.command("sh", ["DIR/barrier.sh", "DIR", str.format(ctx, "${i}"), "4"]))) {
        .Err(_e) => -1,
        .Ok(child) => match (child.wait(ctx)) {
            .Ok(.Exited(code)) => code,
            .Ok(_signaled) => -2,
            .Err(_e) => -3,
        },
    }
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Spawn: host.spawn, Stdout: host.stdout, Tasks: host.tasks };
    let codes = tasks.parallel(ctx, list.range(ctx, 0, 4), fn(c, i, _x) => waited(c, i));
    let _ = io.println(ctx, "${codes.mapCtx(ctx, fn(c, code) => str.format(c, "${code}")).join(ctx, " ")}").ignore();
    .Ok(())
}
"#,
        &dir,
    );
    let Some(r) = ran_within("side-wait", &source) else { return };
    assert_eq!(r.stdout, "0 0 0 0\n");
}

/// Four steps that each mark a file and then `time.sleep` until all four
/// files are there.
#[test]
fn steps_that_sleep_run_side_by_side() {
    let dir = scratch("sleep");
    let source = at(
        r#"
from "core/fs" import { FileSystemRead, FileSystemWrite };
from "core/fs" import * as fs;
from "core/io" import * as io;
from "core/str" import * as str;
from "core/list" import * as list;
from "core/path" import { Path };
from "core/path" import * as filepath;
from "core/tasks" import * as tasks;
from "core/time" import * as time;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Clock, Stdout, Tasks };

fn met<C: Allocator + Clock + FileSystemRead + FileSystemWrite>(ctx: C, dir: Path, left: Int): Bool {
    let arrived = match (fs.listDir(ctx, dir.join(ctx, "arrived"))) {
        .Ok(names) => names.length(),
        .Err(_e) => 0,
    };
    if (arrived >= 4) {
        true
    } else if (fs.exists(ctx, dir.join(ctx, "failed"))) {
        false
    } else if (left == 0) {
        let _ = fs.writeText(ctx, dir.join(ctx, "failed"), "").ignore();
        false
    } else {
        let _ = time.sleep(ctx, time.milliseconds(10));
        met(ctx, dir, left - 1)
    }
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Clock: host.clock,
        FileSystemRead: host.fs,
        FileSystemWrite: host.fs,
        Stdout: host.stdout,
        Tasks: host.tasks,
    };
    let met = tasks.parallel(ctx, list.range(ctx, 0, 4), fn(c, i, _x) => {
        let dir = filepath.of(c, "DIR");
        let _ = fs.writeText(c, dir.join(c, "arrived").join(c, str.format(c, "${i}")), "").ignore();
        met(c, dir, 2000)
    });
    let _ = io.println(ctx, "${met.mapCtx(ctx, fn(c, m) => str.format(c, "${m}")).join(ctx, " ")}").ignore();
    .Ok(())
}
"#,
        &dir,
    );
    let Some(r) = ran_within("side-sleep", &source) else { return };
    assert_eq!(r.stdout, "true true true true\n");
}

/// One step reads a FIFO and the other writes it. Opening either end blocks
/// until the other end is open, so one step alone waits forever.
#[test]
fn a_step_reading_a_fifo_meets_the_step_writing_it() {
    let dir = scratch("fifo");
    let fifo = dir.join("pipe");
    let made = Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(made.success(), "mkfifo failed");
    let source = at(
        r#"
from "core/fs" import { FileSystemRead, FileSystemWrite };
from "core/fs" import * as fs;
from "core/io" import * as io;
from "core/str" import * as str;
from "core/list" import * as list;
from "core/path" import * as filepath;
from "core/tasks" import * as tasks;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout, Tasks };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        FileSystemRead: host.fs,
        FileSystemWrite: host.fs,
        Stdout: host.stdout,
        Tasks: host.tasks,
    };
    let said = tasks.parallel(ctx, list.range(ctx, 0, 2), fn(c, i, _x) => {
        let pipe = filepath.of(c, "DIR/pipe");
        if (i == 0) {
            match (fs.readText(c, pipe)) {
                .Ok(body) => str.format(c, "read ${body}"),
                .Err(_e) => "read failed",
            }
        } else {
            match (fs.writeText(c, pipe, "hello")) {
                .Ok(_u) => "wrote",
                .Err(_e) => "write failed",
            }
        }
    });
    let _ = io.println(ctx, said.join(ctx, ", ")).ignore();
    .Ok(())
}
"#,
        &dir,
    );
    let Some(r) = ran_within("side-fifo", &source) else { return };
    assert_eq!(r.stdout, "read hello, wrote\n");
}

/// A scope's body and the task it spawned each run a child, and each child
/// waits for the other.
#[test]
fn a_scope_runs_its_task_beside_its_body() {
    let dir = scratch("scope");
    let source = at(
        r#"
from "core/io" import * as io;
from "core/str" import * as str;
from "core/process" import * as process;
from "core/process" import { Spawn };
from "core/tasks" import * as tasks;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout, Tasks };

fn barrier<C: Allocator + Spawn>(ctx: C, name: Str): Int {
    match (process.run(ctx, process.command("sh", ["DIR/barrier.sh", "DIR", name, "2"]))) {
        .Ok(out) => out.code,
        .Err(_e) => -1,
    }
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Spawn: host.spawn, Stdout: host.stdout, Tasks: host.tasks };
    let body = tasks.scope(ctx, fn(c, s) => {
        let _ = tasks.spawn(c, s, fn(t) => {
            let _ = barrier(t, "task");
            ()
        });
        barrier(c, "body")
    });
    let _ = io.println(ctx, "body ${body}").ignore();
    .Ok(())
}
"#,
        &dir,
    );
    let Some(r) = ran_within("side-scope", &source) else { return };
    assert_eq!(r.stdout, "body 0\n");
}

/// Steps on several threads at once share a list and its strings, made before
/// the fan-out, and take and drop references to them over and over. Every
/// count has to come back to zero: the heap check fails the run otherwise.
#[test]
fn steps_share_values_made_before_the_fan_out() {
    let source = r#"
from "core/io" import * as io;
from "core/str" import * as str;
from "core/list" import * as list;
from "core/tasks" import * as tasks;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Tasks };

fn churn<C: Allocator>(ctx: C, words: [Str], k: Int, acc: Int): Int {
    if (k == 0) {
        acc
    } else {
        let kept = words.map(ctx, fn(w) => w);
        let longest = kept.fold(fn(a, w) => if (w.length() > a) { w.length() } else { a }, 0);
        churn(ctx, words, k - 1, acc + longest + kept.length())
    }
}

fn rounds<C: Allocator + Tasks>(ctx: C, words: [Str], k: Int, acc: Int): Int {
    if (k == 0) {
        acc
    } else {
        let ys = tasks.parallel(ctx, list.range(ctx, 0, 32), fn(c, i, _x) => churn(c, words, 200, i));
        rounds(ctx, words, k - 1, acc + ys.fold(fn(a, y) => a + y, 0))
    }
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Tasks: host.tasks };
    let words = list.range(ctx, 0, 16).mapCtx(ctx, fn(c, i) => str.format(c, "word number ${i} of the shared list"));
    let _ = io.println(host.stdout, "${rounds(ctx, words, 20, 0)}").ignore();
    .Ok(())
}
"#;
    let Some(r) = ran_within("side-shared", source) else { return };
    assert_eq!(r.stdout, "6281920\n");
}
