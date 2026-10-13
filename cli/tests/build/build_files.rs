//! Each way a `BUILD.buri` or `REPO.buri` field can be written wrong, and the
//! finding `buri build` reports for it.
//!
//! A row is one repository: a library at `//lib/x`, and the row's text as the
//! file it names. Each must fail and report the finding the row expects.

use crate::harness::*;

/// The file a row writes, its text, and the `code message` of the finding it
/// draws.
const ROWS: &[(&str, &str, &str)] = &[
    ("lib/x/BUILD.buri", "library { test { timeout_seconds: -1 } }", "build-field-mismatch `timeout_seconds` holds a non-negative number, found a number"),
    ("lib/x/BUILD.buri", "library { test { timeout_seconds: 4294967296 } }", "build-field-mismatch `timeout_seconds` holds a non-negative number, found a number"),
    ("lib/x/BUILD.buri", "library { backends: [QQQQ] }", "build-unknown-word `QQQQ` is not a backend"),
    ("lib/x/BUILD.buri", "library { generators: [\"x\"] }", "build-field-mismatch `generators` holds a block, found a string"),
    ("tools/x/BUILD.buri", "tool { generate { accepts: [\"json\"] } }", "build-field-mismatch `accepts` holds a block, found a string"),
    ("lib/x/BUILD.buri", "binary { outputs: [{ variant: \"x\" }] }", "output-missing-platform"),
    ("lib/x/BUILD.buri", "binary { outputs: [{ platform: 5 }] }", "build-field-mismatch `platform` holds a platform name, found a number"),
    ("platform/x/BUILD.buri", "platform { entry: \"x\" }", "build-field-mismatch `entry` holds a block, found a string"),
    ("platform/x/BUILD.buri", "platform { entry { name: \"main\" backend: JS variant_required: yes } }", "build-field-mismatch `variant_required` holds `true` or `false`, found an identifier"),
    ("lib/x/BUILD.buri", "binary { outputs: [{ platform: \"//p\", entries: [\"x\"] }] }", "build-field-mismatch `entries` holds a block, found a string"),
    ("lib/x/BUILD.buri", "binary { outputs: [{ platform: \"//p\", entries: [{ name: \"main\" }] }] }", "entry-missing-field"),
    ("lib/x/BUILD.buri", "binary { outputs: [{ platform: \"//p\", entries: [{ function: \"main\" }] }] }", "entry-missing-field"),
    ("lib/x/BUILD.buri", "binary { outputs: [{ platform: \"//p\", entries: [{ name: 1, function: \"main\" }] }] }", "build-field-mismatch `name` holds a string"),
    ("lib/x/BUILD.buri", "binary { outputs: [{ platform: \"node\", entries: [{ name: \"main\", function: \"1x\" }] }] }", "invalid-entry-function"),
    ("lib/x/BUILD.buri", "binary { outputs: [{ platform: \"node\" }] visibility: [\"//visibility:public\"] }", "binary-field"),
    ("REPO.buri", "language: \"x\"", "build-field-mismatch `language` holds a block, found a string"),
    ("REPO.buri", "language { name: 1 }", "build-field-mismatch `name` holds a string"),
    ("REPO.buri", "language { name: \"lines\" extensions: [\".\"] }", "invalid-extension"),
    ("REPO.buri", "language { name: \"lines\" extensions: [\".a/b\"] }", "invalid-extension"),
    ("REPO.buri", "language { name: \"lines\" extensions: [\". x\"] }", "invalid-extension"),
    ("REPO.buri", "language { name: \"json\" extensions: [\".jsn\"] }\nlanguage { name: \"lines\" extensions: [\".jsn\"] }", "duplicate-extension"),
    ("REPO.buri", "lint { check_during_build: 1 }", "build-field-mismatch `check_during_build` holds `true` or `false`, found a number"),
    ("REPO.buri", "lint { rules { default: ENABLE } }", "build-unknown-word `ENABLE` is not a rule default"),
    ("REPO.buri", "lint { rules { default: QQQQ } }", "build-unknown-word `QQQQ` is not a rule default"),
    ("REPO.buri", "tag: \"x\"", "tag-not-block"),
    ("REPO.buri", "tag { name: \"a\" requires { tags: [\"b\"] } }", "tags-under-requires"),
    ("REPO.buri", "tag { name: \"a\" }\ntag { name: \"a\" }", "duplicate-tag"),
    ("tools/x/BUILD.buri", "tool { format { accepts: [] } }", "build-unknown-field"),
    ("lib/x/BUILD.buri", "binary { outputs: [{ platform: \"node\", artifact_name: \"app\" }, { variant: \"x\" }] }", "output-missing-platform"),
    ("lib/x/BUILD.buri", "binary { outputs: [{ platform: \"node\" }] platforms: [\"node\"] }", "binary-field"),
    ("lib/x/BUILD.buri", "library { visibility: [\"//visibility:friends\"] }", "unknown-visibility `//visibility:friends` is not a visibility"),
    // The text format itself, written wrong.
    ("lib/x/BUILD.buri", "library {} }", "build-file-syntax expected a field name, found `}`"),
    ("lib/x/BUILD.buri", "library { backends: [QQQQ] }\n# a comment with no line break after it", "build-unknown-word `QQQQ` is not a backend"),
    ("lib/x/BUILD.buri", "library { sources: [\"a", "build-file-syntax unterminated string"),
    ("lib/x/BUILD.buri", "library { sources: [\"a\\", "build-file-syntax unterminated string"),
    ("lib/x/BUILD.buri", "library { sources: [\"a\\tb\\n\\r\\\"\\é.buri\"] }", "unknown-source a\tb\n\r\"é.buri does not exist"),
    ("lib/x/BUILD.buri", "library { backends: [_X] }", "build-unknown-word `_X` is not a backend"),
    ("lib/x/BUILD.buri", "library { sources: [99999999999999999999] }", "build-file-syntax `99999999999999999999` is not a number"),
];

