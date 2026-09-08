//! What makes an action hermetic, and what two builds of one tree have to
//! agree about.
//!
//! Hermeticity here is a property of the type system rather than of a
//! confinement the toolchain applies: every ambient read is a `$host_*`
//! intrinsic, `core/host` is importable only from the module exporting `main`,
//! and a test's capabilities are fakes the runner injects. Nothing in an action
//! has a *name* for the environment, the clock, the filesystem, or the network,
//! so there is nothing for an operating-system sandbox to confine. The toolchain
//! applies none.
//!
//! That moves the burden of proof onto verification, and this is where it lands:
//!
//! - **Reproducibility.** Two builds of one tree, in two directories, byte for
//!   byte — the check that would catch a toolchain bug leaking an intrinsic or a
//!   code generator embedding a path.
//! - **A perturbed parent environment.** The same tree built and tested with
//!   junk variables and a different `TZ` in the parent, producing the same
//!   artifact bytes and the same suite result.
//! - **The cache under contention**, since a cache is only worth having if it
//!   cannot hold a wrong answer.
//!
//! The compile-time half — a test source being unable to import `core/host` at
//! all — is pinned by the reject corpus, which is where a rule about what does
//! not compile belongs.
//!
//! ```text
//! BURI_BLESS=1 cargo test -p buri --test build hermeticity::    # record the goldens
//! ```
use crate::harness::*;

use std::path::Path;

/// The corpus: the closed platform enum, and `--check-reproducible`.
#[test]
fn hermeticity_rules() {
    run_corpus(&tests_dir().join("repositories/hermeticity"), "hermeticity", 2);
}

// ---------------------------------------------------------------------------
// The action's spawn, which is about determinism rather than confinement
// ---------------------------------------------------------------------------

