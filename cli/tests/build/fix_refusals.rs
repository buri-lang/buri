//! `lint --fix` refusing a file's edits: two fixes claiming the same bytes,
//! and a remembered fix whose range no longer fits the file.
//!
//! Each refusal leaves the file exactly as it was.
use crate::harness::*;

const BUILD: &str = "binary {\n    outputs: [{ platform: \"node\" }]\n}\n";

/// `width` and `one` take a `ctx` neither reads. Deleting `width`'s context
/// argument deletes the `if` holding `one`'s call, and deleting `one`'s
/// argument edits inside it.
const NESTED: &str = r#"from "core/io" import * as io;
from "node" import { NodeHost };
from "platform/effect" import { Allocator, Stdout };

fn one<C: Allocator>(ctx: C, n: Int): Int {
    n
}

fn width<C: Allocator>(ctx: C, label: Str): Int {
    label.length()
}

export fn main(host: NodeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    io.println(ctx, "${width(if (one(ctx, 1) > 0) { ctx } else { ctx }, "ab")}")
        .mapErr(fn(_e) => "no stdout")
}
"#;

#[test]
fn two_fixes_editing_the_same_bytes_are_both_refused() {
    let scratch = Scratch::repo("fix-refusals-overlap");
    scratch.write("cmd/app/BUILD.buri", BUILD);
    scratch.write("cmd/app/main.buri", NESTED);
    scratch
        .run(&["lint", "//...", "--fix"])
        .exits(1)
        .says("warning: cmd/app/main.buri has overlapping fixes (")
        .says("so none were applied")
        .says("nothing to fix");
    assert_eq!(scratch.read("cmd/app/main.buri"), NESTED);
}

/// One finding with a fix: `width`'s unused `ctx`. The comment holds a
/// character two bytes wide, at bytes 6 and 7.
const ONE_FIX: &str = r#"// café
from "core/io" import * as io;
from "node" import { NodeHost };
from "platform/effect" import { Allocator, Stdout };

fn width<C: Allocator>(ctx: C, label: Str): Int {
    label.length()
}

export fn main(host: NodeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    io.println(ctx, "${width(ctx, "ab")}").mapErr(fn(_e) => "no stdout")
}
"#;

/// Lints a repository holding [`ONE_FIX`], then rewrites the range of the
/// first fix the lint cache remembers with `damage`, and runs `--fix`.
///
/// A remembered finding is the code's text, then the count of its edits, then
/// each edit's file name, start and end, all lengths and offsets
/// little-endian `u32`s.
fn fix_with_damaged_range(name: &str, damage: impl Fn(u32, u32) -> (u32, u32)) -> (Scratch, Run) {
    let scratch = Scratch::repo(name);
    scratch.write("cmd/app/BUILD.buri", BUILD);
    scratch.write("cmd/app/main.buri", ONE_FIX);
    scratch
        .run(&["lint", "//..."])
        .exits(1)
        .says("[unused-context]");
    let code = [&14u32.to_le_bytes()[..], b"unused-context"].concat();
    let file = b"cmd/app/main.buri";
    let mut damaged = 0;
    for dir in std::fs::read_dir(scratch.path(".buri/cache"))
        .unwrap()
        .flatten()
    {
        let Ok(entries) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(mut bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            let Some(at) = bytes.windows(code.len()).position(|w| w == code) else {
                continue;
            };
            let edit = at + code.len() + 4;
            assert_eq!(
                &bytes[edit + 4..edit + 4 + file.len()],
                file,
                "the record changed shape"
            );
            let range = edit + 4 + file.len();
            let word = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
            let (start, end) = damage(word(range), word(range + 4));
            bytes[range..range + 4].copy_from_slice(&start.to_le_bytes());
            bytes[range + 4..range + 8].copy_from_slice(&end.to_le_bytes());
            std::fs::write(entry.path(), bytes).unwrap();
            damaged += 1;
        }
    }
    assert_eq!(damaged, 1, "no remembered fix was found to damage");
    let run = scratch.run(&["lint", "//...", "--fix"]);
    assert_eq!(scratch.read("cmd/app/main.buri"), ONE_FIX);
    (scratch, run)
}

#[test]
fn a_remembered_fix_running_backwards_is_refused() {
    let (_scratch, run) = fix_with_damaged_range("fix-refusals-inverted", |_, end| (end + 1, end));
    run.exits(1)
        .says("has an edit whose range runs backwards (")
        .says("so none were applied");
}

#[test]
fn a_remembered_fix_past_the_end_is_refused() {
    let (_scratch, run) =
        fix_with_damaged_range("fix-refusals-past-end", |start, _| (start, 100_000));
    run.exits(1).says(&format!(
        "has an edit at offset 100000, past the end of the {}-byte file",
        ONE_FIX.len()
    ));
}

#[test]
fn a_remembered_fix_inside_a_character_is_refused() {
    let (_scratch, run) = fix_with_damaged_range("fix-refusals-inside", |_, end| (7, end));
    run.exits(1)
        .says("has an edit at offset 7, which is inside a character, so none were applied");
}
