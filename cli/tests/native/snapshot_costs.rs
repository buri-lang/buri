//! What recording a snapshot costs, natively, counted rather than timed.
//!
//! `buri test --update` paints each snapshot and writes it as a PNG. The test
//! binary here records one snapshot in its first block and [`MANY`] in its
//! second, so the difference between the two runs is the painting and writing
//! of the extra ones, and everything a process does once (starting, loading
//! fonts, reading the stylesheet) cancels.

use crate::conformance::{linked, Built};
use crate::counted::counted_output;
use crate::shared::{heap_checked, ran_command};
use std::path::Path;

/// How many snapshots the second block records.
const MANY: usize = 9;

/// A card wide enough to have something on every row: a heading, a line of
/// text and a coloured panel with rounded corners, on an 800-pixel page.
const CARD: &str = r#"
from "platform/effect" import { Allocator, Ui };
from "platform/effect/testing" import { alloc, headless, snapshot };
from "ui/node" import * as ui;
from "ui/node" import { Node };

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

/// [`CARD`], and a block recording one snapshot, then a block recording
/// [`MANY`].
fn source() -> String {
    let take = |name: String| {
        format!(
            "    snapshot(context {{ Allocator: alloc(), Ui: headless() }}, \"{name}\", card(), .Hover, []);\n"
        )
    };
    let many: String = (0..MANY).map(|i| take(format!("many-{i}"))).collect();
    format!(
        "{CARD}\ntest \"one\" {{\n{}}}\n\ntest \"many\" {{\n{many}}}\n",
        take(String::from("one"))
    )
}

/// The test binary run on block `block` alone, recording into `dir` when
/// `update`, and its instructions: the fewest of three runs, as
/// `collection_costs.rs` counts. `None` where the kernel counts nothing.
fn recorded(binary: &Path, dir: &Path, block: usize, update: bool) -> Option<u64> {
    let command = || {
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
    };
    let mut fewest: Option<Option<u64>> = None;
    for _ in 0..3 {
        let (out, count) = counted_output(command().spawn().unwrap());
        assert_eq!(
            out.status.code(),
            Some(0),
            "block {block}, update {update}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        fewest = Some(match fewest {
            None => count,
            Some(seen) => seen.zip(count).map(|(a, b)| a.min(b)),
        });
    }
    // On arm64, Memcheck reads the paint runtime's optimized code as branching
    // on unset values, which `memcheck.yml` says why ("arm64 runs `paint::` at
    // opt-level 0"). There the heap check runs outside Valgrind, and the
    // encoder's own unit tests are the ones Memcheck checks.
    let (status, stderr) = if cfg!(target_arch = "aarch64") {
        let out = heap_checked(&mut command()).output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        (out.status.code().unwrap_or(-1), stderr)
    } else {
        let ran = ran_command(heap_checked(&mut command()));
        (ran.status, ran.stderr)
    };
    assert_eq!(status, 0, "block {block} under the heap check: {stderr}");
    fewest.flatten()
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
    if let Some(why) = crate::conformance::skip_reason() {
        crate::ci::missing("stencil", &why);
        return;
    }
    let binary = match linked("snapshot-costs", &source()) {
        Built::Linked(binary, blocks) => {
            assert_eq!(blocks, 2);
            binary
        }
        Built::FrontEnd(why) | Built::Unsupported(why) => panic!("{why}"),
    };
    let dir = binary.with_file_name("snapshot-costs-__snapshots__");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let one = recorded(&binary, &dir, 0, true);
    let many = recorded(&binary, &dir, 1, true);
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
    recorded(&binary, &dir, 1, false);
    assert!(
        !dir.join("many-0.diff.png").exists(),
        "a matching run left a diff"
    );

    let each = one
        .zip(many)
        .map(|(a, b)| b.saturating_sub(a) / (MANY as u64 - 1));
    eprintln!("one snapshot recorded: {each:?} instructions");
    if let Some(each) = each {
        assert!(
            each < BOUND,
            "one snapshot recorded took {each} instructions, over {BOUND}"
        );
    }
}

/// See [`a_snapshot_is_recorded_through_a_word_at_a_time_encoder`].
const BOUND: u64 = 56_000_000;
