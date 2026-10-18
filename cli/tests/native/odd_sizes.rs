//! Values at the sizes a native backend takes a different path for: list
//! elements three, five, six and seven bytes wide, which no single load reads,
//! and a string literal longer than sixteen mebibytes, past what a Mach-O
//! instruction relocation's addend can reach.
//!
//! Each run is a real `buri test` over a repository, under the heap check, on
//! JavaScript and on every native backend this toolchain has built in.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace(name: &str) -> PathBuf {
    crate::sweep::once();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-odd-sizes-{}", std::process::id()))
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

/// `buri test //lib` on every backend, with `test` as the one test source.
/// Each run must pass all `blocks` of it.
fn passes(name: &str, test: &str, blocks: usize) {
    let repo = workspace(name);
    write(&repo.join("REPO.buri"), "");
    write(
        &repo.join("lib/BUILD.buri"),
        "library {\n    test {\n        sources: [\"test/t.buri\"]\n    }\n}\n",
    );
    write(
        &repo.join("lib/lib.buri"),
        "export fn unused(): Int {\n    0\n}\n",
    );
    write(&repo.join("lib/test/t.buri"), test);
    let mut modes = crate::e2e::build_modes();
    modes.push(("js", &["--output=js"]));
    for (backend, flags) in modes {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
        cmd.current_dir(&repo)
            .arg("test")
            .args(flags)
            .arg("//lib")
            .arg("--force");
        let ran = crate::shared::ran_command(crate::shared::heap_checked(&mut cmd));
        let summary = format!("{blocks} passed, 0 failed");
        assert!(
            ran.status == 0 && ran.stdout.contains(&summary),
            "{backend}: status {}\n{}\n{}",
            ran.status,
            ran.stdout,
            ran.stderr
        );
    }
}

/// Structs of three, five, six and seven `Bool`s, read out of a list by a
/// pattern and by `map`.
#[test]
fn elements_of_three_five_six_and_seven_bytes_read_back() {
    passes(
        "narrow-elements",
        r#"from "core/testing/assert" import * as assert;
from "platform/effect" import { Allocator };
from "platform/effect/testing" import { alloc };

derive Equal, Show for Three;
struct Three {
    a: Bool,
    b: Bool,
    c: Bool,
}

derive Equal, Show for Five;
struct Five {
    a: Bool,
    b: Bool,
    c: Bool,
    d: Bool,
    e: Bool,
}

derive Equal, Show for Six;
struct Six {
    a: Bool,
    b: Bool,
    c: Bool,
    d: Bool,
    e: Bool,
    f: Bool,
}

derive Equal, Show for Seven;
struct Seven {
    a: Bool,
    b: Bool,
    c: Bool,
    d: Bool,
    e: Bool,
    f: Bool,
    g: Bool,
}

fn second<T>(xs: [T], none: T): T {
    match (xs) {
        [_, x, ..] => x,
        _ => none,
    }
}

test "three bytes" {
    let ctx = context {
        Allocator: alloc(),
    };
    let off = Three { a: false, b: false, c: false };
    let on = Three { a: true, b: true, c: true };
    let xs = [on, Three { ..off, b: true }, on];
    assert.equal(second(xs, off), Three { ..off, b: true });
    assert.equal(xs.map(ctx, fn(t) => t.b && !t.c), [false, true, false]);
}

test "five bytes" {
    let off = Five { a: false, b: false, c: false, d: false, e: false };
    let on = Five { a: true, b: true, c: true, d: true, e: true };
    assert.equal(second([on, Five { ..off, c: true }, on], on), Five { ..off, c: true });
}

test "six bytes" {
    let off = Six { a: false, b: false, c: false, d: false, e: false, f: false };
    let on = Six { a: true, b: true, c: true, d: true, e: true, f: true };
    assert.equal(second([on, Six { ..off, f: true }, on], on), Six { ..off, f: true });
}

test "seven bytes" {
    let off = Seven { a: false, b: false, c: false, d: false, e: false, f: false, g: false };
    let on = Seven { a: true, b: true, c: true, d: true, e: true, f: true, g: true };
    assert.equal(second([on, Seven { ..off, a: true }, on], on), Seven { ..off, a: true });
}
"#,
        4,
    );
}

/// A literal of sixteen mebibytes and eight bytes, with a short one after it,
/// both read back.
#[test]
fn a_literal_past_sixteen_mebibytes_builds_and_reads_back() {
    let n = (16 << 20) + 8;
    let test = format!(
        r#"from "core/testing/assert" import * as assert;

test "a long literal and a short one after it" {{
    let long = "{}";
    let short = "after";
    assert.equal(long.length(), {n});
    assert.equal(short, "after");
    assert.equal(long.slice(0, 4), "abab");
}}
"#,
        "ab".repeat(n / 2)
    );
    passes("long-literal", &test, 1);
}
