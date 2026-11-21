//! What recording and comparing a snapshot cost, natively, counted rather
//! than timed.
//!
//! `buri test --update` paints each snapshot and writes it as a PNG, and
//! `buri test` paints it and compares it against that PNG. The test binary
//! here takes one snapshot in its first block and [`MANY`] in its second, so
//! the difference between the two runs is the painting and the writing or
//! comparing of the extra ones, and everything a process does once (starting,
//! loading fonts, reading the stylesheet) cancels.
//!
//! Its third block takes a snapshot whose golden its fourth block recorded
//! from another picture, so comparing it fails and draws a diff.

use crate::conformance::{linked, Built};
use crate::counted::counted_output;
use crate::shared::{heap_checked, ran_command};
use std::path::{Path, PathBuf};

/// How many snapshots the second block takes.
const MANY: usize = 9;

/// A card wide enough to have something on every row: a heading, a line of
/// text and a coloured panel with rounded corners, on an 800-pixel page.
const CARD: &str = r#"
fn card<C>(): Node<C> {
    ui.stack({
        styles: [
            .Layout(.Column),
            .Group([
                .Padding(.Px(16)),
                .Background(.Rgb(240, 240, 245)),
                .Radius(.Px(8)),
                .Gap(.Px(8)),
            ]),
        ],
        children: [
            ui.text({
                content: .Const("A card with a heading"),
                headingLevel: .Some(2),
                styles: .Some([]),
            }),
            ui.text({ content: .Const("And a line of text under it, long enough to run across the panel.") }),
        ],
    })
}
"#;

const IMPORTS: &str = r#"
from "platform/effect" import { Allocator, Ui };
from "platform/effect/testing" import { alloc, headless, snapshot };
from "ui/node" import * as ui;
from "ui/node" import { Node };
"#;

/// The blocks, in order: one snapshot, [`MANY`], the snapshot `changed` of
/// [`CARD`], and the snapshot `changed` of a card whose heading says
/// something else.
fn source() -> String {
    let take = |name: &str, node: &str| {
        format!(
            "    snapshot(context {{ Allocator: alloc(), Ui: headless() }}, \"{name}\", {node}(), .Hover, []);\n"
        )
    };
    let many: String = (0..MANY)
        .map(|i| take(&format!("many-{i}"), "card"))
        .collect();
    let variant = CARD
        .replace("fn card", "fn variant")
        .replace("A card with a heading", "A card with a header!");
    format!(
        "{IMPORTS}{CARD}{variant}\ntest \"one\" {{\n{}}}\n\ntest \"many\" {{\n{many}}}\n\ntest \"changed\" {{\n{}}}\n\ntest \"variant\" {{\n{}}}\n",
        take("one", "card"),
        take("changed", "card"),
        take("changed", "variant"),
    )
}

/// The first block, the one with [`MANY`] snapshots, the one whose
/// comparison fails, and the one that records its golden.
const ONE: usize = 0;
const SEVERAL: usize = 1;
const CHANGED: usize = 2;
const VARIANT: usize = 3;