#[test]
fn build_file_fields_written_wrong() {
    let mut failures = Vec::new();
    for (file, text, want) in ROWS {
        let s = Scratch::repo("build-files");
        s.write("lib/x/lib.buri", "export let nothing: Int = 0;\n");
        s.write("lib/x/BUILD.buri", "library {}\n");
        s.write(file, text);
        let run = s.run(&["build", "//lib/x", "--error-format=json"]);
        let mut said = Vec::new();
        for line in run
            .stdout
            .lines()
            .chain(run.stderr.lines())
            .filter(|l| l.starts_with('{'))
        {
            let d = buri::json::parse(line).unwrap();
            let text = |path: &str| {
                d.at(path)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string()
            };
            said.push(format!("{} {}", text("code"), text("message")));
        }
        if run.code == 0 || !said.iter().any(|s| s.starts_with(want)) {
            failures.push(format!(
                "{file}: {text}\n  exit {}, expected {want:?}, found {said:#?}",
                run.code
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// `//cmd/...` in a library's `visibility` admits `//cmd` and a package below
/// it, and refuses one beside it.
#[test]
fn a_recursive_visibility_admits_the_packages_below_it() {
    let s = Scratch::repo("recursive-visibility");
    s.write("lib/x/lib.buri", "export let answer: Int = 42;\n");
    s.write(
        "lib/x/BUILD.buri",
        "library {\n    visibility: [\"//cmd/...\"]\n}\n",
    );
    s.write(
        "cmd/app/BUILD.buri",
        "binary {\n    dependencies: [\"//lib/x\"]\n\n    outputs: [{ platform: \"node\" }]\n}\n",
    );
    s.write(
        "cmd/app/main.buri",
        "from \"node\" import { NodeHost };\nfrom \"//lib/x\" import { answer };\n\nexport fn main(_host: NodeHost): Result<(), Str> {\n    let _ = answer;\n    .Ok(())\n}\n",
    );
    s.run(&["build", "//cmd/app"]).ok();
    // `//cmd` itself is inside `//cmd/...` too.
    s.write("cmd/BUILD.buri", &s.read("cmd/app/BUILD.buri"));
    s.write("cmd/main.buri", &s.read("cmd/app/main.buri"));
    s.run(&["build", "//cmd"]).ok();
    s.write(
        "tools/app/BUILD.buri",
        "binary {\n    dependencies: [\"//lib/x\"]\n\n    outputs: [{ platform: \"node\" }]\n}\n",
    );
    s.write("tools/app/main.buri", &s.read("cmd/app/main.buri"));
    s.run(&["build", "//tools/app"]).exits(1).says("visibility");
}

/// Each target pattern `buri build` refuses, and the one that reaches every
/// package by naming the root's subtree.
#[test]
fn target_patterns_written_wrong() {
    let s = Scratch::repo("target-patterns");
    s.write("lib/x/lib.buri", "export let nothing: Int = 0;\n");
    s.write("lib/x/BUILD.buri", "library {}\n");
    for (pattern, said) in [
        (
            "@other//lib/x",
            "`@other//lib/x` names an external repository, which is reserved and unimplemented",
        ),
        (
            "lib/x",
            "`lib/x` is not a label; labels are repository-absolute and start with `//`",
        ),
        ("//lib/x/", "`//lib/x/` has a trailing slash"),
        (
            "//lib...",
            "`//lib...` is not a label; the only pattern forms are `//pkg/...` and `//...`",
        ),
    ] {
        s.run(&["build", pattern]).exits(2).says(said);
    }
    s.run(&["build", "///..."]).ok();
}
