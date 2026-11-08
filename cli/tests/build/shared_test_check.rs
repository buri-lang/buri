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

/// A chain of `n` UI libraries, each with a JavaScript suite. Each library's
/// `frame` adds its own padding and hands the styles on to the one before,
/// and the first gives them a node, so which classes a frame can be handed
/// depends on which suites a program holds. Each suite asserts that its
/// stylesheet holds its own width and not the next library's.
fn ui_chain(scratch: &Scratch, n: usize) {
    for k in 0..n {
        let (deps, frame) = match k.checked_sub(1) {
            Some(before) => (
                format!("    dependencies: [\"//lib/u{before}\"]\n\n"),
                format!(
                    "from \"//lib/u{before}\" import * as before;\n\n\
                     export fn frame<C>(styles: [Style], children: [Node<C>]): Node<C> {{\n    \
                     before.frame([.Group(styles), .Padding(.Px({k}))], children)\n}}\n"
                ),
            ),
            None => (
                String::new(),
                String::from(
                    "from \"ui/node\" import * as ui;\n\n\
                     export fn frame<C>(styles: [Style], children: [Node<C>]): Node<C> {\n    \
                     ui.stack({ styles: [.Group(styles)], children: children })\n}\n",
                ),
            ),
        };
        let px = 300 + k;
        scratch.write(
            &format!("lib/u{k}/BUILD.buri"),
            &format!(
                "library {{\n{deps}    visibility: [\"//visibility:public\"]\n\n    \
                 test {{\n        sources: [\"test/t.buri\"]\n        backends: [JS]\n    }}\n}}\n"
            ),
        );
        scratch.write(
            &format!("lib/u{k}/lib.buri"),
            &format!(
                "from \"ui/node\" import {{ Node }};\n\
                 from \"ui/style\" import {{ Style }};\n\
                 {frame}\n\
                 let width: Style = .MinWidth(.Px({px}));\n\n\
                 export fn view<C>(children: [Node<C>]): Node<C> {{\n    \
                 frame([width], children)\n}}\n"
            ),
        );
        scratch.write(
            &format!("lib/u{k}/test/t.buri"),
            &format!(
                "from \"//lib/u{k}\" import {{ view }};\n\
                 from \"core/testing/assert\" import * as assert;\n\
                 from \"platform/effect\" import {{ Ui }};\n\
                 from \"platform/effect/testing\" import {{ headless, render, stylesheet }};\n\n\
                 test \"u{k} has only its own width\" {{\n    \
                 let ctx = context {{\n        Ui: headless(),\n    }};\n    \
                 let _ = render(ctx, view([]));\n    \
                 assert.isTrue(stylesheet().contains(\".minw-{px}{{min-width:{px}px}}\"));\n    \
                 assert.isFalse(stylesheet().contains(\"minw-{next}\"));\n}}\n",
                next = px + 1
            ),
        );
    }
}

/// The `style declarations walked` count of a profiled cold
/// `buri test <target>`, which passes `passed` tests.
fn styles_walked(scratch: &Scratch, target: &str, passed: usize) -> u64 {
    std::fs::remove_dir_all(scratch.path(".buri")).ok();
    let run = scratch.run_with_env(&["test", target], &[("BURI_PROFILE", "1")]);
    run.ok();
    assert_eq!(run.tests_passed(), passed, "{}", indent(&run.all()));
    let all = run.all();
    all.lines()
        .find_map(|l| l.strip_prefix("style declarations walked "))
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or_else(|| panic!("no count:\n{}", indent(&all)))
}

/// **A cold test run walks each styled declaration once, whatever else it
/// tests.** Each UI suite of a chain holds every library before it, and each
/// suite's styles were extracted over all of them again, so the run walked
/// the first library's once per suite. Now testing the whole chain walks
/// what testing its last suite alone does.
#[test]
fn a_cold_test_run_walks_each_styled_declaration_once() {
    let scratch = Scratch::repo("shared-test-check-styles-once");
    ui_chain(&scratch, 4);
    let alone = styles_walked(&scratch, "//lib/u3", 1);
    assert_eq!(styles_walked(&scratch, "//...", 4), alone);
}

