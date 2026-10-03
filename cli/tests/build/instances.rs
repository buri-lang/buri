//! What a native artifact holds when many `context { ... }` values agree.
//!
//! Every `context { ... }` expression has a type of its own, so a generic
//! function over `C: Allocator` used to be compiled once per expression: a
//! suite of a hundred tests that each built the same context carried a hundred
//! copies of everything it reached. Contexts with equal bindings now share one
//! copy. These rows check that through what a user sees: the size of the
//! artifact, what the program prints, and what `--explain` says the cache did.
//!
//! ```text
//! cargo test -p buri --test build instances::
//! ```

use crate::harness::{indent, Run, Scratch};

/// The platform a binary here declares: this machine's own native variant.
fn host_platform() -> String {
    let os = if cfg!(target_os = "macos") { "macos" } else { "linux" };
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" };
    format!("\"native\", variant: \"{os}-{arch}\"")
}

/// The native artifact `//cmd/<name>` builds to.
fn artifact(scratch: &Scratch, name: &str) -> std::path::PathBuf {
    let os = if cfg!(target_os = "macos") { "macos" } else { "linux" };
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" };
    scratch.path(&format!(".buri/out/native/{os}-{arch}/cmd/{name}/{name}"))
}

/// Builds `source` as the native binary `//cmd/<name>` and answers what it
/// printed.
fn build_and_run(scratch: &Scratch, name: &str, source: &str) -> Run {
    scratch.write(
        &format!("cmd/{name}/BUILD.buri"),
        &format!("binary {{\n  outputs: [{{ platform: {} }}]\n}}\n", host_platform()),
    );
    scratch.write(&format!("cmd/{name}/main.buri"), source);
    scratch.run(&["build", &format!("//cmd/{name}")]).ok();
    let out = std::process::Command::new(artifact(scratch, name))
        .output()
        .unwrap_or_else(|e| panic!("cannot run //cmd/{name}: {e}"));
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        what: format!("//cmd/{name}"),
    }
}

/// A program that builds `count` equal contexts and hands each one to a
/// generic function reaching most of `core/orderedmap`.
fn many_contexts(count: usize) -> String {
    let mut body = String::new();
    for i in 0..count {
        body.push_str(&format!(
            "  let c{i} = context {{ Allocator: host.alloc, Stdout: host.stdout }};\n  \
             let m{next} = fill(c{i}, m{i}, {i});\n",
            next = i + 1
        ));
    }
    format!(
        r#"from "platform/effect" import {{ Allocator, Stdout }};
from "native" import {{ NativeHost }};
from "core/io" import * as io;
from "core/orderedmap" import * as ordmap;
from "core/orderedmap" import {{ OrderedMap }};

fn fill<C: Allocator>(ctx: C, m: OrderedMap<Int, Str>, k: Int): OrderedMap<Int, Str> {{
  m.insert(ctx, k, "v").remove(ctx, k + 1000)
}}

export fn main(host: NativeHost): Result<(), Str> {{
  let start = context {{ Allocator: host.alloc, Stdout: host.stdout }};
  let m0: OrderedMap<Int, Str> = ordmap.empty();
{body}  let _ = io.println(start, "size=${{m{count}.length()}}").ignore();
  .Ok(())
}}
"#
    )
}

/// Forty equal contexts cost about what two do.
///
/// Before contexts with equal bindings shared their instances, each one added
/// its own copy of `insert`, `remove` and everything under them, and the
/// forty-context binary was several times the size of the two-context one.
/// The bound is generous on purpose: what it rules out is growth with the
/// number of contexts, not a few bytes of `main`.
#[test]
fn equal_contexts_share_one_copy_of_the_code_they_reach() {
    let scratch = Scratch::repo("instances-many-contexts");
    let few = build_and_run(&scratch, "few", &many_contexts(2));
    assert_eq!(few.stdout, "size=2\n", "{}", indent(&few.all()));
    let many = build_and_run(&scratch, "many", &many_contexts(40));
    assert_eq!(many.stdout, "size=40\n", "{}", indent(&many.all()));

    let size = |name: &str| std::fs::metadata(artifact(&scratch, name)).unwrap().len();
    let (few, many) = (size("few"), size("many"));
    assert!(
        many <= few + few / 4,
        "forty equal contexts built a {many}-byte binary and two built a {few}-byte one; \
         contexts with equal bindings should share their instances"
    );
}