/// The environment an action's process is started with: cleared, then exactly
/// two constants, both of which are the clock.
///
/// Named individually so that a third variable cannot be slipped in without
/// this failing. "Explicit" is only a claim if the list is short enough to
/// write down.
#[test]
fn an_action_is_spawned_with_an_explicit_environment() {
    let mut cmd = buri::build::spawn::command(&js_runtime())
        .unwrap_or_else(|| panic!("`{}` is not on PATH", js_runtime()));
    let vars: Vec<String> =
        cmd.get_envs().filter_map(|(k, v)| v.map(|_| k.to_string_lossy().to_string())).collect();
    assert_eq!(vars.len(), 2, "an action's environment holds more than the clock: {vars:?}");
    assert!(vars.contains(&"TZ".to_string()));
    assert!(vars.contains(&"SOURCE_DATE_EPOCH".to_string()));

    // And what the child actually observes, through the same call the test
    // runner makes. `TZ=UTC` is what renders the fixed epoch as the instant the
    // specification names rather than as whatever the machine is set to.
    let dir = std::env::temp_dir().join(format!("buri-spawn-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a directory for the probe");
    let script = dir.join("probe.mjs");
    std::fs::write(
        &script,
        "const e = process.env;\n\
         console.log('vars=' + Object.keys(e).sort().join(','));\n\
         console.log('at=' + new Date(0).toISOString());\n",
    )
    .expect("writing the probe");
    let out = cmd.arg(&script).output().expect("the javascript runtime runs");
    let seen = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        seen.contains("vars=SOURCE_DATE_EPOCH,TZ"),
        "the parent's environment reached the child:\n{}",
        indent(&seen)
    );
    assert!(
        seen.contains("at=1970-01-01T00:00:00.000Z"),
        "the epoch did not render as 1970-01-01T00:00:00Z:\n{}",
        indent(&seen)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The test that carries the load in this model.
///
/// Nothing in an action can *read* the parent's environment, so the risk is not
/// a leak a program arranges — it is a machine whose time zone or locale
/// silently changes what a build produces. Both halves of the toolchain's output
/// are compared across a parent environment stuffed with junk and set to a time
/// zone twelve hours away: the artifact's bytes, and the suite's verdict.
#[test]
fn a_perturbed_parent_environment_changes_neither_the_bytes_nor_the_verdict() {
    let clean = Scratch::copy_of("perturbed-clean", &example_repo());
    let dirty = Scratch::copy_of("perturbed-dirty", &example_repo());

    // A time zone half a day away, a locale, and a variable whose name nothing
    // should know — the three shapes of ambient state a build could
    // accidentally read.
    const PERTURBED: &[(&str, &str)] = &[
        ("TZ", "Pacific/Auckland"),
        ("LANG", "tr_TR.UTF-8"),
        ("LC_ALL", "tr_TR.UTF-8"),
        ("BURI_HERMETICITY_JUNK", "this must not reach an artifact"),
        ("SOURCE_DATE_EPOCH", "1700000000"),
    ];
    let run = |scratch: &Scratch, args: &[&str], perturb: bool| -> Run {
        scratch.run_with_env(args, if perturb { PERTURBED } else { &[] })
    };

    run(&clean, &["build", "//cmd/web"], false).ok();
    run(&dirty, &["build", "//cmd/web"], true).ok();
    let a = std::fs::read(clean.artifact("cmd/web")).expect("the clean artifact");
    let b = std::fs::read(dirty.artifact("cmd/web")).expect("the perturbed artifact");
    assert!(!a.is_empty(), "the artifact is empty, so the comparison proves nothing");
    assert_eq!(
        buri::build::actions::first_difference(&a, &b),
        None,
        "the parent's environment changed the artifact's bytes"
    );

    // The suite half. `--force` on both, so neither answer can come from a
    // cache entry the other one wrote.
    let clean_tests = run(&clean, &["test", "//lib/money", "--force"], false);
    let dirty_tests = run(&dirty, &["test", "//lib/money", "--force"], true);
    clean_tests.ok();
    dirty_tests.ok();
    assert!(clean_tests.tests_passed() >= 1, "the suite asserted nothing");
    assert_eq!(
        clean_tests.tests_passed(),
        dirty_tests.tests_passed(),
        "the parent's environment changed a suite's verdict:\n{}",
        indent(&dirty_tests.all())
    );
}

/// The compile-time half, which is the layer the whole model rests on: a test
/// source has no name for ambient state, so there is nothing to confine.
///
/// The rule itself is pinned by the reject corpus. What this adds is that it
/// holds for a *test source* specifically — the one kind of module somebody
/// would most plausibly argue should be allowed a real capability, because it is
/// not shipped.
#[test]
fn a_test_source_cannot_import_core_host() {
    let scratch = Scratch::repo("host-import-in-a-test");
    scratch.write("lib/probe/BUILD.buri", "library {\n  test { sources: [\"test/env.buri\"] }\n}\n");
    scratch.write("lib/probe/lib.buri", "export fn identity(n: Int): Int { n }\n");
    scratch.write(
        "lib/probe/test/env.buri",
        "from \"core/testing/assert\" import * as assert;\n\
         from \"core/host\" import * as host;\n\n\
         test \"reads the machine\" {\n  assert.equal(1, 1);\n}\n",
    );
    scratch
        .run(&["test", "//lib/probe"])
        .exits(1)
        .says("host-import")
        .says("importable only from the module that exports `main`");
}

// ---------------------------------------------------------------------------
// Reproducibility
// ---------------------------------------------------------------------------

/// "The same commit, on any machine, produces byte-identical artifacts."
///
/// Two checkouts in two directories, so that a path which leaked into an
/// artifact would leak differently in each — which is the failure this is most
/// likely to catch and the reason the two copies are not one copy built twice.
#[test]
fn two_checkouts_of_one_tree_build_identical_bytes() {
    let one = Scratch::copy_of("reproducible-one", &example_repo());
    let two = Scratch::copy_of("reproducible-two", &example_repo());
    assert_ne!(one.root, two.root);

    one.run(&["build", "//cmd/web"]).ok();
    two.run(&["build", "//cmd/web"]).ok();

    let a = std::fs::read(one.artifact("cmd/web")).expect("the first artifact");
    let b = std::fs::read(two.artifact("cmd/web")).expect("the second artifact");
    assert!(!a.is_empty(), "the artifact is empty, so the comparison proves nothing");
    assert_eq!(
        buri::build::actions::first_difference(&a, &b),
        None,
        "two checkouts of one tree produced different artifacts, first differing at byte {:?}",
        buri::build::actions::first_difference(&a, &b)
    );

    // And the flag that lets a repository ask the same question of itself.
    one.run(&["build", "//cmd/web", "--check-reproducible"]).ok();
    one.run(&["build", "//cmd/web", "--check-reproducible", "--release"]).ok();
}

/// A repository whose only source is written by a generator, for the two rows
/// below.
///
/// The tool's `main` binds `FileSystemRead` and `FileSystemWrite` as well as the three
/// `codegen.run` needs, which the language allows — a bound is a floor and not
/// a ceiling — and `marked` is the switch that turns the extra two into an
/// answer that depends on what has happened before. So one repository states
/// both halves: a generator is reproducible, and one that is not is caught.
fn generated_only(name: &str, marked: bool) -> Scratch {
    let scratch = Scratch::repo(name);
    scratch.write(
        "lib/wire/BUILD.buri",
        "library {\n    generators: [{ tool: \"//cmd/gen\", inputs: [\"units.txt\"] }]\n\n    \
         visibility: [\"//visibility:public\"]\n}\n",
    );
    scratch.write("lib/wire/units.txt", "3\n");
    scratch.write("lib/wire/lib.buri", "from \"//lib/wire/units\" export { width };\n");
    scratch
        .write("cmd/gen/BUILD.buri", "binary {\n    outputs: [{ platform: JS }]\n}\n");
    scratch.write("cmd/gen/main.buri", &generator(marked));
    scratch.write(
        "cmd/app/BUILD.buri",
        "binary {\n    dependencies: [\"//lib/wire\"]\n\n    outputs: [{ platform: JS }]\n}\n",
    );
    scratch.write(
        "cmd/app/main.buri",
        "from \"core/effect\" import { Allocator, Stdout };\n\
         from \"core/host\" import * as host;\n\
         from \"core/io\" import * as io;\n\
         from \"//lib/wire\" import { width };\n\n\
         export fn main(): Result<(), Str> {\n  \
         let ctx = context { Allocator: host.alloc, Stdout: host.stdout };\n  \
         let _ = io.println(ctx, \"width=${width}\").ignore();\n  \
         .Ok(())\n\
         }\n",
    );
    scratch
}

/// The tool [`generated_only`] runs.
///
/// With `marked`, the number it writes is one more the second time it runs,
/// because it leaves a file behind and looks for it. That is the smallest
/// possible generator whose answer is not a function of its request, and it is
/// deliberate rather than random: a test that relied on two clocks or two
/// random numbers differing would be a test that usually passes.
fn generator(marked: bool) -> String {
    let extra = match marked {
        false => String::new(),
        true => "  let tally = fs.readText(ctx, path.of(ctx, \"tally.txt\")).withDefault(\"\");\n  \
                 let _ = fs\n    .writeText(ctx, path.of(ctx, \"tally.txt\"), tally.concat(ctx, \"x\"))\n    \
                 .ignore();\n  let n = n0 + tally.len();\n"
            .to_string(),
    };
    let plain = match marked {
        false => "  let n = n0;\n".to_string(),
        true => String::new(),
    };
    format!(
        r#"from "core/effect" import {{ Allocator, Stdin, Stdout }};
from "core/fs" import * as fs;
from "core/fs" import {{ FileSystemRead, FileSystemWrite }};
from "core/host" import * as host;
from "core/io" import * as io;
from "core/json" import * as json;
from "core/json" import {{ Json }};
from "core/list" import * as list;
from "core/path" import * as path;
from "core/str" import * as str;

export fn main(): Result<(), Str> {{
  let ctx = context {{
    Allocator: host.alloc,
    FileSystemRead: host.fs,
    FileSystemWrite: host.fs,
    Stdin: host.stdin,
    Stdout: host.stdout,
  }};
  let line = io.readLine(ctx).okOr("no request")?;
  let request = json.parse(ctx, line).mapErr(fn(_e) => "the request is not JSON")?;
  let n0 = firstInput(request).withDefault("0").trim().toInt().withDefault(0);
{extra}{plain}  let source = str.format(ctx, "export let width: Int = ${{n}};\n");
  let unit: Json = .Object([
    ("name", .Str("units")),
    ("text", .Str(source)),
    ("anchors", .Array(list.empty())),
  ]);
  let response: Json = .Object([
    ("modules", .Array([unit])),
    ("diagnostics", .Array(list.empty())),
  ]);
  let _ = io.println(ctx, "${{json.stringify(ctx, response)}}").ignore();
  .Ok(())
}}

fn firstInput(request: Json): Option<Str> {{
  let inputs = match (request) {{
    .Object(fields) => fields.find(fn(f) => f.0 == "inputs").map(fn(f) => f.1),
    _ => .None,
  }};
  let items = match (inputs.withDefault(.Null)) {{
    .Array(xs) => xs,
    _ => list.empty(),
  }};
  let pair = match (items.get(0).withDefault(.Null)) {{
    .Array(xs) => xs,
    _ => list.empty(),
  }};
  match (pair.get(1).withDefault(.Null)) {{
    .Str(s) => .Some(s),
    _ => .None,
  }}
}}
"#
    )
}

/// **Two clean checkouts of a repository whose code is generated produce the
/// same bytes, and `--check-reproducible` says so about either of them.**
///
/// `two_checkouts_of_one_tree_build_identical_bytes` asks this of the worked
/// monorepo, which declares no generator, so the one action whose output comes
/// out of a *user's program* was outside every reproducibility claim in the
/// suite. Two directories rather than one built twice, for that test's reason:
/// a path that leaked into a generated module would leak differently in each.
#[test]
fn two_checkouts_of_a_generated_tree_build_identical_bytes() {
    let one = generated_only("reproducible-generated-one", false);
    let two = generated_only("reproducible-generated-two", false);
    assert_ne!(one.root, two.root);

    one.run(&["run", "//cmd/app"]).ok().says("width=3");
    two.run(&["run", "//cmd/app"]).ok().says("width=3");

    let a = std::fs::read(one.artifact("cmd/app")).expect("the first artifact");
    let b = std::fs::read(two.artifact("cmd/app")).expect("the second artifact");
    assert!(!a.is_empty(), "the artifact is empty, so the comparison proves nothing");
    assert_eq!(
        buri::build::actions::first_difference(&a, &b),
        None,
        "two checkouts of one generated tree produced different artifacts"
    );

    one.run(&["build", "//cmd/app", "--check-reproducible"]).ok();
}

/// ...and the negative twin: a generator whose answer is not a function of its
/// request is caught by the same flag.
///
/// `core/codegen`'s `run` bounds what it hands `generate` to `Alloc + Stdin +
/// Stdout`, and `cli/tests/reject/generator_reaches_beyond_its_context` is the
/// half of that a type error covers. A bound is a *floor*, though: a `main`
/// that binds the disk as well hands `generate` the disk, and nothing at
/// compile time says otherwise. This is the check that does — the one
/// `build/generators.md` sends a reader to.
#[test]
fn a_generator_whose_answer_is_not_a_function_of_its_request_is_caught() {
    let scratch = generated_only("reproducible-generated-drift", true);
    // It builds, and the first answer is the honest one: nothing here is
    // refused, which is why the flag has to be what asks.
    scratch.run(&["run", "//cmd/app"]).ok().says("width=");

    let run = scratch.run(&["build", "//cmd/app", "--check-reproducible"]);
    assert_ne!(
        run.code, 0,
        "a generator that answers differently the second time passed --check-reproducible:\n{}",
        run.all()
    );
    run.says("differs between two builds of the same tree");
}

// ---------------------------------------------------------------------------
// Concurrency
// ---------------------------------------------------------------------------

/// "All commands are safe to run concurrently; a file lock serializes cache
/// writes" (CLI.md:25).
///
/// Two builds of one repository started at once, from cold, so both reach the
/// point of writing the same entries. What is asserted afterwards is not only
/// that both succeeded but that the cache still answers — an entry half-written
/// by one process and read by the other would be a stale artifact rather than a
/// failed build, and the second is much easier to notice.
#[test]
fn two_concurrent_builds_leave_the_cache_intact() {
    let scratch = Scratch::copy_of("concurrent-build", &example_repo());

    // Four at once, from cold, all racing for the same entries. `//cmd/web` and
    // the libraries under it are the part of the example repository this
    // toolchain has a backend for; what is being contended is the cache, not
    // the native backend that does not exist yet.
    let spawn = || {
        buri_command()
            .args(["build", "//cmd/web", "//lib/money", "//lib/ledger", "--color=never"])
            .current_dir(&scratch.root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("the buri binary runs")
    };
    let racing: Vec<_> = (0..4).map(|_| spawn()).collect();
    for (n, child) in racing.into_iter().enumerate() {
        let out = child.wait_with_output().expect("a concurrent build finishes");
        assert!(
            out.status.success(),
            "concurrent build {n} exited {:?}:\n{}",
            out.status.code(),
            indent(&String::from_utf8_lossy(&out.stderr))
        );
    }

    // Nothing half-written: a `.tmp` left in the cache is an interrupted write
    // that a reader could have picked up.
    let mut entries = 0usize;
    let mut leftovers = Vec::new();
    walk(&scratch.path(".buri/cache"), &mut |p| {
        let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
        if name.starts_with('.') {
            return;
        }
        if name.contains("tmp") {
            leftovers.push(name);
            return;
        }
        entries += 1;
        assert!(
            std::fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false),
            "{} is an empty cache entry",
            p.display()
        );
    });
    assert!(leftovers.is_empty(), "half-written cache entries: {leftovers:?}");
    assert!(entries > 0, "the two builds wrote nothing, so the cache proves nothing");

    // And the cache still answers, with the artifact the program's behaviour
    // says it should be.
    scratch.run(&["build", "//cmd/web"]).ok().says("cached");
    let after = std::fs::read(scratch.artifact("cmd/web")).expect("the artifact");
    scratch.run(&["build", "//cmd/web", "--force"]).ok();
    let forced = std::fs::read(scratch.artifact("cmd/web")).expect("the artifact");
    assert_eq!(
        buri::build::actions::first_difference(&after, &forced),
        None,
        "the cache served an artifact a fresh build disagrees with"
    );
}

fn walk(dir: &Path, f: &mut dyn FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.filter_map(Result::ok) {
        let p = e.path();
        if p.is_dir() {
            walk(&p, f);
        } else {
            f(&p);
        }
    }
}
