//! A real repository's `buri test //...`, scaled down.
//!
//! One user repository of 142k lines, 82 suites and 2,224 tests took 494 s for
//! a cold `buri test //...`, and 374 s after an edit that changed one comment.
//! Every cause of that had a shape a small repository can show: many packages
//! over one shared library, native and JavaScript suites, suites that fail and a
//! suite that does not compile, and a package whose module a tool of the
//! repository generates. [`monorepo`] writes that shape, and each test here is
//! one thing a second run over it may not redo.
//!
//! The claims are read off what `buri test --explain` prints and off the report
//! itself, never off timings. The time a cold run takes is
//! `repositories::monorepo_test_budget`'s claim.
use crate::harness::*;

/// The suites that pass, as package names under `libs/`.
const PASSING: [&str; 7] = ["n0", "n1", "n2", "n3", "j0", "j1", "wire"];

/// The suites with one failing test each: one native, one JavaScript.
const FAILING: [&str; 2] = ["failing", "failing_js"];

/// The suite whose library does not type check.
const BROKEN: &str = "broken";

/// The library every other package depends on.
const BASE: &str = "libs/base/lib.buri";

/// Writes the repository: a shared library, the suites above, and a tool.
fn monorepo(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    scratch.write("libs/base/BUILD.buri", "library {\n  visibility: [\"//...\"]\n}\n");
    scratch.write(BASE, "export fn base(): Int {\n  20\n}\n");
    for n in ["n0", "n1", "n2", "n3"] {
        package(&scratch, n, "", 21);
    }
    for n in ["j0", "j1"] {
        package(&scratch, n, "    backends: [JS]\n", 21);
    }
    package(&scratch, "failing", "", 99);
    package(&scratch, "failing_js", "    backends: [JS]\n", 99);
    package(&scratch, BROKEN, "", 21);
    scratch.write(&format!("libs/{BROKEN}/lib.buri"), "export fn value(): Int { missing() }\n");
    generated_package(&scratch);
    scratch
}

/// `libs/<name>`, whose `value()` is `base() + 1`, and a suite of three tests,
/// the last of which expects `want`.
fn package(scratch: &Scratch, name: &str, backends: &str, want: i64) {
    scratch.write(
        &format!("libs/{name}/BUILD.buri"),
        &format!(
            "library {{\n  dependencies: [\"//libs/base\"]\n  test {{\n    sources: [\"test/{name}.buri\"]\n{backends}  }}\n}}\n"
        ),
    );
    scratch.write(
        &format!("libs/{name}/lib.buri"),
        "from \"//libs/base\" import { base };\n\nexport fn value(): Int { base() + 1 }\n",
    );
    scratch.write(
        &format!("libs/{name}/test/{name}.buri"),
        &format!(
            "from \"//libs/{name}\" import {{ value }};\n\
             from \"core/testing/assert\" import * as assert;\n\
             \ntest \"is the base plus one\" {{\n  assert.equal(value(), 21);\n}}\n\
             \ntest \"is positive\" {{\n  assert.equal(value() > 0, true);\n}}\n\
             \ntest \"is what the suite wants\" {{\n  assert.equal(value(), {want});\n}}\n"
        ),
    );
}

/// `libs/wire`, whose one module `//tool/gen` generates from `size.txt`.
fn generated_package(scratch: &Scratch) {
    scratch.write(
        "libs/wire/BUILD.buri",
        "library {\n  generators: [\n    { tool: \"//tool/gen\", inputs: [\"size.txt\"] },\n  ]\n\n  test {\n    sources: [\"test/wire.buri\"]\n  }\n}\n",
    );
    scratch.write("libs/wire/lib.buri", "from \"//libs/wire/units\" export { size };\n");
    scratch.write("libs/wire/size.txt", "four");
    scratch.write(
        "libs/wire/test/wire.buri",
        "from \"//libs/wire\" import { size };\n\
         from \"core/testing/assert\" import * as assert;\n\
         \ntest \"the input is not empty\" {\n  assert.equal(size > 0, true);\n}\n",
    );
    scratch.write("tool/gen/BUILD.buri", "tool {\n    generate {}\n}\n");
    scratch.write(
        "tool/gen/tool.buri",
        "from \"core/buri/ast\" import * as ast;\n\
         from \"core/str\" import * as str;\n\
         from \"core/tool\" import { Generated, GenerateRequest };\n\
         from \"platform/effect\" import { Allocator };\n\
         \n\
         /// A module holding how many characters its one input has.\n\
         export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<Str>): Generated {\n    \
             let text = request.inputs.get(0).map(fn(i) => i.value).withDefault(\"\");\n    \
             let source = str.format(ctx, \"export let size: Int = ${text.length()};\\n\");\n    \
             let modules = match (ast.parse(ctx, \"\", source)) {\n        \
                 .Ok(parsed) => [(\"units\", parsed)],\n        \
                 .Err(_) => [],\n    \
             };\n    \
             Generated { modules: modules, diagnostics: [], needs: [] }\n\
         }\n",
    );
}

