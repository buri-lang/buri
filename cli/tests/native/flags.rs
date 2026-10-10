//! How many bytes a `derive Flags` value takes, natively, on every native
//! backend this toolchain has built in.
//!
//! A list of a thousand values is a thousand strides of payload, so the bytes
//! the runtime hands out for one, against the same list empty, are the stride
//! times a thousand. The bytes are read through the runtime's own heap
//! statistics, and every run is under the heap check.

use crate::shared::{probed, ALLOC_PROBE};

/// [`ALLOC_PROBE`], plus a line with every byte the run allocated.
fn bytes_probe() -> String {
    format!(
        "{ALLOC_PROBE}
__attribute__((destructor)) static void buri_bytes_probe(void) {{
  Stats s; buri_rt_heap_stats(&s);
  fprintf(stderr, \"bytes=%llu\\n\", (unsigned long long)s.total_bytes);
}}
"
    )
}

/// The `bytes=` line a [`bytes_probe`]-linked run printed.
fn bytes(stderr: &str) -> u64 {
    stderr
        .lines()
        .find_map(|l| l.strip_prefix("bytes="))
        .unwrap_or_else(|| panic!("the probe printed nothing: {stderr:?}"))
        .trim()
        .parse()
        .unwrap()
}

/// `struct W<n>` with `n` `Bool` fields, deriving `Flags`.
fn flags_struct(n: usize) -> String {
    let fields: Vec<String> = (0..n).map(|i| format!("f{i}: Bool")).collect();
    format!("derive Flags, Equal for W{n};\nstruct W{n} {{ {} }}\n", fields.join(", "))
}

/// **The word is the smallest that holds every field.** 8 flags are a byte, 9
/// and 16 two, 17 and 32 four, 33 and 64 eight, and 8 flags beside a `U8` are
/// two bytes. A struct of eight `Bool`s laid out plainly is eight bytes, so a
/// packing that stopped happening shows as a stride eight times too wide.
#[test]
fn a_flags_value_is_the_smallest_word_that_holds_it() {
    let widths = [8usize, 9, 16, 17, 32, 33, 64];
    let mut source = String::from(
        r#"
from "core/env" import * as env;
from "core/flags" import * as flags;
from "core/flags" import { Flags };
from "core/io" import * as io;
from "core/list" import * as list;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

derive Equal for Held;
struct Held { access: W8, tag: U8 }
"#,
    );
    for n in widths {
        source.push_str(&flags_struct(n));
    }
    // `main <type> <count>` builds `count` of the type's `all` and prints how
    // many flags the last one holds, or the width when there is none. Both
    // runs print the same and both counts are four digits, so the only bytes
    // that differ are the list's.
    let arms: Vec<String> = widths
        .iter()
        .map(|n| {
            format!(
                ".Some(\"W{n}\") => list.repeat(ctx, flags.all<W{n}>(), count).last().map(fn(v) => v.count()).withDefault({n}),"
            )
        })
        .collect();
    source.push_str(&format!(
        r#"
export fn main(host: NativeHost): Result<(), Str> {{
  let ctx = context {{ Allocator: host.alloc, Environment: host.env, Stdout: host.stdout }};
  let args = env.arguments(ctx);
  let count = args.get(1).andThen(fn(s) => s.toInt()).withDefault(0);
  let held = Held {{ access: flags.all<W8>(), tag: 1 }};
  let answer = match (args.first()) {{
    {}
    .Some("Held") => list.repeat(ctx, held, count).last().map(fn(h) => h.access.count()).withDefault(8),
    _ => -1,
  }};
  io.println(ctx, "${{answer}}").mapErr(fn(_e) => "stdout")
}}
"#,
        arms.join("\n    ")
    ));
    let rows = [("W8", 1, 8), ("W9", 2, 9), ("W16", 2, 16), ("W17", 4, 17), ("W32", 4, 32), ("W33", 8, 33), ("W64", 8, 64), ("Held", 2, 8)];
    let probe = bytes_probe();
    for (backend, build) in crate::e2e::probed_builds(&probe) {
        let binary = build("flags-stride", &source);
        for (ty, stride, held) in rows {
            let run = |count: &str| {
                let mut cmd = std::process::Command::new(&binary);
                cmd.args([ty, count]);
                crate::shared::ran_command(crate::shared::heap_checked(&mut cmd))
            };
            let (none, many) = (run("0000"), run("1000"));
            assert_eq!(many.stdout, format!("{held}\n"), "{backend} {ty}: {}", many.stderr);
            assert_eq!(none.stdout, many.stdout, "{backend} {ty}: {}", none.stderr);
            for r in [&none, &many] {
                assert_eq!(r.status, 0, "{backend} {ty}: {}", r.stderr);
                assert_eq!(probed(&r.stderr).1, 0, "{backend} {ty}: blocks still live at exit");
            }
            let grew = bytes(&many.stderr) - bytes(&none.stderr);
            assert_eq!(
                grew,
                1000 * stride,
                "{backend}: a thousand {ty} took {grew} bytes, where {stride} a value is {}",
                1000 * stride
            );
        }
    }
}

const REPORTS: &str = r#"from "core/flags" import * as flags;
from "core/flags" import { Flags };
from "core/testing/assert" import * as assert;

derive Flags, Equal, Show for Access;
struct Access {
    read: Bool,
    write: Bool,
}

// No `Show`, so the report walks the word itself.
derive Flags, Equal for Bare;
struct Bare(Bool, Bool);

test "a flags value" {
    assert.equal(Access { ..flags.none<Access>(), write: true }, flags.all<Access>());
}

test "no Show" {
    assert.equal(Bare(true, false), Bare(false, true));
}

test "a list" {
    assert.equal([flags.none<Access>()], []);
}
"#;

/// **A failing assertion prints a flags value as the plain struct**, on
/// JavaScript and every native backend, through `Show` where the type derives
/// it and through the report's own walk where it does not.
#[test]
fn a_failing_assert_prints_flags_as_the_plain_struct_on_every_backend() {
    let repo = crate::hand_written_impls::repository("flags-reports", REPORTS);
    let expected = [
        "    actual:   Access { read: false, write: true }\n    expected: Access { read: true, write: true }\n",
        "    actual:   Bare(true, false)\n    expected: Bare(false, true)\n",
        "    actual:   [Access { read: false, write: false }]\n    expected: []\n",
    ];
    for (backend, status, stdout, stderr) in crate::hand_written_impls::every_backend(&repo) {
        let context = format!("{backend}:\n{stdout}\n{stderr}");
        assert_eq!(status, 1, "{context}");
        assert!(stdout.contains("0 passed, 3 failed"), "{context}");
        for line in expected {
            assert!(stdout.contains(line), "{backend}: missing\n{line}\nin\n{stdout}\n{stderr}");
        }
    }
}