/// The test binary, linked once for every test here, and an empty snapshot
/// directory of the test's own beside it.
fn built(test: &str) -> Option<(PathBuf, PathBuf)> {
    if let Some(why) = crate::conformance::skip_reason() {
        crate::ci::missing("stencil", &why);
        return None;
    }
    let binary = match linked("snapshot-costs", &source()) {
        Built::Linked(binary, blocks) => {
            assert_eq!(blocks, 4);
            binary
        }
        Built::FrontEnd(why) | Built::Unsupported(why) => panic!("{why}"),
    };
    let dir = binary.with_file_name(format!("snapshot-costs-{test}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Some((binary, dir))
}

/// The test binary run on block `block` alone, recording into `dir` when
/// `update`.
fn command(binary: &Path, dir: &Path, block: usize, update: bool) -> std::process::Command {
    let mut cmd = std::process::Command::new(binary);
    cmd.env("BURI_TEST_FROM", block.to_string())
        .env("BURI_TEST_TO", (block + 1).to_string())
        .env("BURI_SNAPSHOT_DIR", dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if update {
        cmd.env("BURI_SNAPSHOT_UPDATE", "1");
    }
    cmd
}

/// Block `block` run three times, each ending with exit code `code`, and its
/// instructions: the fewest of the three, as `collection_costs.rs` counts.
/// `None` where the kernel counts nothing. The last run's standard error
/// comes back too.
fn counted(
    binary: &Path,
    dir: &Path,
    block: usize,
    update: bool,
    code: i32,
) -> (Option<u64>, String) {
    let mut fewest: Option<Option<u64>> = None;
    let mut stderr = String::new();
    for _ in 0..3 {
        let (out, count) = counted_output(command(binary, dir, block, update).spawn().unwrap());
        stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(
            out.status.code(),
            Some(code),
            "block {block}, update {update}: {stderr}"
        );
        fewest = Some(match fewest {
            None => count,
            Some(seen) => seen.zip(count).map(|(a, b)| a.min(b)),
        });
    }
    (fewest.flatten(), stderr)
}

/// [`counted`] for a block that passes, then the same block under the heap
/// check.
fn recorded(binary: &Path, dir: &Path, block: usize, update: bool) -> Option<u64> {
    let (count, _) = counted(binary, dir, block, update, 0);
    // On arm64, Memcheck reads the paint runtime's optimized code as branching
    // on unset values, which `memcheck.yml` says why ("arm64 runs `paint::` at
    // opt-level 0"). There the heap check runs outside Valgrind, and the
    // encoder's own unit tests are the ones Memcheck checks.
    let (status, stderr) = if cfg!(target_arch = "aarch64") {
        let out = heap_checked(&mut command(binary, dir, block, update))
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        (out.status.code().unwrap_or(-1), stderr)
    } else {
        let ran = ran_command(heap_checked(&mut command(binary, dir, block, update)));
        (ran.status, ran.stderr)
    };
    assert_eq!(status, 0, "block {block} under the heap check: {stderr}");
    count
}

/// What each of the second block's extra snapshots cost over the first
/// block's one.
fn each_extra(one: Option<u64>, many: Option<u64>) -> Option<u64> {
    one.zip(many)
        .map(|(a, b)| b.saturating_sub(a) / (MANY as u64 - 1))
}

/// **A snapshot is written through a PNG encoder that works a word at a
/// time.** The encoder compared candidate matches a byte at a time, wrote
/// each Huffman code a bit at a time, and took two remainders a byte for the
/// Adler-32. Each recording writes the same picture, and comparing against
/// what was written passes.
///
/// The bound is on the instructions one more recorded snapshot costs: the
/// painting, which is unchanged, and the encoding of a 1600 by 158 picture.
/// On macOS stencil, 96.2 M to 16.1 M. The bound sits about halfway, so it
/// fails if the encoder goes back, and leaves the painting room to grow.
#[test]
fn a_snapshot_is_recorded_through_a_word_at_a_time_encoder() {
    let Some((binary, dir)) = built("record") else {
        return;
    };
    let one = recorded(&binary, &dir, ONE, true);
    let many = recorded(&binary, &dir, SEVERAL, true);
    let first = std::fs::read(dir.join("one.png")).unwrap();
    assert!(
        first.starts_with(b"\x89PNG\r\n\x1a\n"),
        "one.png is not a PNG"
    );
    for i in 0..MANY {
        let again = std::fs::read(dir.join(format!("many-{i}.png"))).unwrap();
        assert!(again == first, "many-{i}.png is not the picture one.png is");
    }
    // Comparing against what was just written passes, and leaves no diff.
    recorded(&binary, &dir, SEVERAL, false);
    assert!(
        !dir.join("many-0.diff.png").exists(),
        "a matching run left a diff"
    );

    let each = each_extra(one, many);
    eprintln!("one snapshot recorded: {each:?} instructions");
    if let Some(each) = each {
        assert!(
            each < RECORD_BOUND,
            "one snapshot recorded took {each} instructions, over {RECORD_BOUND}"
        );
    }
}

/// See [`a_snapshot_is_recorded_through_a_word_at_a_time_encoder`].
const RECORD_BOUND: u64 = 56_000_000;

/// **A snapshot that did not change is compared a block of bytes at a
/// time.** The comparison inflates the golden and checks the fresh raster's
/// rows against it. It made a straight copy of every row, looking at each
/// pixel's alpha, and compared a byte at a time; and the inflater read its
/// input a byte at a time. Now an opaque row is compared where it stands,
/// with vector instructions, and the inflater reads eight bytes at once.
///
/// The bound is on the instructions one more compared snapshot costs: the
/// painting, which is unchanged, and the comparison of a 1600 by 158 picture.
/// On macOS stencil, 8.81 M to 7.09 M, of which the painting is about 4.5 M.
/// The bound sits about halfway, so it fails if the row comparison goes back
/// to copying, and leaves the painting about 0.9 M to grow.
#[test]
fn a_snapshot_that_did_not_change_is_compared_a_block_at_a_time() {
    let Some((binary, dir)) = built("compare") else {
        return;
    };
    recorded(&binary, &dir, ONE, true);
    recorded(&binary, &dir, SEVERAL, true);
    let one = recorded(&binary, &dir, ONE, false);
    let many = recorded(&binary, &dir, SEVERAL, false);
    let left: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".diff.png"))
        .collect();
    assert!(left.is_empty(), "a matching run left diffs: {left:?}");

    let each = each_extra(one, many);
    eprintln!("one snapshot compared: {each:?} instructions");
    if let Some(each) = each {
        assert!(
            each < COMPARE_BOUND,
            "one snapshot compared took {each} instructions, over {COMPARE_BOUND}"
        );
    }
}

/// See [`a_snapshot_that_did_not_change_is_compared_a_block_at_a_time`].
const COMPARE_BOUND: u64 = 8_000_000;

/// **A snapshot that changed draws its diff a row at a time.** The diff read
/// both pictures a pixel at a time, each read checking its coordinates, and
/// the encoder that writes it took the Adler-32 a byte at a time. Now each
/// row of the diff is decided from the two pictures' rows, and the checksum
/// sums a block of bytes with vector instructions. The test fails naming the
/// diff, which is a PNG.
///
/// The bound is on what a changed snapshot costs over one that matches: the
/// full decode of the golden, the diff, and encoding it. On macOS stencil,
/// 25.6 M to 11.8 M. The bound sits about halfway, so it fails if the diff
/// goes back to reading a pixel at a time (about 14 M of the difference).
#[test]
fn a_changed_snapshot_draws_its_diff_a_row_at_a_time() {
    let Some((binary, dir)) = built("changed") else {
        return;
    };
    recorded(&binary, &dir, ONE, true);
    recorded(&binary, &dir, VARIANT, true);
    let matching = recorded(&binary, &dir, ONE, false);
    let (changed, stderr) = counted(&binary, &dir, CHANGED, false, 1);
    assert!(
        stderr
            .contains("the snapshot \"changed\" changed: see test/__snapshots__/changed.diff.png"),
        "{stderr}"
    );
    let diff = std::fs::read(dir.join("changed.diff.png")).unwrap();
    assert!(
        diff.starts_with(b"\x89PNG\r\n\x1a\n"),
        "changed.diff.png is not a PNG"
    );

    let more = matching.zip(changed).map(|(a, b)| b.saturating_sub(a));
    eprintln!("a changed snapshot over a matching one: {more:?} instructions");
    if let Some(more) = more {
        assert!(
            more < CHANGED_BOUND,
            "a changed snapshot took {more} instructions over a matching one, over {CHANGED_BOUND}"
        );
    }
}

/// See [`a_changed_snapshot_draws_its_diff_a_row_at_a_time`].
const CHANGED_BOUND: u64 = 18_000_000;
