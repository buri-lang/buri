//! What `tasks.parallel` costs the scheduler, natively, on the release backend:
//! the threads a fan-out wakes, counted rather than timed, through the
//! runtime's `buri_rt_tasks_dispatch_wakes`.

use crate::shared::{probed, ran_checked, ALLOC_PROBE};

/// [`ALLOC_PROBE`], plus a line with the threads fan-outs woke over the run.
fn wake_probe() -> String {
    format!(
        "{ALLOC_PROBE}
extern uint64_t buri_rt_tasks_dispatch_wakes(void);
__attribute__((destructor)) static void buri_wake_probe(void) {{
  fprintf(stderr, \"wakes=%llu\\n\", (unsigned long long)buri_rt_tasks_dispatch_wakes());
}}
"
    )
}

/// The `wakes=` line a [`wake_probe`]-linked run printed.
fn wakes(stderr: &str) -> u64 {
    stderr
        .lines()
        .find_map(|l| l.strip_prefix("wakes="))
        .unwrap_or_else(|| panic!("the probe printed nothing: {stderr:?}"))
        .trim()
        .parse()
        .unwrap()
}

/// 500 fan-outs of 64 steps that each do one multiply. A broadcast per
/// fan-out woke every sleeping thread, and most found the queue already empty.
/// Now a fan-out wakes one, and that one wakes the next only if work is left.
#[test]
fn a_fan_out_of_trivial_steps_wakes_at_most_one_thread() {
    let source = r#"
from "core/io" import * as io;
from "core/list" import * as list;
from "core/tasks" import * as tasks;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Tasks };

fn rounds<C: Allocator + Tasks>(ctx: C, k: Int, acc: Int): Int {
  if (k == 0) {
    acc
  } else {
    let ys = tasks.parallel(ctx, list.range(ctx, 0, 64), fn(c, i, x) => x * 2 + i);
    rounds(ctx, k - 1, (acc + ys.fold(fn(a, y) => a + y, 0)) % 1000003)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Tasks: host.tasks };
  let _ = io.println(host.stdout, "${rounds(ctx, 500, 0)}").ignore();
  .Ok(())
}
"#;
    let Some(binary) = crate::e2e::built_probed("fan-out-wakes", source, &wake_probe()) else { return };
    let r = ran_checked(&binary);
    assert_eq!(r.status, 0, "stderr: {}", r.stderr);
    assert_eq!(r.stdout, "23991\n", "stderr: {}", r.stderr);
    let (_, live) = probed(&r.stderr);
    assert_eq!(live, 0, "blocks still live at exit");
    let woke = wakes(&r.stderr);
    assert!(woke <= 500, "500 fan-outs of trivial steps woke {woke} threads");
}

/// [`ALLOC_PROBE`], plus a line with the batches fan-outs queued and the most
/// task stacks the run had out at once.
fn window_probe() -> String {
    format!(
        "{ALLOC_PROBE}
extern uint64_t buri_rt_tasks_batches(void);
extern uint64_t buri_rt_task_stacks_peak(void);
__attribute__((destructor)) static void buri_window_probe(void) {{
  fprintf(stderr, \"batches=%llu peak=%llu\\n\",
          (unsigned long long)buri_rt_tasks_batches(), (unsigned long long)buri_rt_task_stacks_peak());
}}
"
    )
}

/// 20 fan-outs of 3,000 trivial steps, past the 1,024-step window. The window
/// refills 64 steps at a time, so a fan-out queues at most 1 + ⌈1,976 / 64⌉ =
/// 32 batches, where it queued one per step. A step maps its stack when it
/// first runs, and these never park, so no more stacks are out than threads
/// that run steps: the scheduler's 256 and the caller. Each queued step held
/// one: 1,024 at least.
#[test]
fn a_wide_fan_out_refills_in_batches_and_maps_stacks_for_running_steps() {
    let source = r#"
from "core/io" import * as io;
from "core/list" import * as list;
from "core/tasks" import * as tasks;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Tasks };

