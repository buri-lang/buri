//! `buri build` checks each module once, for every program and every entry
//! that reaches it, and each artifact behaves as if built on its own.
use crate::harness::*;

/// A library with a styled helper, and two programs that use it: `//cmd/app`
/// is a page and a node program out of one `main.buri`, and `//cmd/other` is a
/// second page that styles the helper differently.
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
    scratch.write(
        "cmd/app/BUILD.buri",
        "binary {\n    dependencies: [\"//lib/look\"]\n\n    outputs: [\n        \
         { platform: \"web\", entries: [{ name: \"main\", function: \"page\" }] },\n        \
         { platform: \"node\" },\n    \
         ]\n}\n",
    );
    scratch.write(
        "cmd/app/main.buri",
        "from \"core/io\" import * as io;\n\
         from \"platform/effect\" import { Allocator, Stdout, Ui, Watch };\n\
         from \"node\" import { NodeHost };\n\
         from \"ui/node\" import * as ui;\n\
         from \"web\" import { WebHost };\n\
         from \"//lib/look\" import { card, word };\n\n\
         export fn page(host: WebHost): Result<(), Str> {\n    \
         let ctx = context {\n        Allocator: host.alloc,\n        Stdout: host.stdout,\n        \
         Ui: host.ui,\n        Watch: host.watch,\n    };\n    \
         let _ = io.println(ctx, \"page\").mapErr(fn(_e) => \"could not write\")?;\n    \
         let _ = io.println(ctx, word()).mapErr(fn(_e) => \"could not write\")?;\n    \
         ui.mount(ctx, card([.MinWidth(.Px(285))], [ui.text({ content: .Const(\"app\") })]), [])\n}\n\n\
         export fn main(host: NodeHost): Result<(), Str> {\n    \
         let ctx = context {\n        Allocator: host.alloc,\n        Stdout: host.stdout,\n    };\n    \
         let _ = io.println(ctx, \"node\").mapErr(fn(_e) => \"could not write\")?;\n    \
         io.println(ctx, word()).mapErr(fn(_e) => \"could not write\")\n}\n",
    );
    scratch.write(
        "cmd/other/BUILD.buri",
        "binary {\n    dependencies: [\"//lib/look\"]\n    outputs: [{ platform: \"web\" }]\n}\n",
    );
    scratch.write(
        "cmd/other/main.buri",
        "from \"core/io\" import * as io;\n\
         from \"platform/effect\" import { Allocator, Stdout, Ui, Watch };\n\
         from \"ui/node\" import * as ui;\n\
         from \"web\" import { WebHost };\n\
         from \"//lib/look\" import { card };\n\n\
         export fn main(host: WebHost): Result<(), Str> {\n    \
         let ctx = context {\n        Allocator: host.alloc,\n        Stdout: host.stdout,\n        \
         Ui: host.ui,\n        Watch: host.watch,\n    };\n    \
         let _ = io.println(ctx, \"other\").mapErr(fn(_e) => \"could not write\")?;\n    \
         ui.mount(ctx, card([.MinWidth(.Px(300))], [ui.text({ content: .Const(\"other\") })]), [])\n}\n",
    );
    scratch
}

/// **Two entries and a second program share a styled helper, and each
/// artifact runs as its own.** Each page's stylesheet holds the class its own
/// call styles the helper with, and each artifact prints what its entry does.
#[test]
fn entries_and_programs_sharing_a_styled_helper_each_run_as_their_own() {
    let scratch = repository("shared-check-styled");
    scratch.run(&["build", "//..."]).ok();
    let sheet = |dir: &str| scratch.read(&format!(".buri/out/web/{dir}/main.css"));
    assert!(
        sheet("cmd/app").contains(".minw-285{min-width:285px}"),
        "{}",
        sheet("cmd/app")
    );
    assert!(
        sheet("cmd/other").contains(".minw-300{min-width:300px}"),
        "{}",
        sheet("cmd/other")
    );
    assert_eq!(
        scratch.exec_js_in("web", "cmd/app").ok().stdout,
        "page\nshared\n"
    );
    assert_eq!(scratch.exec_js("cmd/app").ok().stdout, "node\nshared\n");
    assert_eq!(
        scratch.exec_js_in("web", "cmd/other").ok().stdout,
        "other\n"
    );
}