/// **A style a shared library can't make static is refused for each suite
/// that reaches it**, as each refuses it alone, and the same way on every run.
#[test]
fn a_style_a_shared_library_cant_make_static_is_refused_for_each_suite() {
    let scratch = Scratch::repo("shared-test-check-style-refused");
    ui_chain(&scratch, 3);
    let lib = scratch
        .read("lib/u0/lib.buri")
        .replace("import { Style }", "import { Color, Style }")
        .replace("frame([width]", "frame([width, highlighted(.Rgb(1, 2, 3))]");
    scratch.write(
        "lib/u0/lib.buri",
        &format!(
            "{lib}\nfn highlighted(accent: Color): Style {{\n    \
             .On(.Hover, [.Background(accent)])\n}}\n"
        ),
    );
    let run = scratch.run(&["test", "//..."]);
    run.exits(1).says("3 failed to compile");
    let at = "--> lib/u0/lib.buri:16:18";
    assert_eq!(run.stderr.matches(at).count(), 3, "{}", indent(&run.all()));
    std::fs::remove_dir_all(scratch.path(".buri")).unwrap();
    let alone = scratch.run(&["test", "//lib/u2"]);
    assert_eq!(
        alone.stderr.matches(at).count(),
        1,
        "{}",
        indent(&alone.all())
    );
    std::fs::remove_dir_all(scratch.path(".buri")).unwrap();
    assert_eq!(scratch.run(&["test", "//..."]).stderr, run.stderr);
}

/// **A declaration one suite's program answers differently is the only one
/// walked again for it.** The chain's first library matches on a style the
/// later ones give a node, so its suite walks that function again; what it
/// walks after that it takes from the shared run.
#[test]
fn a_suite_walks_again_only_what_its_program_answers_differently() {
    let scratch = Scratch::repo("shared-test-check-styles-again");
    ui_chain(&scratch, 4);
    let lib = scratch.read("lib/u0/lib.buri").replace(
        "from \"ui/style\" import { Style };\n",
        "from \"ui/style\" import { Style };\n\n\
         export fn tone(n: Int): Style {\n    \
         if (n > 0) { .Padding(.Px(3)) } else { .Padding(.Px(4)) }\n}\n\n\
         export fn padded(): Bool {\n    \
         match (tone(1)) {\n        .Padding(_) => true,\n        _ => false,\n    }\n}\n",
    );
    scratch.write("lib/u0/lib.buri", &lib);
    let lib = scratch
        .read("lib/u1/lib.buri")
        .replace(".Padding(.Px(1))", "before.tone(1)");
    scratch.write("lib/u1/lib.buri", &lib);
    let test = scratch
        .read("lib/u0/test/t.buri")
        .replace("import { view }", "import { padded, view }");
    scratch.write(
        "lib/u0/test/t.buri",
        &format!(
            "{test}\ntest \"u0 matches a padding\" {{\n    \
             assert.isTrue(padded());\n    \
             assert.isFalse(stylesheet().contains(\"p-3\"));\n}}\n"
        ),
    );
    let alone = styles_walked(&scratch, "//lib/u3", 1);
    assert_eq!(styles_walked(&scratch, "//...", 5), alone + 1);
}

/// **A style one suite gives a node and another only matches on stays a
/// value in the second**, and puts nothing in its stylesheet, when both are
/// tested together.
#[test]
fn a_style_only_one_suite_gives_a_node_stays_a_value_in_the_other() {
    let scratch = repository("shared-test-check-style-value");
    let look = scratch.read("lib/look/lib.buri");
    scratch.write(
        "lib/look/lib.buri",
        &format!(
            "{look}\nexport fn tone(n: Int): Style {{\n    \
             if (n > 0) {{ .Padding(.Px(3)) }} else {{ .Padding(.Px(4)) }}\n}}\n"
        ),
    );
    let a = scratch
        .read("lib/a/lib.buri")
        .replace(
            "card([.MinWidth(.Px(285))]",
            "card([.MinWidth(.Px(285)), tone(word().length())]",
        )
        .replace("import { card, word }", "import { card, tone, word }");
    scratch.write("lib/a/lib.buri", &a);
    let b = scratch
        .read("lib/b/lib.buri")
        .replace("import { card, word }", "import { card, tone, word }");
    scratch.write(
        "lib/b/lib.buri",
        &format!(
            "{b}\nexport fn padded(): Bool {{\n    \
             match (tone(word().length())) {{\n        .Padding(_) => true,\n        \
             _ => false,\n    }}\n}}\n"
        ),
    );
    let test = scratch
        .read("lib/b/test/t.buri")
        .replace("import { name, view }", "import { name, padded, view }");
    scratch.write(
        "lib/b/test/t.buri",
        &format!(
            "{test}\ntest \"b matches a padding\" {{\n    \
             assert.isTrue(padded());\n    \
             let ctx = context {{\n        Ui: headless(),\n    }};\n    \
             let _ = render(ctx, view());\n    \
             assert.isFalse(stylesheet().contains(\"p-3\"));\n}}\n"
        ),
    );
    scratch.run(&["test", "//lib/b"]).ok().says("3 passed");
    std::fs::remove_dir_all(scratch.path(".buri")).unwrap();
    scratch.run(&["test", "//..."]).ok().says("6 passed");
}
