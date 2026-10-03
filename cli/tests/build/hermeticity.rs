//! What makes an action hermetic, and what two builds of one tree have to
//! agree about.
//!
//! Hermeticity here is a property of the type system rather than of a
//! confinement the toolchain applies: every ambient read is a `$host_*`
//! intrinsic, `platform/host` is importable only from a platform's `platform.buri`,
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
//! The compile-time half — a test source being unable to import `platform/host` at
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
fn a_test_source_cannot_import_a_host_module() {
    let module = "platform/host";
    {
        let scratch = Scratch::repo("host-import-in-a-test");
        scratch.write("lib/probe/BUILD.buri", "library {\n  test { sources: [\"test/env.buri\"] }\n}\n");
        scratch.write("lib/probe/lib.buri", "export fn identity(n: Int): Int { n }\n");
        scratch.write(
            "lib/probe/test/env.buri",
            &format!(
                "from \"core/testing/assert\" import * as assert;\n\
                 from \"{module}\" import * as host;\n\n\
                 test \"reads the machine\" {{\n  assert.equal(1, 1);\n}}\n"
            ),
        );
        scratch
            .run(&["test", "//lib/probe"])
            .exits(1)
            .says("host-import-outside-platform")
            .says(&format!("\"{module}\" is importable only by a platform"));
    }
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
/// `reads_the_disk` is the switch that asks for `FileSystemRead` beside the
/// allocator a tool is handed, which is the smallest tool whose answer could
/// depend on something other than its request.
fn generated_only(name: &str, reads_the_disk: bool) -> Scratch {
    let scratch = Scratch::repo(name);
    scratch.write(
        "lib/wire/BUILD.buri",
        "library {\n    generators: [{ tool: \"//tool/gen\", inputs: [\"units.txt\"] }]\n\n    \
         visibility: [\"//visibility:public\"]\n}\n",
    );
    scratch.write("lib/wire/units.txt", "3\n");
    scratch.write("lib/wire/lib.buri", "from \"//lib/wire/units\" export { width };\n");
    scratch.write("tool/gen/BUILD.buri", "tool {\n    generate {}\n}\n");
    scratch.write("tool/gen/tool.buri", &generator(reads_the_disk));
    scratch.write(
        "cmd/app/BUILD.buri",
        "binary {\n    dependencies: [\"//lib/wire\"]\n\n    outputs: [{ platform: \"node\" }]\n}\n",
    );
    scratch.write(
        "cmd/app/main.buri",
        "from \"platform/effect\" import { Allocator, Stdout };\n\
         from \"node\" import { NodeHost };\n\
         from \"core/io\" import * as io;\n\
         from \"//lib/wire\" import { width };\n\n\
         export fn main(host: NodeHost): Result<(), Str> {\n  \
         let ctx = context { Allocator: host.alloc, Stdout: host.stdout };\n  \
         let _ = io.println(ctx, \"width=${width}\").ignore();\n  \
         .Ok(())\n\
         }\n",
    );
    scratch
}

/// The tool [`generated_only`] runs: `export let width: Int = <the input>;`.
fn generator(reads_the_disk: bool) -> String {
    let bound = match reads_the_disk {
        false => "Allocator",
        true => "Allocator + FileSystemRead",
    };
    format!(
        r#"from "core/buri/ast" import * as ast;
from "platform/effect" import {{ Allocator }};
from "core/fs" import {{ FileSystemRead }};
from "core/str" import * as str;
from "core/tool" import {{ Generated, GenerateRequest }};

export fn generate<C: {bound}>(ctx: C, request: GenerateRequest<Str>): Generated {{
  let n = request.inputs.get(0).map(fn(i) => i.value.trim()).withDefault("0");
  let source = str.format(ctx, "export let width: Int = ${{n}};\n");
  let modules = match (ast.parse(ctx, "", source)) {{
    .Ok(parsed) => [("units", parsed)],
    .Err(_) => [],
  }};
  Generated {{ modules: modules, diagnostics: [], needs: [] }}
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

/// ...and the negative twin: a generator that asks for more than an allocator
/// is refused before it runs.
///
/// A tool's answer is cached under its request, so a tool that could read the
/// disk could give two answers to one question. Its `ctx` has `Allocator` and
/// nothing else, and asking for the disk is refused by name rather than caught
/// afterwards by `--check-reproducible`.
#[test]
fn a_generator_that_asks_for_the_disk_is_refused() {
    let scratch = generated_only("reproducible-generated-disk", true);
    scratch.run(&["run", "//cmd/app"]).exits(1).says("`generate` asks for `FileSystemRead`");
}


// ---------------------------------------------------------------------------
// Concurrency
// ---------------------------------------------------------------------------

/// "Commands can run concurrently" (CLI.md), with no lock on cache writes.
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

/// The toolchain in every key is the id the linker wrote into the `buri`
/// binary, and it has to be one: a rebuilt `buri` with different code must
/// name itself differently, and the same binary must name itself the same way
/// every time it is asked.
///
/// Two real binaries linked by this build stand in for a rebuild — the `buri`
/// under test and this test's own executable, which share a toolchain, a
/// profile and the linker flags `cli/build.rs` asks for, and differ in code.
/// Each must carry a linker-written id rather than falling back to hashing its
/// bytes, which is what proves the flags reach the link on this host, and
/// `buri version --verbose` must print the one this reads.
#[test]
fn the_toolchain_is_named_by_its_linker_id() {
    use buri::build::exe_identity::linker_identity;
    let compiler = Path::new(buri());
    let other = std::env::current_exe().expect("this test's own executable");

    let named = linker_identity(compiler).expect("`buri` carries no LC_UUID or build id");
    assert!(
        named.starts_with("macho-uuid:") || named.starts_with("elf-build-id:"),
        "{named}"
    );
    assert_eq!(linker_identity(compiler).as_deref(), Some(&*named), "one binary, two names");
    let different = linker_identity(&other).expect("the test binary carries no linker id");
    assert_ne!(named, different, "two different binaries share an identity");

    let nowhere = Scratch::empty("toolchain-identity");
    nowhere.run(&["version", "--verbose"]).ok().says(&format!("this executable: {named}\n"));
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