/// **A program's artifacts are the same bytes whatever else the build holds.**
/// Built beside the other program, and built alone, each artifact is the
/// same: a helper's styles hold only the classes its own program gives it.
#[test]
fn a_programs_artifacts_do_not_depend_on_what_else_is_built() {
    let scratch = repository("shared-check-hermetic");
    scratch.run(&["build", "//..."]).ok();
    let out = scratch.path(".buri/out");
    let together = crate::programs_at_once::files(&out);
    std::fs::remove_dir_all(scratch.path(".buri")).unwrap();
    scratch.run(&["build", "//cmd/app"]).ok();
    scratch.run(&["build", "//cmd/other"]).ok();
    let alone = crate::programs_at_once::files(&out);
    assert_eq!(
        together.keys().collect::<Vec<_>>(),
        alone.keys().collect::<Vec<_>>()
    );
    for (path, bytes) in &together {
        assert!(alone[path] == *bytes, "{path} differs");
    }
}

/// **A cold build loads each module once, and draws the style graph once for
/// the check and once for each program**, however many entries reach them.
/// There are three modules: the library and each program's `main.buri`. A
/// program's styles come from its own callers alone, so each draws its own
/// graph; the check of the library draws one over everything. When each
/// output loaded and styled its whole program again, this was 9 modules and 4
/// graphs. PERFORMANCE.md §6.97.
#[test]
fn a_cold_build_of_programs_sharing_a_library_loads_each_module_once() {
    let scratch = repository("shared-check-once");
    let run = scratch.run_with_env(&["build", "//..."], &[("BURI_PROFILE", "1")]);
    run.ok();
    let all = run.all();
    let count = |what: &str| {
        all.lines()
            .find_map(|l| l.strip_prefix(what))
            .map(|n| n.trim().to_string())
    };
    assert_eq!(
        count("modules loaded ").as_deref(),
        Some("3"),
        "{}",
        indent(&all)
    );
    assert_eq!(
        count("style graphs drawn ").as_deref(),
        Some("3"),
        "{}",
        indent(&all)
    );
}

/// What `buri build <label>` says on standard error, alone.
fn alone(scratch: &Scratch, label: &str) -> String {
    scratch.run(&["build", label]).stderr.clone()
}

/// **An entry that doesn't fit its platform is refused on its own.** The node
/// entry takes the page's host: the build says what building that binary alone
/// says, and the page beside it and the other program still run.
#[test]
fn an_entry_that_does_not_fit_its_platform_is_refused_alone() {
    let scratch = repository("shared-check-entry");
    let main = scratch.read("cmd/app/main.buri").replace(
        "export fn main(host: NodeHost)",
        "export fn main(host: WebHost)",
    );
    scratch.write("cmd/app/main.buri", &main);
    let run = scratch.run(&["build", "//..."]);
    run.exits(1).says("cmd/app/main.buri:");
    assert_eq!(run.stderr, alone(&scratch, "//cmd/app"));
    assert_eq!(
        scratch.exec_js_in("web", "cmd/app").ok().stdout,
        "page\nshared\n"
    );
    assert_eq!(
        scratch.exec_js_in("web", "cmd/other").ok().stdout,
        "other\n"
    );
}

