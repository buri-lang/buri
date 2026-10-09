//! What `tasks.parallel` costs the scheduler, natively, on the release backend:
//! the threads a fan-out wakes, counted rather than timed, through the
//! runtime's `buri_rt_tasks_dispatch_wakes`.
//!
//! The copy-and-patch backend runs a fan-out's steps one after another on the
//! calling thread, so only the release backend has anything to count.

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
