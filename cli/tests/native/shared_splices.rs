//! `replaceAt` and `insertAt` on a list something else still holds, natively,
//! on every native backend built in: what each answer holds, that the list it
//! came from is unchanged, and what one such copy costs, counted rather than
//! timed.

use crate::collection_costs::steps_cost;

/// `main` takes a count `k`, builds one list of 16 elements, and splices it
/// `k` times, as `MODE` says, while the list itself stays alive, so every splice
/// copies. It prints a sum over the answers that reads the spliced element
/// and the list's length, then the first list, unchanged.
const SPLICED: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn ints<C: Allocator>(ctx: C, i: Int, acc: [Int]): [Int] {
  if (i == 16) { acc } else { ints(ctx, i + 1, acc.push(ctx, i * 3)) }
}

fn texts<C: Allocator>(ctx: C, i: Int, acc: [Str]): [Str] {
  if (i == 16) { acc } else { texts(ctx, i + 1, acc.push(ctx, str.format(ctx, "w${i}"))) }
}

fn replacedInts<C: Allocator>(ctx: C, xs: [Int], k: Int, acc: Int): Int {
  if (k == 0) { acc } else {
    let ys = xs.replaceAt(ctx, k % 16, k);
    replacedInts(ctx, xs, k - 1, acc + ys.length() + ys.get(k % 16).withDefault(0))
  }
}

fn insertedInts<C: Allocator>(ctx: C, xs: [Int], k: Int, acc: Int): Int {
  if (k == 0) { acc } else {
    let ys = xs.insertAt(ctx, k % 17, k);
    insertedInts(ctx, xs, k - 1, acc + ys.length() + ys.get(k % 17).withDefault(0))
  }
}

fn replacedTexts<C: Allocator>(ctx: C, xs: [Str], k: Int, acc: Int): Int {
  if (k == 0) { acc } else {
    let ys = xs.replaceAt(ctx, k % 16, "spliced");
    replacedTexts(ctx, xs, k - 1, acc + ys.length() + ys.get(k % 16).withDefault("").length())
  }
}