/// Contexts that bind different implementations, or the same ones in a
/// different order, still get code of their own.
///
/// `silent` binds a `Stdout` that writes nothing, so if its instance of `shout`
/// were shared with the real one, `b` would be printed. `swapped` binds the same
/// values as `first` in the other order, which lays the context out
/// differently.
#[test]
fn contexts_with_different_bindings_keep_their_own_code() {
    let program = r#"from "platform/effect" import { Allocator, IoError, Stdout };
from "native" import { NativeHost };
from "core/io" import * as io;

struct Silent(Int);
impl Stdout for Silent {
  fn print(self, text: Template): Result<(), IoError> { .Ok(()) }
  fn println(self, text: Template): Result<(), IoError> { .Ok(()) }
  fn writeBytes(self, b: [U8]): Result<(), IoError> { .Ok(()) }
}

// Recursive, so inlining leaves one function per instance.
fn shout<C: Stdout>(ctx: C, what: Str, n: Int): Int {
  if (n <= 0) {
    0
  } else {
    let _ = io.println(ctx, "${what}").ignore();
    shout(ctx, what, n - 1)
  }
}

export fn main(host: NativeHost): Result<(), Str> {
  let first = context { Allocator: host.alloc, Stdout: host.stdout };
  let silent = context { Allocator: host.alloc, Stdout: Silent(0) };
  let again = context { Allocator: host.alloc, Stdout: host.stdout };
  let swapped = context { Stdout: host.stdout, Allocator: host.alloc };
  let _ = shout(first, "a", 2);
  let _ = shout(silent, "b", 2);
  let _ = shout(again, "c", 1);
  let _ = shout(swapped, "d", 1);
  .Ok(())
}
"#;
    let scratch = Scratch::repo("instances-different-bindings");
    let run = build_and_run(&scratch, "shout", program);
    assert_eq!(run.stdout, "a\na\nc\nd\n", "{}", indent(&run.all()));
}

/// The test source of `//lib/walk`, with `extra` written before its one test.
fn walk_test(extra: &str) -> String {
    format!(
        r#"from "core/testing/assert" import * as assert;
from "platform/effect" import {{ Allocator, Clock, Stdout }};
from "platform/effect/testing" import {{ alloc, clock, stdout }};
from "//lib/walk" import {{ countdown }};
{extra}
test "counts down" {{
    let ctx = context {{ Allocator: alloc(), Stdout: stdout() }};
    assert.equal(countdown(ctx, 3, []), [3, 2, 1]);
}}
"#
    )
}

/// What `--explain` said about the `codegen` of one unit of `//lib/walk`.
fn codegen_status(run: &Run, unit: &str) -> String {
    let label = format!("//lib/walk:{unit}");
    for line in run.stdout.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() == 5 && f[1] == "codegen" && f[2] == label {
            return f[0].to_string();
        }
    }
    panic!("no `codegen {label}` line in:\n{}", indent(&run.all()));
}

/// Adding a test with a context of its own leaves the code for every other
/// context where the cache can find it.
///
/// The new test's context comes before the existing one in the source, so it
/// is checked first. When instances were named after a context's position in
/// that order, it renamed every instance over the later context, and the units
/// holding them — `countdown`'s and `core/list`'s — were compiled again.
#[test]
fn adding_a_context_keeps_the_other_instances_cached() {
    let scratch = Scratch::repo("instances-stable-names");
    scratch.write(
        "lib/walk/BUILD.buri",
        "library {\n    test {\n        sources: [\"test/walk.buri\"]\n    }\n}\n",
    );
    scratch.write(
        "lib/walk/lib.buri",
        r#"from "platform/effect" import { Allocator };

// Recursive, so inlining leaves it a function of its own.
export fn countdown<C: Allocator>(ctx: C, n: Int, acc: [Int]): [Int] {
    if (n <= 0) {
        acc
    } else {
        countdown(ctx, n - 1, acc.push(ctx, n))
    }
}
"#,
    );
    scratch.write("lib/walk/test/walk.buri", &walk_test(""));
    let first = scratch.run(&["test", "//lib/walk", "--explain"]);
    first.ok();
    assert_eq!(first.tests_passed(), 1, "{}", indent(&first.all()));

    scratch.write(
        "lib/walk/test/walk.buri",
        &walk_test(
            "\ntest \"an unrelated test\" {\n    \
             let _ = context { Allocator: alloc(), Clock: clock() };\n    \
             assert.equal(1 + 1, 2);\n}\n",
        ),
    );
    let second = scratch.run(&["test", "//lib/walk", "--explain"]);
    second.ok();
    assert_eq!(second.tests_passed(), 2, "{}", indent(&second.all()));
    for unit in ["__lib_walk_lib_buri", "core_list"] {
        assert_eq!(
            codegen_status(&second, unit),
            "cached",
            "adding an unrelated context recompiled `{unit}`:\n{}",
            indent(&second.all())
        );
    }
}
