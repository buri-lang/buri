//! What the command line says about how it was asked: no command at all, a
//! flag given a value it can't take, two spellings of a mode, a command or
//! subcommand misspelled, a query that names nothing, and the skills a
//! release stopped shipping.
use crate::harness::*;

fn empty(name: &str) -> Scratch {
    Scratch::empty(name)
}

/// A repository with a library and a binary that depends on it.
fn graph(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    scratch.write(
        "lib/a/BUILD.buri",
        "library {\n    visibility: [\"//visibility:public\"]\n}\n",
    );
    scratch.write("lib/a/lib.buri", "export fn one(): Int {\n    1\n}\n");
    scratch.write(
        "cmd/app/BUILD.buri",
        "binary {\n    dependencies: [\"//lib/a\"]\n\n    outputs: [{ platform: \"node\" }]\n}\n",
    );
    scratch.write(
        "cmd/app/main.buri",
        "from \"node\" import { NodeHost };\n\nexport fn main(_host: NodeHost): Result<(), Str> {\n    .Ok(())\n}\n",
    );
    scratch
}

/// `buri` alone is a question the usage answers, and exits 2 because nothing
/// was asked; asking for help of a command is asking, and exits 0.
#[test]
fn no_command_prints_the_usage_and_exits_2() {
    let scratch = empty("command-line-bare");
    // Run bare: the harness adds a flag to every command it runs.
    let bare = buri_command()
        .current_dir(&scratch.root)
        .output()
        .expect("buri runs");
    let stdout = String::from_utf8_lossy(&bare.stdout);
    assert_eq!(bare.status.code(), Some(2), "{stdout}");
    assert!(
        stdout.contains("the toolchain for the Buri language") && stdout.contains("buri build"),
        "the usage is on stdout: {stdout}"
    );

    scratch
        .run(&["build", "--help"])
        .ok()
        .says("the toolchain for the Buri language");
}

#[test]
fn a_flag_given_a_value_it_cannot_take_is_refused() {
    let scratch = empty("command-line-flag-value");
    scratch
        .run(&["build", "--verbose=yes"])
        .exits(2)
        .says("`--verbose` takes no value");
}

#[test]
fn release_and_debug_together_are_refused() {
    let scratch = empty("command-line-modes");
    scratch
        .run(&["build", "--release", "--debug"])
        .exits(2)
        .says("`--release` and `--debug` are exclusive");
}

#[test]
fn jobs_takes_a_whole_number_from_one() {
    let scratch = empty("command-line-jobs");
    scratch
        .run(&["test", "--jobs=0"])
        .exits(2)
        .says("`--jobs` takes a whole number from 1 up, not `0`");
}

/// `doc` is the mistype an agent reaches for most, and is answered like
/// `docs`.
#[test]
fn doc_is_docs() {
    let scratch = empty("command-line-doc");
    let docs = scratch.run(&["docs", "cli", "version"]);
    docs.ok();
    let doc = scratch.run(&["doc", "cli", "version"]);
    doc.ok();
    assert_eq!(doc.stdout, docs.stdout);
    assert!(!doc.stdout.is_empty());
}

#[test]
fn a_misspelled_command_names_the_nearest_one() {
    let scratch = empty("command-line-misspelled");
    scratch
        .run(&["biuld"])
        .exits(2)
        .says("there is no command `biuld`")
        .says("did you mean `buri build`?");
}

#[test]
fn a_subcommand_nothing_resembles_names_no_nearest_one() {
    let scratch = empty("command-line-no-subcommand");
    scratch
        .run(&["add", "zzzzzzzzzzzz"])
        .exits(2)
        .says("there is no `buri add` subcommand `zzzzzzzzzzzz`")
        .silent_about("did you mean");
}

#[test]
fn version_checks_the_bundled_standard_library_on_request() {
    let scratch = empty("command-line-self-check");
    scratch
        .run(&["version", "--self-check"])
        .ok()
        .says("standard library: ")
        .says(" modules, checked");
}

/// Each query form refuses a label that names no target the same way, and a
/// query that is cut off says so.
#[test]
fn a_query_that_names_nothing_is_refused() {
    let scratch = graph("command-line-queries");
    for form in ["deps", "rdeps", "tags", "platforms", "sources"] {
        scratch
            .run(&["query", &format!("{form}(//nope)")])
            .exits(2)
            .says("error: no target `//nope`");
    }
    scratch
        .run(&["query", "path(//lib/a, //nope)"])
        .exits(2)
        .says("error: no such target");
    scratch
        .run(&["query", "deps(//lib/a"])
        .exits(2)
        .says("is missing its closing parenthesis");
}

/// A release that stops shipping a skill takes it away on the next
/// `buri add skills`, and leaves the directories that were never its own.
#[test]
fn a_skill_no_longer_shipped_is_removed() {
    let scratch = empty("command-line-retired-skill");
    scratch.write(".agent/skills/buri-retired/SKILL.md", "old\n");
    scratch.write(".agent/skills/mine/SKILL.md", "mine\n");
    scratch
        .run(&["add", "skills"])
        .ok()
        .says("removed .agent/skills/buri-retired");
    assert!(!scratch.path(".agent/skills/buri-retired").exists());
    assert_eq!(scratch.read(".agent/skills/mine/SKILL.md"), "mine\n");
}

/// A `.gitignore` the build's entries are merged into, whatever it ends in.
#[test]
fn init_merges_into_a_gitignore_without_a_final_newline() {
    let scratch = empty("command-line-init-no-newline");
    scratch.write(".gitignore", "node_modules/");
    scratch.run(&["init"]).ok().says("updated .gitignore");
    let merged = scratch.read(".gitignore");
    assert!(merged.starts_with("node_modules/\n"), "{merged}");
    assert!(merged.contains(".buri/"), "{merged}");
}