fn insertedTexts<C: Allocator>(ctx: C, xs: [Str], k: Int, acc: Int): Int {
  if (k == 0) { acc } else {
    let ys = xs.insertAt(ctx, k % 17, "spliced");
    insertedTexts(ctx, xs, k - 1, acc + ys.length() + ys.get(k % 17).withDefault("").length())
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let k = env.arguments(ctx).first().andThen(fn(s) => s.toInt()).withDefault(0);
  let nums = ints(ctx, 0, []);
  let ts = texts(ctx, 0, []);
  let line = match ("MODE") {
    "replace-ints" => str.format(ctx, "${replacedInts(ctx, nums, k, 0)} ${nums.mapCtx(ctx, fn(c, i) => str.format(c, "${i}")).join(ctx, ",")}"),
    "insert-ints" => str.format(ctx, "${insertedInts(ctx, nums, k, 0)} ${nums.mapCtx(ctx, fn(c, i) => str.format(c, "${i}")).join(ctx, ",")}"),
    "replace-texts" => str.format(ctx, "${replacedTexts(ctx, ts, k, 0)} ${ts.join(ctx, ",")}"),
    "insert-texts" => str.format(ctx, "${insertedTexts(ctx, ts, k, 0)} ${ts.join(ctx, ",")}"),
    _ => "no mode",
  };
  io.println(ctx, line).mapErr(fn(_e) => "stdout")
}
"#;

/// The modes [`SPLICED`] runs in.
const MODES: [&str; 4] = [
    "replace-ints",
    "insert-ints",
    "replace-texts",
    "insert-texts",
];

/// What [`SPLICED`] prints for `k` steps of `mode`, worked out by hand.
fn spliced(mode: &str, k: u64) -> String {
    let ints: Vec<String> = (0..16).map(|i| (i * 3).to_string()).collect();
    let texts: Vec<String> = (0..16).map(|i| format!("w{i}")).collect();
    // Each step adds the answer's length and what sits at the spliced index:
    // the step's own count for `Int`s, and the text's 7 bytes for texts.
    let (sum, first) = match mode {
        "replace-ints" => ((1..=k).map(|s| 16 + s).sum::<u64>(), ints.join(",")),
        "insert-ints" => ((1..=k).map(|s| 17 + s).sum::<u64>(), ints.join(",")),
        "replace-texts" => (k * (16 + 7), texts.join(",")),
        // "insert-texts", the last of [`MODES`].
        _ => (k * (17 + 7), texts.join(",")),
    };
    format!("{sum} {first}\n")
}

/// **Each splice of a list something else holds answers the spliced list and
/// leaves the first one as it was.** Every mode, over a count that covers
/// each index of the 16 elements and the 17 places an insert can go, under
/// the heap check, so a text retained once too few or too many times fails.
#[test]
fn a_splice_of_a_held_list_copies_it() {
    for (backend, build) in crate::e2e::probed_backends() {
        for mode in MODES {
            let binary = build(
                &format!("shared-splices-held-{mode}"),
                &SPLICED.replace("MODE", mode),
            );
            let ran = crate::shared::ran_command(crate::shared::heap_checked(
                std::process::Command::new(&binary).arg("40"),
            ));
            assert_eq!(ran.status, 0, "{backend} {mode}: {}", ran.stderr);
            assert_eq!(ran.stdout, spliced(mode, 40), "{backend} {mode}");
        }
    }
}

/// The two step counts [`SPLICED`] runs at.
const SPLICE_COUNTS: (u64, u64) = (100_000, 900_000);

/// **A splice of a held list is one copy, with nothing else around it.**
///
/// A replace used to copy the list element by element, retaining each, then
/// give the old list's count back through `decref`, whose drop glue walked
/// the elements a second time when that count was the last. Now the list is
/// one copy, every element but the spliced one takes a count after it, and
/// the count given back is the runtime's own, so a dying list is walked
/// once. An insert into a shared list gives its count back the same way.
///
/// The bounds are on the instructions one step costs, the copy, its frees
/// and the loop's step included. On macOS, before and after:
///
/// | backend, mode | before | after | bound |
/// | --- | --- | --- | --- |
/// | stencil, replace `Int`s | 352 | 315 | 333 |
/// | stencil, insert `Int`s | 382 | 360 | 371 |
/// | stencil, replace texts | 1,447 | 1,096 | 1,208 |
/// | stencil, insert texts | 1,531 | 1,253 | 1,373 |
/// | LLVM, replace `Int`s | 287 | 250 | 268 |
/// | LLVM, insert `Int`s | 318 | 296 | 307 |
/// | LLVM, replace texts | 935 | 809 | 872 |
/// | LLVM, insert texts | 997 | 960 | 978 |
///
/// Each bound sits halfway between the two, so the old code fails it by 1.2%
/// or more, about 40 times the 0.03% two runs differ by, and the new code
/// has the same room to grow. The splices are the runtime's, so every
/// backend is bounded.
///
/// Copy-and-patch's text rows have a second step. Each text the copy keeps
/// takes its count through the retain glue, which had a frame and a copy of
/// the text around one increment: 1,321 and 1,493 before the glue lost them.
/// Those bounds sit halfway from there.
#[test]
fn a_splice_of_a_held_list_costs_one_copy() {
    let (fewer, more) = SPLICE_COUNTS;
    let steps = more - fewer;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let llvm = backend == "llvm";
        for (mode, bound) in [
            ("replace-ints", if llvm { 268.0 } else { 333.0 }),
            ("insert-ints", if llvm { 307.0 } else { 371.0 }),
            ("replace-texts", if llvm { 872.0 } else { 1_208.0 }),
            ("insert-texts", if llvm { 978.0 } else { 1_373.0 }),
        ] {
            let binary = build(
                &format!("shared-splices-cost-{mode}"),
                &SPLICED.replace("MODE", mode),
            );
            let n = steps_cost(backend, &binary, SPLICE_COUNTS, |k| spliced(mode, k));
            let per = n.map(|n| n as f64 / steps as f64);
            eprintln!("{backend}, {mode}: {per:?} instructions a step");
            if per.is_some_and(|per| per > bound) {
                failures.push(format!(
                    "{backend} {mode}: {per:?} instructions a step, over {bound}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