/// Every `--explain` row as `(status, action, label)`.
fn explained(run: &Run) -> Vec<(String, String, String)> {
    run.stdout
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<&str>>())
        .filter(|f| f.len() == 5 && matches!(f[0], "run" | "cached" | "keyed"))
        .map(|f| (f[0].to_string(), f[1].to_string(), f[2].to_string()))
        .collect()
}

/// What `--explain` said about `<action> //libs/<name>`.
fn status(run: &Run, action: &str, name: &str) -> String {
    let label = format!("//libs/{name}");
    explained(run)
        .into_iter()
        .find(|(_, a, l)| a == action && *l == label)
        .map(|(s, _, _)| s)
        .unwrap_or_else(|| panic!("no `{action} {label}` row in:\n{}", indent(&run.all())))
}

/// The rows that did work other than running a test: a check, a unit's code
/// or a link.
fn work(run: &Run) -> Vec<String> {
    explained(run)
        .into_iter()
        .filter(|(s, a, _)| s == "run" && a != "test")
        .map(|(_, a, l)| format!("{a} {l}"))
        .collect()
}

/// What a run printed about the code: the failures, without the `--explain`
/// rows and without the summary line, whose time moves between two runs.
fn report(run: &Run) -> String {
    run.stdout
        .lines()
        .filter(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            let row = f.len() == 5 && matches!(f[0], "run" | "cached" | "keyed");
            !row && !l.contains(" passed, ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The summary line's counts, without the time and the cached count.
fn counts(run: &Run) -> String {
    let line = run
        .stdout
        .lines()
        .rev()
        .find(|l| l.contains(" passed, "))
        .unwrap_or_else(|| panic!("no summary line in:\n{}", indent(&run.all())));
    line.split(" (").next().unwrap_or(line).to_string()
}

/// The cold run every test starts from: twenty-three tests pass, the two
/// failing suites fail one test each, and the broken suite does not compile.
///
/// `None` on a host whose toolchain cannot run a native suite.
fn cold(scratch: &Scratch) -> Option<Run> {
    let run = scratch.run(&["test", "//...", "--explain"]);
    if run.stderr.contains("test-run-unavailable")
        && crate::harness::ci::skipped("build", "this toolchain cannot run a suite on its own host")
    {
        return None;
    }
    run.exits(1);
    assert_eq!(
        counts(&run),
        "23 passed, 2 failed, 0 skipped, 1 failed to compile",
        "{}",
        indent(&run.all())
    );
    for name in FAILING {
        assert!(run.stdout.contains(&format!("FAIL //libs/{name} ")), "{}", indent(&run.all()));
    }
    Some(run)
}

/// A second run over the tree the first reported on ran the failing suites
/// and no passing one, and reported exactly what the first did.
fn ran_only_the_failures(first: &Run, then: &Run, what: &str) {
    then.exits(1);
    for name in PASSING {
        assert_eq!(
            status(then, "test", name),
            "cached",
            "{what} re-ran //libs/{name}, which passed and could not have changed:\n{}",
            indent(&then.all())
        );
    }
    for name in FAILING {
        assert_eq!(status(then, "test", name), "run", "{}", indent(&then.all()));
    }
    assert!(
        then.stdout.contains("19 cached)"),
        "{what} did not serve the nineteen passing tests from the cache:\n{}",
        indent(&then.all())
    );
    assert_eq!(counts(then), counts(first), "{what} counted differently:\n{}", indent(&then.all()));
    assert_eq!(report(then), report(first), "{what} reported the failures differently");
    assert_eq!(then.stderr, first.stderr, "{what} printed the compile errors differently");
}

/// A warm `buri test //...` builds nothing and runs only the suites that
/// failed.
///
/// A failing verdict is never cached, so those suites run every time. What they
/// do not need is their build: the native suite runs the binary it linked, the
/// JavaScript one runs the bundle it emitted, and the suite that did not
/// compile prints the errors it printed without being checked again.
#[test]
fn a_warm_run_builds_nothing_and_runs_only_the_failures() {
    let scratch = monorepo("monorepo-warm");
    let Some(first) = cold(&scratch) else { return };

    let warm = scratch.run(&["test", "//...", "--explain"]);
    ran_only_the_failures(&first, &warm, "a warm run");
    assert_eq!(work(&warm), Vec::<String>::new(), "a warm run built:\n{}", indent(&warm.all()));
    for name in FAILING.into_iter().chain([BROKEN]) {
        assert_eq!(status(&warm, "build", name), "cached", "{}", indent(&warm.all()));
    }
}

/// A comment and whitespace edit to the library every suite reads re-runs no
/// passing suite and builds nothing at all, and an edit that changes what it
/// computes re-runs every suite above it.
///
/// The edit adds a comment line, a trailing comment and indentation. A doc
/// comment is not in it, because a doc comment is part of what a suite reads.
#[test]
fn a_comment_edit_to_the_shared_library_re_runs_no_passing_suite() {
    let scratch = monorepo("monorepo-comment");
    let Some(first) = cold(&scratch) else { return };

    scratch.write(
        BASE,
        "// Where every value starts.\nexport fn base(): Int {\n        20 // twenty\n}\n",
    );
    let commented = scratch.run(&["test", "//...", "--explain"]);
    ran_only_the_failures(&first, &commented, "a comment edit");
    assert_eq!(work(&commented), Vec::<String>::new(), "a comment edit built:\n{}", indent(&commented.all()));

    // The negative twin: every value moves, so every suite above it runs.
    scratch.write(BASE, "export fn base(): Int {\n  30\n}\n");
    let behaviour = scratch.run(&["test", "//...", "--explain"]);
    behaviour.exits(1);
    for name in PASSING.iter().chain(FAILING.iter()).filter(|name| **name != "wire") {
        assert_eq!(status(&behaviour, "test", name), "run", "{}", indent(&behaviour.all()));
    }
    assert_eq!(status(&behaviour, "test", "wire"), "cached", "{}", indent(&behaviour.all()));
}

/// A comment edit that moves the lines a failure is reported at builds nothing,
/// and prints exactly what a cold run over the edited tree prints.
///
/// The failing suites' test files and the broken library gain comment lines
/// above what they report, a trailing comment and some indentation, so every
/// location and every quoted line moves. The suites still run the binary and
/// the bundle they built, and the broken suite still repeats its errors without
/// a check. What moves is where those are reported, and that has to be where a
/// cold run reports them.
#[test]
fn a_comment_edit_above_a_failure_reports_it_where_a_cold_run_does() {
    let scratch = monorepo("monorepo-moved");
    let Some(first) = cold(&scratch) else { return };
    let edit = |scratch: &Scratch| {
        scratch.write(BASE, "// Where every value starts.\nexport fn base(): Int {\n    20 // twenty\n}\n");
        for name in FAILING {
            let file = format!("libs/{name}/test/{name}.buri");
            let moved = scratch.read(&file).replace(
                "\ntest \"is what",
                "\n// The one that fails.\n\n  test \"is what",
            );
            scratch.write(&file, &format!("// Three tests.\n// One of them fails.\n\n{moved}"));
        }
        scratch.write(
            &format!("libs/{BROKEN}/lib.buri"),
            "// Broken on purpose.\n\nexport fn value(): Int {    missing() } // still\n",
        );
    };
    edit(&scratch);
    let warm = scratch.run(&["test", "//...", "--explain"]);
    warm.exits(1);
    assert_eq!(work(&warm), Vec::<String>::new(), "a comment edit built:\n{}", indent(&warm.all()));
    assert_ne!(report(&warm), report(&first), "the edit moved no failure");
    assert_ne!(warm.stderr, first.stderr, "the edit moved no error");

    let fresh = monorepo("monorepo-moved-cold");
    edit(&fresh);
    let Some(cold) = cold(&fresh) else { return };
    assert_eq!(report(&warm), report(&cold), "a warm run reported a failure where a cold run does not");
    assert_eq!(warm.stderr, cold.stderr, "a warm run printed a compile error where a cold run does not");
    assert_eq!(counts(&warm), counts(&cold), "{}", indent(&warm.all()));
}

/// An edit to a generator's input re-runs the one suite that reads the module
/// it generates, and builds nothing for any other.
#[test]
fn an_edit_to_a_generators_input_re_runs_only_the_suite_that_reads_it() {
    let scratch = monorepo("monorepo-generator");
    let Some(first) = cold(&scratch) else { return };

    scratch.write("libs/wire/size.txt", "fourteen");
    let edited = scratch.run(&["test", "//...", "--explain"]);
    edited.exits(1);
    assert_eq!(status(&edited, "test", "wire"), "run", "{}", indent(&edited.all()));
    for name in PASSING.iter().filter(|name| **name != "wire") {
        assert_eq!(status(&edited, "test", name), "cached", "{}", indent(&edited.all()));
    }
    for row in work(&edited) {
        assert!(
            row.contains("//libs/wire"),
            "an edit to //libs/wire's generator input built `{row}`:\n{}",
            indent(&edited.all())
        );
    }
    assert_eq!(counts(&edited), counts(&first), "{}", indent(&edited.all()));
}

/// Every link after the first runs the linker itself, without starting the C
/// driver.
///
/// A link through the driver is three processes: a wrapper script, clang, and
/// the linker. clang pays libLLVM's start-up just to print a command line, and
/// a repository of dozens of suites paid it once per binary. So the driver is
/// asked once what it would run, and that line is replayed. The driver here is
/// a script on `CC` that writes down how it was called and hands on to the real
/// one: across two runs of four binaries each it is asked once, and links
/// nothing itself.
///
/// gcc prints a command that cannot be replayed, so this is a clang's claim.
#[cfg(unix)]
#[test]
fn links_after_the_first_do_not_start_the_c_driver() {
    use std::os::unix::fs::PermissionsExt;
    let real_cc = std::env::var("CC").unwrap_or_else(|_| String::from("cc"));
    let banner = std::process::Command::new(&real_cc).arg("--version").output();
    if !banner.is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains("clang"))
        && crate::harness::ci::skipped("build", "the C driver is not a clang")
    {
        return;
    }
    let scratch = Scratch::repo("monorepo-direct-link");
    scratch.write("libs/base/BUILD.buri", "library {\n  visibility: [\"//...\"]\n}\n");
    scratch.write(BASE, "export fn base(): Int {\n  20\n}\n");
    for name in ["n0", "n1", "n2", "n3"] {
        package(&scratch, name, "", 21);
    }
    let calls = scratch.path("driver-calls");
    let driver = scratch.write(
        "fake-cc",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{calls}'\nexec '{real_cc}' \"$@\"\n",
            calls = calls.display()
        ),
    );
    std::fs::set_permissions(&driver, std::fs::Permissions::from_mode(0o755)).unwrap();
    let driver = driver.display().to_string();
    // A batch limit of one byte puts every suite in a binary of its own.
    let env = [("CC", driver.as_str()), ("BURI_TEST_BATCH_BYTES", "1")];
    let run = || {
        let run = scratch.run_with_env(&["test", "//...", "--explain"], &env);
        if run.stderr.contains("test-run-unavailable") {
            return None;
        }
        run.ok();
        assert_eq!(run.tests_passed(), 12, "{}", indent(&run.all()));
        let links = explained(&run).iter().filter(|(s, a, _)| s == "run" && a == "link").count();
        assert_eq!(links, 4, "the four suites were not linked apart:\n{}", indent(&run.all()));
        Some(run)
    };
    if run().is_none() {
        crate::harness::ci::skipped("build", "this toolchain cannot run a suite on its own host");
        return;
    }
    // A body edit every suite reads, so every binary is linked again.
    scratch.write(BASE, "export fn base(): Int {\n  19 + 1\n}\n");
    run().expect("the second run ran where the first did");

    let calls = std::fs::read_to_string(&calls).unwrap_or_default();
    let dry_run = format!("-{}", "###");
    let asked = calls.lines().filter(|l| l.contains(&dry_run)).count();
    let linked: Vec<&str> =
        calls.lines().filter(|l| !l.contains(&dry_run) && l.contains("-o artifact")).collect();
    assert_eq!(asked, 1, "the driver was asked what it would run {asked} times:\n{calls}");
    assert!(linked.is_empty(), "eight links started the C driver {} times:\n{calls}", linked.len());
}