#[test]
fn init_fills_in_an_empty_gitignore() {
    let scratch = empty("command-line-init-empty-ignore");
    scratch.write(".gitignore", "");
    scratch.run(&["init"]).ok().says("updated .gitignore");
    let merged = scratch.read(".gitignore");
    assert!(!merged.starts_with('\n'), "{merged:?}");
    assert!(merged.contains(".buri/"), "{merged}");
}

/// A target that climbs out of a directory that isn't there is the
/// directory it climbs to.
#[test]
fn init_reads_a_target_that_climbs_out_of_nothing() {
    let scratch = empty("command-line-init-dotdot");
    scratch.run(&["init", "nothing/.."]).ok();
    assert!(scratch.path("REPO.buri").is_file());
}

/// A build file `gen` can't write is an error, not a run that said nothing.
#[test]
fn gen_says_when_it_cannot_write_a_build_file() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::repo("command-line-gen-readonly");
    scratch.write(
        "lib/a/BUILD.buri",
        "library {\n    visibility: [\"//...\"]\n}\n",
    );
    scratch.write("lib/a/lib.buri", "export fn one(): Int {\n    1\n}\n");
    scratch.write("lib/a/extra.buri", "export fn two(): Int {\n    2\n}\n");
    let build = scratch.path("lib/a/BUILD.buri");
    std::fs::set_permissions(&build, std::fs::Permissions::from_mode(0o444)).unwrap();
    let run = scratch.run(&["gen", "//lib/a"]);
    std::fs::set_permissions(&build, std::fs::Permissions::from_mode(0o644)).unwrap();
    run.exits(2).says("cannot write ").says("BUILD.buri");
    assert!(!scratch.read("lib/a/BUILD.buri").contains("extra.buri"));
}

/// Every remembered answer of `gen --check` under `scratch`, as (path, text).
fn gen_records(scratch: &Scratch) -> Vec<(std::path::PathBuf, String)> {
    let mut found = Vec::new();
    for dir in std::fs::read_dir(scratch.path(".buri/cache"))
        .unwrap()
        .flatten()
    {
        let Ok(entries) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Ok(text) = std::fs::read_to_string(entry.path()) {
                if text.starts_with("buri-gen-answer-1\n") {
                    found.push((entry.path(), text));
                }
            }
        }
    }
    found
}

/// An answer `gen --check` remembered that no longer reads as one is asked
/// again: from a release that wrote it differently, or about a package or a
/// kind of rule that has since gone.
#[test]
fn a_damaged_remembered_gen_answer_is_asked_again() {
    let scratch = Scratch::copy_of(
        "command-line-gen-records",
        &tests_dir().join("repositories/cli/gen_check_remembers/repo"),
    );
    scratch.run(&["gen", "--check"]).ok();
    let with_generators: Vec<_> = gen_records(&scratch)
        .into_iter()
        .filter(|(_, text)| text.lines().nth(2).is_some_and(|n| n != "0"))
        .collect();
    assert!(
        !with_generators.is_empty(),
        "no remembered answer names a generator"
    );

    let damage = |edit: &dyn Fn(&str) -> String| {
        for (path, text) in gen_records(&scratch) {
            std::fs::write(path, edit(&text)).unwrap();
        }
        scratch.run(&["gen", "--check"]).ok();
    };
    // A package that is gone.
    let vanished = |text: &str| {
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        if lines.get(2).is_some_and(|n| n != "0") {
            if let Some(line) = lines.get_mut(3) {
                *line = format!(
                    "gone/package{}",
                    line.find('\t').map_or("", |at| &line[at..])
                );
            }
        }
        lines.join("\n") + "\n"
    };
    damage(&vanished);
    // A kind of rule that is gone.
    let unkinded = |text: &str| {
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        if lines.get(2).is_some_and(|n| n != "0") {
            if let Some(line) = lines.get_mut(3) {
                let mut fields: Vec<&str> = line.split('\t').collect();
                fields[1] = "gone-kind";
                *line = fields.join("\t");
            }
        }
        lines.join("\n") + "\n"
    };
    damage(&unkinded);
    // A format this release doesn't read.
    damage(&|text| text.replacen("buri-gen-answer-1", "buri-gen-answer-0", 1));
}

const RUNS: &str = r#"A program a page runs, and what it prints.

```buri run
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    io.println(ctx, "a latte").mapErr(fn(e) => "no")
}
```

```stdout
a latte
```
"#;

/// An example whose program fails when run is reported with how it ended.
#[test]
fn a_documented_program_that_fails_says_how_it_exited() {
    let scratch = Scratch::repo("command-line-doc-exits");
    scratch.write(
        "page.md",
        "Out of milk.\n\n```buri run\nfrom \"native\" import { NativeHost };\n\nexport fn main(_host: NativeHost): Result<(), Str> {\n    .Err(\"out of milk\")\n}\n```\n\n```stdout\nnothing\n```\n",
    );
    scratch
        .run(&["docs", "test", "page.md"])
        .exits(1)
        .says("this example does not run")
        .says("the example exited 1");
}

/// A temporary directory that can't be made is why an example didn't run.
#[test]
fn a_documented_program_needs_a_temporary_directory() {
    let scratch = Scratch::repo("command-line-doc-tmpdir");
    scratch.write("page.md", RUNS);
    scratch.run(&["docs", "test", "page.md"]).ok();
    scratch
        .run_with_env(
            &["docs", "test", "page.md"],
            &[("TMPDIR", "/dev/null/nowhere")],
        )
        .exits(1)
        .says("this example does not run")
        .says("cannot create /dev/null/nowhere/");
}
