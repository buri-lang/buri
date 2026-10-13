//! `buri format` at its edges: a platform's surface, a build file that does
//! not parse, a remembered verdict cut short, files no rule lists, inputs
//! that are not there, and files that are not UTF-8.
use crate::harness::*;

const MESSY: &str = "export fn one(): Int {   1 }\n";
const TIDY: &str = "export fn one(): Int {\n    1\n}\n";

/// A library at `lib/a` holding `MESSY`.
fn messy_library(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    scratch.write("lib/a/BUILD.buri", "library {}\n");
    scratch.write("lib/a/lib.buri", MESSY);
    scratch
}

/// A platform's surface declares its entries without a body, as the standard
/// library does, and is laid out with them.
#[test]
fn a_platform_surface_is_laid_out_with_its_bodiless_declarations() {
    let scratch = Scratch::repo("format-platform-surface");
    scratch.write(
        "platform/p/BUILD.buri",
        "platform {\n    entry {\n        name: \"e\"\n        backend: JS\n    }\n}\n",
    );
    scratch.write(
        "platform/p/platform.buri",
        "export struct PHost {}\n\nexport fn e(host:PHost):  Int;\n",
    );
    scratch
        .run(&["format", "--check"])
        .exits(1)
        .says("platform/p/platform.buri");
    scratch
        .run(&["format"])
        .ok()
        .says("formatted platform/p/platform.buri");
    assert_eq!(
        scratch.read("platform/p/platform.buri"),
        "export struct PHost {}\n\nexport fn e(host: PHost): Int;\n"
    );
    scratch.run(&["format", "--check"]).ok();
}

/// A build file that does not parse stops the command, every time: the
/// refusal is never remembered as a verdict.
#[test]
fn a_build_file_that_does_not_parse_stops_format_every_time() {
    let scratch = messy_library("format-broken-build-file");
    scratch.write(
        "lib/a/BUILD.buri",
        "library {\n    sources: [\"lib.buri\"\n}\n",
    );
    for _ in 0..2 {
        scratch
            .run(&["format", "--check"])
            .exits(2)
            .says("error: lib/a/BUILD.buri does not parse");
    }
    scratch.run(&["format"]).exits(2);
    assert_eq!(scratch.read("lib/a/lib.buri"), MESSY);

    scratch.write("lib/a/BUILD.buri", "library {}\n");
    scratch
        .run(&["format", "--check"])
        .exits(1)
        .says("lib/a/lib.buri");
}

/// Every remembered verdict cut short by a byte, as a crash or a full disk
/// can leave it: the next run lays the file out again and says what a run
/// with nothing remembered says.
#[test]
fn a_remembered_verdict_cut_short_is_laid_out_again() {
    let scratch = messy_library("format-verdict-cut-short");
    scratch
        .run(&["format", "--check"])
        .exits(1)
        .says("lib/a/lib.buri");
    let mut cut = 0;
    for entry in cache_entries(&scratch) {
        let file = std::fs::File::options().write(true).open(&entry).unwrap();
        let length = file.metadata().unwrap().len();
        if length > 0 {
            file.set_len(length - 1).unwrap();
            cut += 1;
        }
    }
    assert!(cut > 0, "the check remembered nothing");
    scratch
        .run(&["format", "--check"])
        .exits(1)
        .says("lib/a/lib.buri");
    scratch
        .run(&["format"])
        .ok()
        .says("formatted lib/a/lib.buri");
    assert_eq!(scratch.read("lib/a/lib.buri"), TIDY);
}

/// A JSON file no rule lists is not the repository's to format, even named
/// on the command line, and neither is a rule's input in a language the
/// repository does not know.
#[test]
fn files_no_rule_formats_are_left_as_written() {
    let scratch = Scratch::repo("format-unlisted-files");
    let json = "{\"a\":   1}\n";
    let notes = "not   buri {\n";
    scratch.write("lib/a/data.json", json);
    scratch.write("lib/a/notes.txt", notes);
    scratch.write("lib/a/lib.buri", TIDY);
    scratch.write(
        "lib/a/BUILD.buri",
        "library {\n    generators: [\n        { tool: \"proto\", inputs: [\"notes.txt\"] },\n    ]\n}\n",
    );
    scratch.run(&["format", "--check", "lib/a/data.json"]).ok();
    scratch.run(&["format", "--check"]).ok();
    scratch.run(&["format", "--check", "//lib/a"]).ok();
    scratch.run(&["format"]).ok();
    assert_eq!(scratch.read("lib/a/data.json"), json);
    assert_eq!(scratch.read("lib/a/notes.txt"), notes);
}

/// A listed input or source that is not there is the build's to report:
/// `format` has nothing to lay out.
#[test]
fn a_listed_file_that_is_not_there_has_nothing_to_lay_out() {
    let scratch = Scratch::repo("format-missing-inputs");
    scratch.write("lib/a/lib.buri", TIDY);
    scratch.write(
        "lib/a/BUILD.buri",
        "library {\n    sources: [\"lib.buri\", \"gone.buri\"]\n    generators: [\n        { tool: \"proto\", inputs: [\"s.proto\"] },\n    ]\n}\n",
    );
    scratch.run(&["format", "--check"]).ok();
    scratch.run(&["format", "--check", "//lib/a"]).ok();
}

/// A file that is not UTF-8 is one the formatter could not read, so a
/// `--check` does not pass it, and `format` names it rather than skipping it
/// in silence: a source, a listed schema and a document alike.
#[test]
fn a_file_that_is_not_utf8_is_named_and_fails_the_check() {
    let scratch = Scratch::repo("format-not-utf8");
    scratch.write(
        "lib/a/BUILD.buri",
        "library {\n    generators: [\n        { tool: \"proto\", inputs: [\"s.proto\"] },\n    ]\n}\n",
    );
    scratch.write("lib/a/lib.buri", TIDY);
    let bad = b"export fn two(): Int {\n    \xff 2\n}\n";
    std::fs::write(scratch.path("lib/a/b.buri"), bad).unwrap();
    std::fs::write(
        scratch.path("lib/a/s.proto"),
        b"syntax = \"proto3\";\n\xff\n",
    )
    .unwrap();
    std::fs::write(scratch.path("README.md"), b"\xff\n```buri\nlet x=1\n```\n").unwrap();
    let not_utf8 = "stream did not contain valid UTF-8";
    scratch
        .run(&["format", "--check"])
        .exits(1)
        .says(&format!("cannot read lib/a/b.buri: {not_utf8}"))
        .says(&format!("cannot read lib/a/s.proto: {not_utf8}"))
        .says(&format!("cannot read README.md: {not_utf8}"));
    // A label's files are its build file, its sources and its inputs.
    scratch
        .run(&["format", "--check", "//lib/a"])
        .exits(1)
        .says(&format!("cannot read lib/a/s.proto: {not_utf8}"));
    scratch
        .run(&["format"])
        .ok()
        .says(&format!("cannot read lib/a/b.buri: {not_utf8}"));
    assert_eq!(std::fs::read(scratch.path("lib/a/b.buri")).unwrap(), bad);

    for file in ["lib/a/b.buri", "lib/a/s.proto", "README.md"] {
        std::fs::remove_file(scratch.path(file)).unwrap();
    }
    scratch.run(&["format", "--check"]).ok();
}

/// The files under `.buri/cache`'s two-character directories, which is where
/// its entries are.
fn cache_entries(scratch: &Scratch) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for dir in std::fs::read_dir(scratch.path(".buri/cache"))
        .unwrap()
        .flatten()
    {
        let Ok(entries) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        out.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_file()));
    }
    out
}