fn rounds<C: Allocator + Tasks>(ctx: C, k: Int, acc: Int): Int {
  if (k == 0) {
    acc
  } else {
    let ys = tasks.parallel(ctx, list.range(ctx, 0, 3000), fn(c, i, x) => x * 2 + i);
    rounds(ctx, k - 1, (acc + ys.fold(fn(a, y) => a + y, 0)) % 1000003)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Tasks: host.tasks };
  let _ = io.println(host.stdout, "${rounds(ctx, 20, 0)}").ignore();
  .Ok(())
}
"#;
    let Some(binary) = crate::e2e::built_probed("fan-out-window", source, &window_probe()) else { return };
    let r = ran_checked(&binary);
    assert_eq!(r.status, 0, "stderr: {}", r.stderr);
    assert_eq!(r.stdout, "909193\n", "stderr: {}", r.stderr);
    let (_, live) = probed(&r.stderr);
    assert_eq!(live, 0, "blocks still live at exit");
    let line = r.stderr.lines().find_map(|l| l.strip_prefix("batches=")).unwrap_or_else(|| panic!("the probe printed nothing: {}", r.stderr));
    let (batches, peak) = line.split_once(" peak=").unwrap();
    let (batches, peak): (u64, u64) = (batches.trim().parse().unwrap(), peak.trim().parse().unwrap());
    assert!(batches <= 20 * 32, "20 fan-outs of 3,000 steps queued {batches} batches");
    assert!(peak <= 257, "a fan-out of trivial steps had {peak} task stacks out at once");
}

/// [`ALLOC_PROBE`], plus a line with the bytes the runtime gave back to the
/// kernel over the run.
fn decommit_probe() -> String {
    format!(
        "{ALLOC_PROBE}
__attribute__((destructor)) static void buri_decommit_probe(void) {{
  Stats s; buri_rt_heap_stats(&s);
  fprintf(stderr, \"decommitted=%llu\\n\", (unsigned long long)s.decommitted_bytes);
}}
"
    )
}

/// The `decommitted=` line a [`decommit_probe`]-linked run printed.
fn decommitted(stderr: &str) -> u64 {
    stderr
        .lines()
        .find_map(|l| l.strip_prefix("decommitted="))
        .unwrap_or_else(|| panic!("the probe printed nothing: {stderr:?}"))
        .trim()
        .parse()
        .unwrap()
}

/// 320,000 trivial steps, each on a task stack the pool hands back. A shallow
/// step leaves its stack's watermark alone, so the stack goes back as it is.
/// Every 1,024th release used to re-map the stack whatever the watermark said:
/// 312 re-maps of 63.75 MiB here, each a TLB shootdown. Now that happens at
/// most once per 10 ms of the run, so the bound grows with a slow machine
/// rather than failing on one.
#[test]
fn shallow_steps_decommit_their_stacks_at_most_once_per_ten_milliseconds() {
    let source = r#"
from "core/io" import * as io;
from "core/list" import * as list;
from "core/tasks" import * as tasks;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Tasks };

fn rounds<C: Allocator + Tasks>(ctx: C, k: Int, acc: Int): Int {
  if (k == 0) {
    acc
  } else {
    let ys = tasks.parallel(ctx, list.range(ctx, 0, 64), fn(c, i, x) => x * 2 + i);
    rounds(ctx, k - 1, (acc + ys.fold(fn(a, y) => a + y, 0)) % 1000003)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Tasks: host.tasks };
  let _ = io.println(host.stdout, "${rounds(ctx, 5000, 0)}").ignore();
  .Ok(())
}
"#;
    let Some(binary) = crate::e2e::built_probed("fan-out-decommits", source, &decommit_probe()) else { return };
    let started = std::time::Instant::now();
    let r = ran_checked(&binary);
    let ran_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    assert_eq!(r.status, 0, "stderr: {}", r.stderr);
    assert_eq!(r.stdout, "239910\n", "stderr: {}", r.stderr);
    let (_, live) = probed(&r.stderr);
    assert_eq!(live, 0, "blocks still live at exit");
    // What one task stack's decommit gives back: all but the retained 256 KiB.
    let remaps = decommitted(&r.stderr) / (64 * 1024 * 1024 - 256 * 1024);
    assert!(remaps <= ran_ms / 10 + 1, "320,000 shallow steps in {ran_ms} ms re-mapped {remaps} stacks");
}