/// **A type error in a shared library is reported once for each target that
/// reaches it**, as each reports it alone, and the same way on every run.
#[test]
fn a_type_error_in_a_shared_library_is_reported_once_for_each_target() {
    let scratch = repository("shared-check-error");
    let lib = scratch
        .read("lib/look/lib.buri")
        .replace("    \"shared\"\n", "    1\n");
    scratch.write("lib/look/lib.buri", &lib);
    let run = scratch.run(&["build", "//..."]);
    run.exits(1).says("lib/look/lib.buri:");
    let at = "--> lib/look/lib.buri:9:23";
    assert_eq!(run.stderr.matches(at).count(), 3, "{}", indent(&run.stderr));
    for label in ["//cmd/app", "//cmd/other", "//lib/look"] {
        assert_eq!(alone(&scratch, label).matches(at).count(), 1);
    }
    assert_eq!(scratch.run(&["build", "//..."]).stderr, run.stderr);
}

/// A native program at `cmd/<name>` that depends on `//lib/named`, holding
/// `types` and a `main` that keeps a `Label` and a `Named` in options.
fn native(scratch: &Scratch, name: &str, types: &str, label: &str) {
    scratch.write(
        &format!("cmd/{name}/BUILD.buri"),
        "binary {\n    dependencies: [\"//lib/named\"]\n    \
         outputs: [{ platform: \"native\" }]\n}\n",
    );
    scratch.write(
        &format!("cmd/{name}/main.buri"),
        &format!(
            "from \"core/io\" import * as io;\n\
             from \"native\" import {{ NativeHost }};\n\
             from \"platform/effect\" import {{ Allocator, Stdout }};\n\
             from \"//lib/named\" import {{ Named, first }};\n\n\
             {types}\n\n\
             export fn main(host: NativeHost): Result<(), Str> {{\n    \
             let ctx = context {{ Allocator: host.alloc, Stdout: host.stdout }};\n    \
             let label: Option<Label> = .Some({label});\n    \
             let named = first([Named {{ name: \"named\", n: 1 }}]);\n    \
             let kept = [label, label];\n    \
             let text = match (label) {{ .Some(l) => l.text, .None => \"none\" }};\n    \
             let name = match (named) {{ .Some(n) => n.name, .None => \"none\" }};\n    \
             io.println(ctx, \"${{text}} ${{name}} ${{kept.length()}}\").mapErr(fn(_e) => \"could not write\")\n}}\n"
        ),
    );
}

/// **A native program's executable is the same bytes whatever else the build
/// holds.** `//cmd/a` declares types of its own, and `//cmd/b` keeps values of
/// its own and the library's in options: built beside `//cmd/a` and built
/// alone, `//cmd/b` is the same executable, and prints the same.
#[test]
fn a_native_programs_executable_does_not_depend_on_what_else_is_built() {
    let scratch = Scratch::repo("shared-check-native");
    scratch.write(
        "lib/named/BUILD.buri",
        "library {\n    visibility: [\"//visibility:public\"]\n}\n",
    );
    scratch.write(
        "lib/named/lib.buri",
        "export struct Named { export name: Str, export n: Int }\n\n\
         export fn first(xs: [Named]): Option<Named> {\n    xs.get(0)\n}\n",
    );
    native(
        &scratch,
        "a",
        "struct One { a: Str }\nstruct Two { b: Option<One> }\n\
         struct Label { text: Str, other: Option<Two> }",
        "Label { text: \"label\", other: .None }",
    );
    native(
        &scratch,
        "b",
        "struct Label { text: Str }",
        "Label { text: \"label\" }",
    );
    let out = scratch.path(".buri/out");
    scratch.run(&["build", "//..."]).ok();
    let together = crate::programs_at_once::files(&out);
    std::fs::remove_dir_all(scratch.path(".buri")).unwrap();
    scratch.run(&["build", "//cmd/b"]).ok();
    let alone = crate::programs_at_once::files(&out);
    let b: Vec<&String> = alone.keys().filter(|p| p.contains("cmd/b")).collect();
    assert!(!b.is_empty(), "{:?}", alone.keys());
    for path in b {
        assert!(together[path] == alone[path], "{path} differs");
        if path.ends_with("/b") {
            let run = std::process::Command::new(out.join(path)).output().unwrap();
            assert_eq!(String::from_utf8_lossy(&run.stdout), "label named 2\n");
        }
    }
}
