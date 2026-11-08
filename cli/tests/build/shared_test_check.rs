//! `buri test` checks each module once, for every suite that reaches it, and
//! each suite reports and builds as if it ran on its own.
use crate::harness::*;

/// A styled helper in `//lib/look`, two JavaScript suites that style it
/// differently and assert that their stylesheets hold only their own styles,
/// and a native suite of a library of its own.
fn repository(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    scratch.write(
        "lib/look/BUILD.buri",
        "library {\n    visibility: [\"//visibility:public\"]\n}\n",
    );
    scratch.write(
        "lib/look/lib.buri",
        "from \"ui/node\" import * as ui;\n\
         from \"ui/node\" import { Node };\n\
         from \"ui/style\" import { Style };\n\n\
         export fn card<C>(styles: [Style], children: [Node<C>]): Node<C> {\n    \
         ui.stack({ styles: [.Group(styles)], children: children })\n}\n\n\
         export fn word(): Str {\n    \"shared\"\n}\n",
    );
    for (pkg, px, other) in [("a", 285, 300), ("b", 300, 285)] {
        scratch.write(
            &format!("lib/{pkg}/BUILD.buri"),
            "library {\n    dependencies: [\"//lib/look\"]\n\n    \
             test {\n        sources: [\"test/t.buri\"]\n        backends: [JS]\n    }\n}\n",
        );
        scratch.write(
            &format!("lib/{pkg}/lib.buri"),
            &format!(
                "from \"ui/node\" import * as ui;\n\
                 from \"ui/node\" import {{ Node }};\n\
                 from \"//lib/look\" import {{ card, word }};\n\n\
                 export fn view<C>(): Node<C> {{\n    \
                 card([.MinWidth(.Px({px}))], [ui.text({{ content: .Const(word()) }})])\n}}\n\n\
                 export fn name(): Str {{\n    word()\n}}\n"
            ),
        );
        scratch.write(
            &format!("lib/{pkg}/test/t.buri"),
            &format!(
                "from \"//lib/{pkg}\" import {{ name, view }};\n\
                 from \"core/testing/assert\" import * as assert;\n\
                 from \"platform/effect\" import {{ Ui }};\n\
                 from \"platform/effect/testing\" import {{ headless, render, stylesheet }};\n\n\
                 test \"{pkg} names itself\" {{\n    assert.equal(name(), \"shared\");\n}}\n\n\
                 test \"{pkg} has only its own styles\" {{\n    \
                 let ctx = context {{\n        Ui: headless(),\n    }};\n    \
                 let _ = render(ctx, view());\n    \
                 assert.isTrue(stylesheet().contains(\".minw-{px}{{min-width:{px}px}}\"));\n    \
                 assert.isFalse(stylesheet().contains(\"minw-{other}\"));\n}}\n"
            ),
        );
    }
    scratch.write(
        "lib/n/BUILD.buri",
        "library {\n    test { sources: [\"test/t.buri\"] }\n}\n",
    );
    scratch.write("lib/n/lib.buri", "export fn one(): Int {\n    1\n}\n");
    scratch.write(
        "lib/n/test/t.buri",
        "from \"//lib/n\" import { one };\n\
         from \"core/testing/assert\" import * as assert;\n\n\
         test \"one is one\" {\n    assert.equal(one(), 1);\n}\n",
    );
    scratch
}

/// The `modules loaded` count of a profiled cold `buri test //...`.
fn modules_loaded(scratch: &Scratch) -> String {
    let run = scratch.run_with_env(&["test", "//..."], &[("BURI_PROFILE", "1")]);
    run.ok().says("5 passed");
    let all = run.all();
    all.lines()
        .find_map(|l| l.strip_prefix("modules loaded "))
        .map(|n| n.trim().to_string())
        .unwrap_or_else(|| panic!("no count:\n{}", indent(&all)))
}

/// **A cold test run loads each module once.** Seven modules: the helper's
/// library, and each suite's library and test source. When each suite was
/// loaded and checked on its own, `//lib/look` was loaded once for each of
/// the two suites that reach it.
#[test]
fn a_cold_test_run_loads_each_module_once() {
    let scratch = repository("shared-test-check-once");
    assert_eq!(modules_loaded(&scratch), "7");
}

/// **The check the lint reads is the same one.** With `check_during_build`,
/// the lint's findings come from the run's one check, so no module is loaded
/// again for it.
#[test]
fn a_cold_test_run_that_lints_loads_each_module_once() {
    let scratch = repository("shared-test-check-lint");
    scratch.write("REPO.buri", "lint {\n    check_during_build: true\n}\n");
    assert_eq!(modules_loaded(&scratch), "7");
}

/// **A suite's bundle is the same bytes whatever else the run checks.**
#[test]
fn a_suites_bundle_does_not_depend_on_what_else_is_tested() {
    let scratch = repository("shared-test-check-bundle");
    let bundle = |pkg: &str| scratch.read(&format!(".buri/out/node/lib/{pkg}/test-library.mjs"));
    scratch.run(&["test", "//..."]).ok().says("5 passed");
    let together = [bundle("a"), bundle("b")];
    std::fs::remove_dir_all(scratch.path(".buri")).unwrap();
    for (pkg, was) in ["a", "b"].iter().zip(&together) {
        scratch
            .run(&["test", &format!("//lib/{pkg}")])
            .ok()
            .says("2 passed");
        assert!(bundle(pkg) == *was, "lib/{pkg}'s bundle differs");
    }
}

/// **A type error in a shared library is reported for each suite that
/// reaches it**, as each reports it alone, and the same way on every run.
#[test]
fn a_type_error_in_a_shared_library_is_reported_for_each_suite() {
    let scratch = repository("shared-test-check-error");
    let lib = scratch
        .read("lib/look/lib.buri")
        .replace("    \"shared\"\n", "    1\n");
    scratch.write("lib/look/lib.buri", &lib);
    let run = scratch.run(&["test", "//..."]);
    run.exits(1).says("1 passed");
    let at = "--> lib/look/lib.buri:9:23";
    assert_eq!(run.stderr.matches(at).count(), 2, "{}", indent(&run.all()));
    for pkg in ["a", "b"] {
        let alone = scratch.run(&["test", &format!("//lib/{pkg}")]);
        assert_eq!(
            alone.stderr.matches(at).count(),
            1,
            "{}",
            indent(&alone.all())
        );
    }
    std::fs::remove_dir_all(scratch.path(".buri")).unwrap();
    assert_eq!(scratch.run(&["test", "//..."]).stderr, run.stderr);
}
