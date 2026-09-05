//! **The build this toolchain was built by is the one it says it was.**
//!
//! This file is two halves. The first reads `.github/workflows/ci.yml` and holds
//! it to the handful of promises the suite makes about it. The second is the set
//! of assertions that used to be shell scripts under `.github/scripts/` — the
//! liveness gates — rewritten as tests, so that they run on a contributor's
//! machine as well as on a runner and so that a workflow step is one `cargo`
//! invocation rather than a call into half a thousand lines of bash.
//!
//! ## The workflow's half
//!
//! * **`BURI_CI=1`.** `cli/tests/harness/ci.rs` turns every
//!   `if !supported() { return; }` in the native suite into a panic when it is
//!   set. Unset, they go back to returning quietly — which is right on a
//!   laptop and is exactly the vacuous green a runner must not report. Delete
//!   the `env:` line and every native test on a runner with a broken toolchain
//!   passes again, and nothing anywhere says so.
//! * **The deferrals.** Two tests in `build/repositories.rs` assert
//!   milliseconds, are meaningless in a debug profile, and are run by the
//!   `language-server-budget` job instead of by the `test` matrix.
//!   `ci::deferred_to` names that job in a string. A string naming a job that
//!   has been renamed is a skip wearing a deferral's clothes.
//! * **No step runs a script from this repository, and no invocation asks for
//!   part of the suite.** Both are the same rule from two sides: what CI runs
//!   has to be a thing a contributor can run, and what it runs has to be all of
//!   it.
//! * **The ignore list.** `.github/known-skips.txt` is every `#[ignore]` in the
//!   tree with the defect that removes it. This file holds the tree to that
//!   list exactly — a new `#[ignore]` fails here, on every machine, before it
//!   ever reaches a runner, and a row whose test is gone fails here too.
//!
//! ## The liveness gates
//!
//! `cli/build.rs` DEGRADES rather than breaks. A host whose `cc` cannot compile
//! the generated C gets an empty stencil library; a host with no musl `rust-std`
//! gets a runtime archive built against the wrong libc; a dependency tree that
//! will not resolve gets an empty archive. In every one of those states the
//! suite still runs, every guarded test returns early, and `cargo test` exits
//! zero having proved nothing. Bytes on disk are the only thing that can tell
//! that apart from a real run, and the tests at the bottom of this file are what
//! read them — under `BURI_CI`, because on a laptop an empty library is a fair
//! answer and on a runner it is a broken machine.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects,
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "test code. The lint set in `Cargo.toml` pins a promise about the \
              toolchain — that no input panics it — and a harness that reads \
              the workflow is not the toolchain."
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn workflow() -> String {
    let path = repo_root().join(".github/workflows/ci.yml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} cannot be read: {e}", path.display()))
}

/// Every `.rs` file in the repository, minus the build outputs.
///
/// "The repository" is git's answer — tracked files, plus untracked ones git
/// would not ignore — because the working tree holds whole *copies* of the
/// tree that are not it: agent worktrees under `.claude/`, the Nix input
/// cache under `.direnv/`, the grammar checkout Zed writes under
/// `editors/zed/grammars/`. Each is ignored by git, each holds `.rs` files,
/// and a walk that read them reported their `#[ignore]`s as this tree's.
/// Where git cannot answer, a plain walk stands in, skipping the build
/// outputs and everything hidden.
fn rust_sources() -> Vec<PathBuf> {
    let root = repo_root();
    let mut out = listed_by_git(&root).unwrap_or_else(|| walked(&root));
    out.sort();
    out
}

fn listed_by_git(root: &Path) -> Option<Vec<PathBuf>> {
    let output = std::process::Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard", "-z", "--", "*.rs"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let listed = String::from_utf8(output.stdout).ok()?;
    // `--cached` still lists a file deleted from disk but not yet from the
    // index; what is read below has to exist.
    Some(listed.split('\0').filter(|p| !p.is_empty()).map(|p| root.join(p)).filter(|p| p.is_file()).collect())
}

fn walked(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if name == "target" || name == "node_modules" || name.starts_with('.') {
                    continue;
                }
                walk(&path, out);
            } else if name.ends_with(".rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

/// The part of a line that is code, with a `//` comment cut off.
///
/// Crude on purpose — a `//` inside a string literal would take the rest of the
/// line with it — and correct for what it is used for, which is deciding
/// whether an `ignore` on a line is an attribute or prose about one. This file
/// itself is full of the latter.
fn code(line: &str) -> &str {
    match line.find("//") {
        Some(at) => &line[..at],
        None => line,
    }
}

/// The double-quoted string literals on a line, in order.
fn strings(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut s = String::new();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    chars.next();
                }
                '"' => break,
                _ => s.push(c),
            }
        }
        out.push(s);
    }
    out
}

// ---------------------------------------------------------------------------
// The ignore list
// ---------------------------------------------------------------------------

/// `<path>::<test>` for every row of `.github/known-skips.txt`, with its reason.
fn signed_for() -> BTreeMap<String, String> {
    let path = repo_root().join(".github/known-skips.txt");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} cannot be read: {e}", path.display()));
    let mut out = BTreeMap::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') || line.starts_with(char::is_whitespace) {
            continue;
        }
        let (name, why) = line.split_once(" — ").unwrap_or_else(|| {
            panic!("a row of known-skips.txt has no ` — <why>`, and a skip with no reason is a skip nobody will remove:\n  {line}")
        });
        assert!(
            why.trim().len() > 40,
            "the reason for {name} is a phrase, and a row in that file has to say what fixing it means:\n  {why}"
        );
        out.insert(name.trim().to_string(), why.trim().to_string());
    }
    out
}

/// The attribute this file is looking for, spelled in two halves.
///
/// Not a coincidence and not cleverness: this file is itself a `.rs` file under
/// the root it walks, so a literal of the whole attribute in here would be
/// found by the scan below and reported as an ignored test in a file that has
/// none. Assembling it at run time is the difference between a scanner and a
/// scanner that finds itself.
const IGNORE: &str = "ignore";

/// Whether an attribute's text names the `ignore` attribute, as a word.
///
/// `#[cfg_attr(not(unix), ignore = "…")]` is the same skip with a condition on
/// it, and `#[allow(clippy::…)]` on a line that happens to contain the letters
/// is not — so the match is on a word rather than on a substring.
fn names_ignore(attribute: &str) -> bool {
    let bytes = attribute.as_bytes();
    let mut from = 0;
    while let Some(at) = attribute[from..].find(IGNORE) {
        let start = from + at;
        let end = start + IGNORE.len();
        let before = start.checked_sub(1).map(|i| bytes[i]);
        let after = bytes.get(end).copied();
        let boundary = |c: Option<u8>| !c.is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_');
        if boundary(before) && boundary(after) {
            return true;
        }
        from = end;
    }
    false
}

/// Every `ignore` attribute in the tree, as `<repo-relative path>::<test name>`.
///
/// An attribute is read whole, across however many lines it spans, and matched
/// as a word — so `#[cfg_attr(…, ignore = "…")]` counts and a `let gitignore =`
/// does not. A conditional ignore is a skip on the hosts its condition selects,
/// and this repository's answer to "this host cannot answer it" is `#[cfg]` —
/// the test is absent rather than reported as not run — so both spellings
/// belong in the list this is compared against.
fn ignored_in_tree() -> BTreeMap<String, PathBuf> {
    let root = repo_root();
    let mut out = BTreeMap::new();
    for path in rust_sources() {
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let mut i = 0;
        while i < lines.len() {
            let start = i;
            let first = code(lines[i]);
            if !first.trim_start().starts_with("#[") {
                i += 1;
                continue;
            }
            // Read the attribute to its closing bracket: the strings inside it
            // are prose and can hold anything, so the depth is counted over
            // brackets outside quotes.
            let mut attribute = String::new();
            let mut depth = 0i32;
            // `quoted` and `escaped` are carried ACROSS lines, because a Rust
            // string literal is: `#[ignore = "…\` runs on to the next line, and
            // a per-line reset reads the closing quote of such a string as an
            // opening one — which leaves the depth counter waiting for a
            // bracket that has already gone by. Found by this test failing on
            // the two multi-line attributes it was written to find.
            let mut quoted = false;
            let mut escaped = false;
            loop {
                let line = code(lines[i]);
                for c in line.chars() {
                    match c {
                        _ if escaped => escaped = false,
                        '\\' if quoted => escaped = true,
                        '"' => quoted = !quoted,
                        '[' if !quoted => depth += 1,
                        ']' if !quoted => depth -= 1,
                        _ => {}
                    }
                }
                attribute.push_str(line);
                attribute.push(' ');
                i += 1;
                if depth <= 0 || i >= lines.len() {
                    break;
                }
            }
            if !names_ignore(&attribute) {
                continue;
            }
            let name = lines[i.saturating_sub(1)..]
                .iter()
                .find_map(|l| {
                    let l = code(l).trim_start();
                    let rest = l.strip_prefix("fn ")?;
                    Some(rest.split('(').next().unwrap_or(rest).trim().to_string())
                })
                .unwrap_or_else(|| {
                    panic!(
                        "{}:{}: an `{IGNORE}` attribute with no `fn` under it",
                        path.display(),
                        start + 1
                    )
                });
            let rel =
                path.strip_prefix(&root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            out.insert(format!("{rel}::{name}"), path.clone());
        }
    }
    out
}

/// The tree's `#[ignore]`s are exactly the rows of `.github/known-skips.txt`.
///
/// Both directions, and the second one matters as much as the first: a row for
/// a test that has been fixed is a file that has stopped being the list it
/// claims to be, and the next person to read it learns to skim it.
#[test]
fn the_only_ignored_tests_are_the_ones_named_here() {
    let signed = signed_for();
    let found = ignored_in_tree();

    let unsigned: Vec<&String> = found.keys().filter(|k| !signed.contains_key(*k)).collect();
    assert!(
        unsigned.is_empty(),
        "these tests are `#[ignore]`d and .github/known-skips.txt does not name them:\n  {}\n\n\
         A skipped test costs what an enabled one costs to compile and proves nothing. The \
         dispositions are: fix it; `#[cfg]` it out on the host that genuinely cannot answer it, so \
         it is absent rather than reported as not run; or delete it, if the behaviour it asserts \
         is no longer wanted. If it is red because the compiler is missing a behaviour it is \
         supposed to have, and that is a slice of its own, add a row to that file naming the slice.",
        unsigned.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n  ")
    );

    let stale: Vec<&String> = signed.keys().filter(|k| !found.contains_key(*k)).collect();
    assert!(
        stale.is_empty(),
        "these rows of .github/known-skips.txt name a test that is no longer ignored (or no \
         longer there):\n  {}\n\nDelete the row — the defect it names is fixed.",
        stale.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n  ")
    );

    for (name, why) in &signed {
        println!("signed-for skip: {name}\n  {why}");
    }
}

// ---------------------------------------------------------------------------
// The workflow
// ---------------------------------------------------------------------------

/// The workflow's own `env:` block — the one before `jobs:`, which GitHub hands
/// to every job and every step in the file.
fn workflow_env(text: &str) -> String {
    let before = text.split("\njobs:").next().unwrap_or(text);
    let Some(at) = before.rfind("\nenv:\n") else {
        panic!("ci.yml has no workflow-level `env:` block");
    };
    before[at..].to_string()
}

/// `BURI_CI=1` reaches every job, because it is set once above all of them.
///
/// A workflow-level `env:` is not a shortcut here, it is the correctness
/// argument: a per-job block is a line somebody adding the ninth job forgets,
/// and the ninth job is then the one where a broken toolchain passes. A
/// job-level `env:` overrides only the keys it names, so the jobs that set
/// `CC` or `BURI_PERF` keep this one.
#[test]
fn the_workflow_sets_buri_ci_for_every_job() {
    let text = workflow();
    let env = workflow_env(&text);
    assert!(
        env.contains("BURI_CI: 1"),
        "the workflow-level `env:` block does not set `BURI_CI: 1`. Without it every \
         `if !supported() {{ return; }}` in the native suite goes back to returning quietly on a \
         runner, and a job whose toolchain is broken reports every native test as passed. \
         `cli/tests/harness/ci.rs` is the other half.\n--- the block ---\n{env}"
    );
}

/// The jobs of `ci.yml`, as `name -> the whole of its text`.
///
/// Split on indentation rather than parsed: a job is a key at two spaces under
/// `jobs:`, and nothing else in this file is. A YAML parser would be a
/// dependency, and the bar in `Cargo.toml` is what it is.
fn jobs(text: &str) -> BTreeMap<String, String> {
    let Some(at) = text.find("\njobs:\n") else { panic!("ci.yml has no `jobs:`") };
    let body = &text[at + "\njobs:\n".len()..];
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in body.lines() {
        let is_key = line.starts_with("  ")
            && !line.starts_with("   ")
            && line.trim_end().ends_with(':')
            && !line.trim_start().starts_with('#');
        if is_key {
            current = Some(line.trim().trim_end_matches(':').to_string());
        }
        if let Some(name) = &current {
            out.entry(name.clone()).or_default().push_str(line);
            out.get_mut(name).unwrap().push('\n');
        }
    }
    out
}

/// **No `run:` step invokes a script file from this repository.**
///
/// This workflow used to call eight scripts under `.github/scripts/`, one of
/// them twenty-eight kilobytes of bash, and that was the shape of the problem
/// rather than an implementation detail of it: a check written in shell runs on
/// a runner and nowhere else, so the thing a contributor can reproduce and the
/// thing that decides whether a commit is green drift apart. Every one of those
/// checks was reading bytes a Rust test could read directly, and they are the
/// tests at the bottom of this file now.
///
/// The one exception is named rather than pattern-matched. `check.sh` in the two
/// `editors/tree-sitter-*` directories is the grammar's own test runner — a
/// contributor runs it by hand, it lives beside the grammar it checks, and it is
/// not CI machinery — so the `tree-sitter` job calls it exactly as a person
/// would.
#[test]
fn no_step_runs_a_script_from_the_repository() {
    let text = workflow();
    let mut offenders: Vec<String> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        // `.sh` as the end of a file name: `.shared` is not a script, and a
        // plain substring test would say it was.
        let names_a_script = line.match_indices(".sh").any(|(at, _)| {
            let after = line[at + ".sh".len()..].chars().next();
            !after.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        });
        if !names_a_script || line.contains("./check.sh") {
            continue;
        }
        offenders.push(format!("{}: {line}", i + 1));
    }
    assert!(
        offenders.is_empty(),
        "{} step(s) in ci.yml call a script from this repository:\n  {}\n\nA check written in \
         shell runs on a runner and nowhere else. Every one of the eight that used to be here was \
         reading bytes — a stencil library's size, an archive's symbol table, an ELF header — and \
         the tests at the bottom of this file read the same bytes on every machine. Write it \
         there, and let the step be `cargo test`.",
        offenders.len(),
        offenders.join("\n  ")
    );
    let scripts = repo_root().join(".github").join("scripts");
    assert!(
        !scripts.exists(),
        "{} is back. The assertions that lived there are tests in this file; a second copy in \
         bash is a second answer to the same question, and only one of the two runs on a laptop.",
        scripts.display()
    );
}

/// **No invocation in the workflow asks for part of the suite.**
///
/// The shell gate this replaces summed the `filtered out` column off every
/// `test result:` line, because a stray name filter or a `--skip` on a step's
/// command line removes tests from a run that still exits zero. The same fact is
/// legible in the workflow itself: a `--skip` is a deliberate subtraction, and
/// there is no longer a job that needs one.
///
/// The count at the bottom is the guard on the guard. This works by recognising
/// the invocations that ask for everything, and a shape it stops recognising is
/// a job it silently stops checking — which is the failure this whole file
/// exists to refuse.
#[test]
fn nothing_in_the_workflow_asks_for_part_of_the_suite() {
    let text = workflow();
    let skipping: Vec<&str> =
        text.lines().map(str::trim).filter(|line| line.contains("--skip ")).collect();
    assert!(
        skipping.is_empty(),
        "these lines in ci.yml pass `--skip`:\n  {}\nA `--skip` removes tests from a run that \
         still exits zero. If a test must not run in a job, it is a deferral — `ci::deferred_to`, \
         which names the job that does run it and is held to it below — and not a flag on a \
         command line.",
        skipping.join("\n  ")
    );
    let whole = text
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("cargo test -p buri") && !line.contains("--test "))
        .count();
    assert!(
        whole >= 2,
        "only {whole} invocation(s) in ci.yml run `cargo test -p buri` unfiltered, and two do — \
         the `test` matrix and the `release` job. Either the workflow stopped running the suite, \
         or the command has changed shape and this test has stopped seeing it."
    );
    println!("{whole} unfiltered `cargo test -p buri` invocation(s), and no `--skip` anywhere");
}


/// Every job declares `timeout-minutes`.
///
/// It is the outer bound on everything: a step that deadlocks, a runner that
/// loses its network, a test the per-invocation cap in `harness/hang.rs` does
/// not cover. Without it GitHub's own six-hour default applies, and a wedged
/// job holds a runner for a working day before anybody is told.
///
/// Nine of nine carry one today and nothing said so — the tenth job, added
/// tomorrow, would have been the one without. Asserted at four spaces of
/// indentation, which is where a job's own keys live: a `timeout-minutes` on a
/// *step* is a different promise and would not bound the job around it.
#[test]
fn every_job_declares_a_timeout() {
    let text = workflow();
    let mut missing: Vec<String> = Vec::new();
    let mut found = 0;
    for (name, body) in jobs(&text) {
        let declared = body.lines().find(|line| {
            line.starts_with("    timeout-minutes:") && !line.starts_with("     ")
        });
        let Some(line) = declared else {
            missing.push(format!("{name}: no `timeout-minutes:`"));
            continue;
        };
        let value = line.split(':').nth(1).unwrap_or("").trim();
        match value.parse::<u32>() {
            Ok(minutes) if minutes > 0 => found += 1,
            _ => missing
                .push(format!("{name}: `timeout-minutes: {value}` is not a number of minutes")),
        }
    }
    assert!(
        missing.is_empty(),
        "{} job(s) in ci.yml are bounded by nothing but GitHub's six-hour default. A job that \
         wedges then holds a runner for a working day and reports nothing anyone can act on. Add \
         `timeout-minutes:` beside `runs-on:`.\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
    assert!(
        found >= 9,
        "only {found} job(s) were found to check, and ci.yml had nine when this was written — \
         the splitter above has stopped seeing jobs and this test is asserting nothing"
    );
    println!("{found} job(s), each with a `timeout-minutes` of its own");
}

/// No runner configuration in the tree promises a cap nothing enforces.
///
/// `.config/nextest.toml` set `slow-timeout = { period = "60s", terminate-after
/// = 5 }` for two slices and could never fire: every invocation in `ci.yml` is
/// `cargo test`, which does not read that file. The file was deleted; the cap it
/// described lives in `cli/tests/harness/hang.rs` and is proved to fire by a
/// test beside it. This is what stops the config coming back as a promise again.
///
/// Assembled rather than written, because this file is one of the files the
/// other tests here walk.
#[test]
fn no_runner_config_promises_a_cap_nothing_reads() {
    let runner = format!("next{}", "est");
    let config = repo_root().join(".config").join(format!("{runner}.toml"));
    assert!(
        !config.exists(),
        "{} is back. Nothing in this repository runs that runner — every invocation in ci.yml is \
         `cargo test -p buri`, which never reads it — so a timeout written there is a sentence \
         and not a cap. The real one is `cli/tests/harness/hang.rs`, which caps one CLI \
         invocation and names the test it killed; the outer bound is each job's \
         `timeout-minutes`, asserted above.",
        config.display()
    );
    let workflow = workflow();
    assert!(
        !workflow.contains(&runner),
        "ci.yml invokes `{runner}`. Nothing else in this repository does, so the timeout that \
         config describes would be a sentence rather than a cap — and a second test runner is a \
         second set of rules about what counts as a skip, on top of the ones this file holds."
    );
}

/// The packages of the workspace, as `directory -> package name`.
///
/// Two files rather than one: the root manifest says which directories are
/// members, and each member's own manifest says what its package is called.
/// The distinction is the whole point of this — `cli/`'s package is `buri`, and
/// a reader who assumed the directory was the name would look for a
/// `-p cli` that never existed.
fn workspace_members() -> BTreeMap<String, String> {
    let root = repo_root();
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let after = manifest.split("members = [").nth(1).unwrap_or_else(|| {
        panic!("the root Cargo.toml has no `members = [` — the workspace has been restructured")
    });
    let list = after.split(']').next().unwrap_or("");
    let mut out = BTreeMap::new();
    for dir in list.split(',') {
        let dir = dir.trim().trim_matches('"');
        if dir.is_empty() {
            continue;
        }
        let path = root.join(dir).join("Cargo.toml");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} cannot be read: {e}", path.display()));
        // `name` under `[package]`, and not the one under `[lib]` — which in
        // `website/Cargo.toml` says the same thing and in another member might
        // not.
        let mut section = "";
        let mut name = None;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                section = line;
            } else if section == "[package]" && line.starts_with("name") {
                name = strings(line).into_iter().next();
                break;
            }
        }
        let name = name.unwrap_or_else(|| panic!("{} names no package", path.display()));
        out.insert(dir.to_string(), name);
    }
    out
}

/// Every member of the workspace is tested by some job.
///
/// `website` was not, for as long as it had existed: `members = ["cli",
/// "website"]`, every `cargo test` in the workflow was `-p buri`, and its
/// forty-one tests ran nowhere. The mechanisms that exist to catch a test that
/// does not run were blind to it — they carry no `#[ignore]`, so the scan above
/// sees nothing, and no runner assumption of theirs was broken, so nothing
/// panicked. Those ask "was a test skipped"; this asks the question underneath
/// it, which is whether the suite was ever asked for at all.
#[test]
fn every_workspace_member_is_tested_by_a_job() {
    let text = workflow();
    let members = workspace_members();
    assert!(
        members.len() >= 2,
        "only {} workspace member(s) were parsed out of the root manifest, and there have been \
         two since `website` was added — this test is asserting nothing",
        members.len()
    );
    let mut untested = Vec::new();
    for (dir, package) in &members {
        // The whole word, not the prefix: `-p website` must not be satisfied by
        // a `-p websites`, and a package whose name is another's prefix is the
        // day a substring test would report a member as covered by the job that
        // covers its neighbour.
        let asked = format!("cargo test -p {package}");
        let covered = text.match_indices(&asked).any(|(at, _)| {
            let after = text[at + asked.len()..].chars().next();
            !after.is_some_and(|c| c.is_alphanumeric() || c == '-' || c == '_')
        });
        if !covered {
            untested.push(format!("{dir}/ (package `{package}`)"));
        }
    }
    assert!(
        untested.is_empty(),
        "{} workspace member(s) are in `members` and in no `cargo test` in ci.yml, so their tests \
         run on no job:\n  {}\nA package nothing asks for is a package where every test is \
         skipped, and the `#[ignore]` scan above cannot see it — there is nothing for it to find.",
        untested.len(),
        untested.join("\n  ")
    );
    println!("{} workspace member(s), each asked for by a job", members.len());
}

/// Every `ci::deferred_to` names a job that is in the workflow and still asks
/// for the tests that defer to it.
///
/// A deferral is the one skip this repository allows on a runner, and it is
/// allowed *because* the assertion is made elsewhere in the same workflow. The
/// day the job is renamed or deleted, the deferral is a plain skip and the only
/// thing that would have noticed is this.
#[test]
fn every_deferral_names_a_job_that_still_asks_for_it() {
    let text = workflow();
    let names: Vec<&str> = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("name: "))
        .map(str::trim)
        .collect();

    // Assembled rather than written, for the reason `IGNORE` is: this file is
    // one of the files walked below.
    let needle = format!("ci::deferred{}(", "_to");

    let mut found = 0;
    for path in rust_sources() {
        let source = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = source.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if !code(line).contains(&needle) {
                continue;
            }
            // The first two string literals from the call onwards: the domain,
            // then the job.
            let mut literals = Vec::new();
            for l in &lines[i..] {
                literals.extend(strings(code(l)));
                if literals.len() >= 2 {
                    break;
                }
            }
            assert!(
                literals.len() >= 2,
                "{}:{}: a `ci::deferred_to` with no job name in it",
                path.display(),
                i + 1
            );
            let job = &literals[1];
            assert!(
                names.iter().any(|n| n == job),
                "{}:{} defers to a job called `{job}` and ci.yml has no job with that name. A \
                 deferral whose job is gone is a test that runs nowhere. The names in the \
                 workflow are:\n  {}",
                path.display(),
                i + 1,
                names.join("\n  ")
            );
            found += 1;
        }
    }
    assert!(found > 0, "no `ci::deferred_to` was found; this test would have asserted nothing");
    println!("{found} deferral(s), each naming a job that is in the workflow");
}

// ---------------------------------------------------------------------------
// The liveness gates
//
// Everything below used to be a shell script under `.github/scripts/`, run as a
// workflow step against the files `cli/build.rs` had just written. They read the
// same bytes here — through the constants the toolchain embeds them in, which is
// the same emptiness `stencil::AVAILABLE` and `runtime_native::AVAILABLE` are
// computed from — so the question "did this toolchain build for real" is asked
// by the suite rather than beside it.
//
// Each opens with `ci::on()`. That is not a skip in the sense this repository
// refuses: a contributor with no clang has an empty stencil library and that is
// the correct state of their machine, while a runner with one is a machine whose
// setup broke. The guard is which of the two hosts is being asked.
// ---------------------------------------------------------------------------

#[path = "harness/ci.rs"]
mod ci;

/// This assertion is about a runner, and this host is not one.
fn only_on_a_runner(what: &str) -> bool {
    if ci::on() {
        return false;
    }
    eprintln!("ci: `{what}` is asserted on a runner only (BURI_CI=1 is unset here)");
    true
}

/// Every tool the workflow installs is on `PATH`.
///
/// Asserted rather than assumed, and asserted *from inside the suite* so that it
/// is the tests' own claim. A missing tool otherwise presents as a test that
/// returned early or as a link failure inside a suite three steps later, and
/// both read as "green" or as "a compiler bug" rather than as "the runner
/// changed". `cli/tests/native/stencil.rs::cross_tools` is the reason the llvm
/// three are in the list: it probes them and its callers return quietly when
/// they are absent, so the tools being present is what makes those tests run.
#[test]
fn every_tool_the_workflow_assumes_is_on_path() {
    if only_on_a_runner("every_tool_the_workflow_assumes_is_on_path") {
        return;
    }
    let mut needed = vec!["cc", "clang", "node", "bun"];
    if cfg!(target_os = "linux") {
        needed.extend(["ld.lld", "mold", "llvm-nm", "llvm-objdump", "llvm-readelf", "readelf"]);
    }
    let missing: Vec<&str> = needed
        .into_iter()
        .filter(|tool| {
            std::process::Command::new(tool)
                .arg("--version")
                .output()
                .is_ok_and(|out| out.status.success())
                .eq(&false)
        })
        .collect();
    assert!(
        missing.is_empty(),
        "these tools are not on PATH: {}\n\nEvery job that runs this suite installs them (the \
         `test` matrix's apt step, and `oven-sh/setup-bun`). One that is missing does not fail \
         anything by itself — it makes a suite return early and report a pass.",
        missing.join(", ")
    );
}

/// **The stencil libraries this toolchain was built with are not empty.**
///
/// The failure this catches is SILENT. `cli/build.rs` degrades rather than
/// breaks: a host whose `cc` cannot compile the generated C gets an empty
/// library, `stencil::available_for` reads the emptiness, and every test in
/// `cli/tests/native/stencil.rs` opens with `if !supported() { return; }`. Such
/// a runner runs the whole suite, reports every test as passed, and proves
/// nothing. `cargo test`'s exit status cannot tell that apart from a real green
/// run; this can, because availability is literally `!blob(t).is_empty()`.
///
/// The *reverse* degrade is asserted too. A Linux host cannot build the
/// `macos-arm64` library — `cli/build.rs` gates that target on an
/// `aarch64-apple-darwin` TARGET — so on Linux that blob must be empty. An
/// assertion that only ever said "present" would not notice a build script that
/// started writing every target unconditionally.
#[test]
fn the_stencil_libraries_are_real() {
    use buri::compiler::backend::stencil::{self, abi::StencilTarget};

    if only_on_a_runner("the_stencil_libraries_are_real") {
        return;
    }
    // Both Linux libraries are cross-compilable from any clang, so both are
    // required on every host — that is what makes `linux-arm64` a container port
    // rather than a second backend.
    for target in [StencilTarget::LinuxArm64, StencilTarget::LinuxX86_64] {
        assert!(
            stencil::available_for(target),
            "the `{target:?}` stencil library is empty, so every test guarded on availability \
             returns early and the suite passes vacuously. `cli/build.rs` writes an empty library \
             when its `cc` cannot produce that target's objects: check that `CC` is clang and \
             that this clang can cross-compile."
        );
    }
    let macos = stencil::available_for(StencilTarget::MacosArm64);
    if cfg!(target_os = "macos") {
        assert!(
            macos,
            "the `macos-arm64` stencil library is empty on a macOS host, which is the host that \
             builds it. Nothing native can run here."
        );
    } else {
        assert!(
            !macos,
            "the `macos-arm64` stencil library is NOT empty on a host that is not macOS. \
             `cli/build.rs` gates that target on an `aarch64-apple-darwin` TARGET, so either the \
             gate moved or these are not the bytes they claim to be."
        );
    }
    // Through a binding, because `AVAILABLE` is a `const` and an `assert!` on
    // one is a lint about a check the compiler could have made — which this is
    // not: the constant is computed from bytes `cli/build.rs` wrote, so its
    // value is a fact about the build and not about the source.
    let runnable: bool = stencil::AVAILABLE;
    assert!(
        runnable,
        "this host's own stencil library is empty (or, on x86-64, `asm.rs` has no entry point for \
         it), so nothing in the native suite can build a runnable program."
    );
}

/// The symbols in the runtime archive, as `nm` reports them.
///
/// The archive is bytes in this binary rather than a file, so it is written back
/// out for `nm` to read. `nm` and not a substring search over the archive: a
/// search would match a path or a piece of prose that happens to carry a crate's
/// name, and the assertion below turns on a crate being *absent*, which is
/// exactly where a false positive would be a red run about nothing.
fn archive_symbols() -> String {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ci-archive");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(buri::compiler::backend::runtime_native::ARCHIVE_NAME);
    std::fs::write(&path, buri::compiler::backend::runtime_native::ARCHIVE).unwrap();
    for tool in ["nm", "llvm-nm"] {
        let Ok(out) = std::process::Command::new(tool).arg(&path).output() else { continue };
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        if !text.trim().is_empty() {
            return text;
        }
    }
    panic!(
        "neither `nm` nor `llvm-nm` produced a symbol table for {}, so the assertions about what \
         the runtime archive carries would pass having read nothing.",
        path.display()
    );
}

/// **The runtime archive is real, is the right size, and carries what it says.**
///
/// The other blob `cli/build.rs` writes, and it degrades the same way the
/// stencil libraries do — an empty archive on a host with no runtime, and, since
/// the `net` feature, on a dependency tree that cannot be resolved. An empty one
/// makes every native test return early and the suite pass vacuously.
///
/// Five claims beyond emptiness, each of which has been wrong once:
///
/// * **The libc.** On Linux the archive is built for `<arch>-unknown-linux-musl`
///   so that every artifact `buri build` links is a static-PIE musl executable
///   that runs on any Linux (ARCHITECTURE.md §9). When the musl `rust-std` is
///   missing `cli/build.rs` falls back to the host's gnu triple, and the
///   resulting compiler produces artifacts that run on the build machine and on
///   nothing older — a break that fails for users and for nobody in CI.
/// * **The size.** Every `buri` binary carries these bytes, and the `net-h3`
///   archive is held to the same number for the reason the budget states.
/// * **The networking crates.** `net` is what links the reactor and the TLS
///   client; an archive that carries neither has an `https://` that fails at run
///   time and a suite that never noticed. `aws_lc` is the tripwire and is
///   forbidden on every leg and every feature set: it was never a dependency, so
///   a second cryptography implementation appearing in every binary means
///   `quinn`'s own defaults — `rustls-aws-lc-rs` and `platform-verifier` —
///   turned themselves back on past the `rustls-ring` `manifest.toml` asks for.
/// * **`quinn`, whose side of the line the FEATURE and the PLATFORM decide
///   together**, and which is the one claim here that has been re-measured
///   rather than merely restated. It is what `net-h3` brings in, so with the
///   feature off it must be absent on every host — the crate is not in the tree
///   at all. With the feature on, what happens next is the linker's:
///
///   * On **macOS**, fat LTO drops it whole. Nothing but `net.rs`'s `size_of`
///     names it, so the h3 archive carries no `quinn` symbol and is 9 147 960
///     bytes against the `net` one's 9 146 416 — a QUIC implementation for
///     1 544 bytes.
///   * On **Linux**, it does not. The ELF archive keeps quinn's symbol names,
///     which is what turned the first h3 CI leg red against an assertion ported
///     from a shell script that had only ever been true on Darwin.
///
///   Both directions are asserted on both platforms, because either of them
///   changing is a fact worth a red X: the day the Linux archive stops carrying
///   quinn is the day fat LTO started dropping it there too, and the day macOS
///   starts carrying it is the day something first CALLED into it — which is
///   the slice the size budget below is waiting for.
/// * **The entropy door.** With `crypto` the archive exports
///   `buri_rt_host_entropy_bytes`; without it, the compile-time refusal fires
///   instead. A toolchain in the third state — a feature file saying `crypto`
///   and no symbol — refuses nothing and fails at the system linker.
#[test]
fn the_runtime_archive_is_real() {
    use buri::build::musl::Libc;
    use buri::compiler::backend::runtime_native as rt;

    if only_on_a_runner("the_runtime_archive_is_real") {
        return;
    }
    assert!(
        !rt::ARCHIVE.is_empty(),
        "libburi_rt.a is empty, so this toolchain has no native runtime: every native test \
         returns early and the suite passes vacuously. `cli/build.rs` writes an empty archive on \
         an unsupported host and on a dependency tree it cannot resolve, and the build log carries \
         a `cargo:warning` saying which."
    );

    // Measured, not guessed, and re-measured in the commit that moves either.
    //
    // **`net-h3` is held to the SAME number, and that is a measured claim rather
    // than an omission.** A separate, larger h3 budget would be a number with
    // nothing behind it, and it would hide exactly the growth this one catches:
    // on macOS the h3 archive is 9 147 960 bytes against the `net` one's
    // 9 146 416 — a QUIC implementation for 1 544 bytes, because nothing but
    // `net.rs`'s `size_of` names it — and on Linux, where quinn's symbols do
    // survive, the h3 archive is still inside the number below. The slice that
    // first CALLS into quinn is the one that comes back here and moves it.
    let budget = if cfg!(target_os = "macos") { 9_536_512 } else { 14_680_064 };
    assert!(
        rt::ARCHIVE.len() <= budget,
        "libburi_rt.a is {} bytes, over the {budget}-byte budget for this platform. Every buri \
         binary carries these bytes. Find out what grew — the usual answer is a dependency whose \
         code now reaches the archive, or a native library a crate compiled in — then fix it, or \
         re-measure and re-state the budget here.",
        rt::ARCHIVE.len()
    );

    if cfg!(target_os = "linux") {
        assert!(
            matches!(rt::libc(), Libc::MuslBaked | Libc::MuslSystem),
            "libburi_rt.a was built against {:?}, not musl. Executables this toolchain links will \
             depend on this machine's glibc and will not run on a Linux with an older one — which \
             fails for users and for nobody in CI. `cli/build.rs` falls back to the host triple \
             when the musl standard library is missing: the job that built this needs \
             `rustup target add $(uname -m)-unknown-linux-musl` before its first cargo build.",
            rt::libc()
        );
    } else {
        assert_eq!(
            rt::libc(),
            Libc::Absent,
            "libburi_rt.a names a Linux libc on a host where there is no Linux libc to name, so \
             the sidecar belongs to a different build than the archive beside it."
        );
    }

    assert!(
        !rt::h3() || rt::net(),
        "the archive declares `net-h3` without `net`, and the manifest makes net-h3 imply net. \
         Either `cli/build.rs` wrote a state cargo cannot produce, or the feature file belongs to \
         a different archive."
    );

    let symbols = archive_symbols().to_lowercase();
    let carries = |crate_name: &str| symbols.contains(crate_name);
    if rt::net() {
        for wanted in ["tokio", "rustls", "ring_core", "hyper", "tungstenite"] {
            assert!(
                carries(wanted),
                "libburi_rt.a carries no symbol from `{wanted}`, and it was built with the \
                 runtime's `net` feature, which is what links the reactor, the TLS client, HTTP/2 \
                 and WebSockets. An archive in this state has an `https://` or a suspending host \
                 call that fails at run time and a suite that never noticed."
            );
        }
    } else {
        for unwanted in ["tokio", "hyper", "rustls", "tungstenite", "ring_core"] {
            assert!(
                !carries(unwanted),
                "libburi_rt.a was built without `net` and carries symbols from `{unwanted}`."
            );
        }
    }

    // `quinn`, in whichever of the four states this archive is in. The doc
    // comment argues each; the table is here so that a reader of the failure
    // sees all four at once rather than the one that fired.
    let quinn_expected = rt::h3() && cfg!(target_os = "linux");
    assert_eq!(
        carries("quinn"),
        quinn_expected,
        "libburi_rt.a {} symbols from `quinn`, and on {} with `net-h3` {} it should {}.\n\
         \n\
         The four states, all measured:\n  \
           net-h3 off, any host  — absent: the crate is not in the tree at all.\n  \
           net-h3 on,  macOS     — absent: nothing but `net.rs`'s `size_of` names it, so fat LTO \
         drops it whole and the archive grows 1 544 bytes.\n  \
           net-h3 on,  Linux     — present: the ELF archive keeps its symbol names.\n\
         \n\
         An unexpected PRESENT with the feature off means a crate was added to \
         cli/runtime/manifest.toml without an argument for it. An unexpected present on macOS \
         means something first CALLED into quinn, which is the slice that also re-measures the \
         size budget above. An unexpected ABSENT on Linux means fat LTO started dropping it \
         there too, which is good news and a stale assertion: re-measure and move this line.",
        if carries("quinn") { "carries" } else { "carries no" },
        std::env::consts::OS,
        if rt::h3() { "on" } else { "off" },
        if quinn_expected { "carry them" } else { "carry none" },
    );

    // `aws_lc` is on the absent list on EVERY leg and every feature set, and it
    // was never a dependency: a second cryptography implementation appearing is
    // a feature that turned itself on somewhere and doubled the crypto in every
    // binary this compiler produces. `quinn` is precisely the crate that would
    // do it, which is why this is asserted hardest on the leg that has it.
    assert!(
        !carries("aws_lc"),
        "libburi_rt.a carries symbols from `aws_lc`, and nothing in the runtime is supposed to \
         reach it — it has never been a dependency. `quinn`'s own defaults are \
         `rustls-aws-lc-rs` and `platform-verifier`, and `cli/runtime/manifest.toml` turns both \
         off and asks for `rustls-ring`; this is what says so when those defaults next change. A \
         binary with two cryptography implementations in it is the thing being refused."
    );

    let entropy = carries("buri_rt_host_entropy_bytes");
    assert_eq!(
        entropy,
        rt::crypto(),
        "the archive's `crypto` feature says {} and its `buri_rt_host_entropy_bytes` door says \
         {entropy}. With the feature and no door, every program that calls `crypto.randomBytes` \
         fails at the system linker while the compile-time refusal that exists for exactly this \
         case stays quiet; with the door and no feature, the toolchain refuses `Entropy` while \
         carrying the body it refused.",
        rt::crypto()
    );
}

/// **The published `buri` crate carries the native runtime's sources.**
///
/// `cli/build.rs` compiles `cli/runtime/` into `libburi_rt.a` at toolchain-build
/// time, so a registry tarball without those files is a `cargo install buri`
/// that fails in the build script with nothing to compile. That is exactly what
/// happened once: the runtime became a cargo package, and `cargo package` skips
/// any subdirectory of a package that holds a `Cargo.toml` — unconditionally,
/// ahead of `include`/`exclude` — so the whole directory silently stopped
/// shipping. Nothing said so, because a checkout still builds.
///
/// The fix is that the runtime's manifest is `manifest.toml` and its lockfile is
/// `manifest.lock`, neither named the way cargo would name it. This is the
/// assertion that the fix holds.
///
/// `--list` rather than a real `cargo package`: the tarball is not wanted, only
/// its file list, and building it would compile the whole toolchain a second
/// time. `--allow-dirty` because this runs on a tree with uncommitted work in it
/// as often as not, and the claim is about the manifest's rules rather than
/// about the index.
#[test]
fn the_published_crate_ships_the_runtime() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| String::from("cargo"));
    let out = std::process::Command::new(cargo)
        .args(["package", "-p", "buri", "--list", "--allow-dirty"])
        .current_dir(repo_root())
        .output()
        .expect("cargo could not be run");
    assert!(
        out.status.success(),
        "`cargo package -p buri --list` failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let listed = String::from_utf8_lossy(&out.stdout);
    let files: Vec<&str> = listed.lines().map(str::trim).collect();

    // Named one by one rather than counted, because `cli/runtime/switch.rs`
    // reaches each `.s` file through `include_str!` at compile time: one missing
    // from the tarball is not a smaller archive, it is a `cargo install buri`
    // that fails in the build script with "couldn't read".
    for name in [
        "lib.rs",
        "manifest.toml",
        "manifest.lock",
        "switch_macos_arm64.s",
        "switch_linux_arm64.s",
        "switch_linux_x86_64.s",
    ] {
        let wanted = format!("runtime/{name}");
        assert!(
            files.contains(&wanted.as_str()),
            "{wanted} is not in the published buri crate. A `cargo install` from a registry \
             tarball would fail in cli/build.rs with no runtime to compile. The usual cause is a \
             `Cargo.toml` that has appeared under `cli/` — cargo package skips the directory that \
             holds one."
        );
    }

    // A floor rather than an equality, so that adding a runtime source is not a
    // CI failure, and not merely `> 0`, so that shipping one file out of
    // nineteen is not a pass. Raising it with each new source keeps it tight.
    let sources = files.iter().filter(|f| f.starts_with("runtime/") && f.ends_with(".rs")).count();
    assert!(
        sources >= 19,
        "the published buri crate carries {sources} runtime sources; cli/runtime holds more than \
         that, so cargo package is dropping some of them."
    );
    println!("the published buri crate carries {sources} runtime sources and both manifests");
}

// ---------------------------------------------------------------------------
// The linked Linux image
// ---------------------------------------------------------------------------

/// A little-endian ELF64 image, read for the four properties CI is about.
///
/// A reader rather than `readelf` and a pile of greps, which is what this used
/// to be: the properties below are a fixed offset into a header each, the file
/// is already bytes in hand, and a parse that is thirty lines of arithmetic
/// cannot mis-split a column the way an `awk '{print $(NF-1)}'` can.
#[cfg(target_os = "linux")]
struct Image {
    bytes: Vec<u8>,
}

#[cfg(target_os = "linux")]
impl Image {
    fn read(path: &Path) -> Self {
        let bytes = std::fs::read(path)
            .unwrap_or_else(|e| panic!("{} cannot be read: {e}", path.display()));
        assert!(
            bytes.len() > 64 && bytes[..4] == *b"\x7fELF" && bytes[4] == 2 && bytes[5] == 1,
            "{} is not a little-endian ELF64 image",
            path.display()
        );
        Self { bytes }
    }

    fn u16(&self, at: usize) -> u64 {
        u64::from(u16::from_le_bytes([self.bytes[at], self.bytes[at + 1]]))
    }

    fn u32(&self, at: usize) -> u64 {
        u64::from(u32::from_le_bytes(self.bytes[at..at + 4].try_into().unwrap()))
    }

    fn u64(&self, at: usize) -> u64 {
        u64::from_le_bytes(self.bytes[at..at + 8].try_into().unwrap())
    }

    /// `(p_type, p_flags)` for every program header.
    fn segments(&self) -> Vec<(u64, u64)> {
        let off = self.u64(32) as usize;
        let size = self.u16(54) as usize;
        (0..self.u16(56) as usize)
            .map(|i| {
                let at = off + i * size;
                (self.u32(at), self.u32(at + 4))
            })
            .collect()
    }

    /// `(name, sh_type, sh_offset, sh_size, sh_link, sh_entsize)` per section.
    fn sections(&self) -> Vec<(String, u64, u64, u64, u64, u64)> {
        let off = self.u64(40) as usize;
        let size = self.u16(58) as usize;
        let count = self.u16(60) as usize;
        let names = off + self.u16(62) as usize * size;
        let names_at = self.u64(names + 24) as usize;
        (0..count)
            .map(|i| {
                let at = off + i * size;
                let name = self.string(names_at + self.u32(at) as usize);
                (
                    name,
                    self.u32(at + 4),
                    self.u64(at + 24),
                    self.u64(at + 32),
                    self.u32(at + 40),
                    self.u64(at + 56),
                )
            })
            .collect()
    }

    fn string(&self, at: usize) -> String {
        let end = self.bytes[at..].iter().position(|b| *b == 0).unwrap_or(0);
        String::from_utf8_lossy(&self.bytes[at..at + end]).to_string()
    }

    /// Whether a symbol of this name is *defined* in the image.
    ///
    /// `st_shndx` of `SHN_UNDEF` is what a collected block looks like from here,
    /// so an undefined symbol of the right name is a failure and not a find.
    fn defines(&self, symbol: &str) -> bool {
        for (_, kind, offset, size, link, entsize) in self.sections() {
            // SHT_SYMTAB, and its `sh_link` names the string table beside it.
            if kind != 2 || entsize == 0 {
                continue;
            }
            let strings = self.sections()[link as usize].2 as usize;
            for i in 0..(size / entsize) as usize {
                let at = offset as usize + i * entsize as usize;
                if self.string(strings + self.u32(at) as usize) == symbol {
                    return self.u16(at + 6) != 0;
                }
            }
        }
        false
    }

    /// Every `DT_NEEDED` entry, by name — the shared objects a loader would have
    /// to find. A hermetic artifact names none.
    fn needed(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (name, _, offset, size, link, entsize) in self.sections() {
            if name != ".dynamic" || entsize == 0 {
                continue;
            }
            let strings = self.sections()[link as usize].2 as usize;
            for i in 0..(size / entsize) as usize {
                let at = offset as usize + i * entsize as usize;
                if self.u64(at) == 1 {
                    out.push(self.string(strings + self.u64(at + 8) as usize));
                }
            }
        }
        out
    }
}

/// **A Linux artifact is a static PIE with a non-executable stack, linked by
/// both linkers, and it runs.**
///
/// Nothing in the build can observe any of this. A glibc-linked binary builds,
/// links, passes the suite, and fails only on somebody else's machine; an
/// executable stack is the ABSENCE of a section, which is invisible in the
/// source; and `--gc-sections` collecting the Buri stack would leave a program
/// that runs until it recurses. So the properties are asserted on the IMAGE.
///
/// **Both linkers.** `build/link.rs::choose` prefers mold on Linux and falls
/// back to lld, and until this repository had Linux runners every ELF byte it
/// emitted had been accepted by exactly one linker. `--gc-sections` and the
/// stack marking are the linker's behaviour and not the emitter's, so each
/// linker's own output is read.
///
/// **And `--check-reproducible`**, driven through the CLI: two builds from two
/// freshly opened sessions with the cache off, into two directories, compared
/// byte for byte. A repository written here rather than one of the checked-in
/// ones, because both native binaries in `cli/tests/example` are refused by the
/// dev build — `//cmd/server` needs `host.HostFs.readFile` and `//tools/report`
/// needs `host.HostEnv.args`. A small program that only prints is what this
/// needs; a small artifact's bytes are no less reproducible.
#[cfg(target_os = "linux")]
#[test]
fn a_linked_linux_artifact_is_a_static_pie_that_runs() {
    use std::process::Command;

    if !buri::compiler::backend::stencil::AVAILABLE {
        assert!(
            !ci::on(),
            "BURI_CI=1 and this toolchain has no stencils — `the_stencil_libraries_are_real` says \
             which blob is empty."
        );
        eprintln!("ci: skipped (this toolchain builds no runnable native program)");
        return;
    }

    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" };
    let selector = if cfg!(target_arch = "aarch64") { "ARM64" } else { "X86_64" };
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("ci-static-pie-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);

    // One repository per invocation: `buri build` writes into the tree it is
    // given, and an artifact found in a directory two builds wrote to is not
    // unambiguously the one this step produced.
    let repo = |name: &str| {
        let dir = root.join(name).join("repo");
        std::fs::create_dir_all(dir.join("cmd/app")).unwrap();
        std::fs::write(dir.join("REPO.buri"), "# a repository with no tags\n").unwrap();
        std::fs::write(
            dir.join("cmd/app/BUILD.buri"),
            format!("binary {{\n    outputs: [\n        {{ platform: LINUX, arch: {selector} }},\n    ]\n}}\n"),
        )
        .unwrap();
        std::fs::write(
            dir.join("cmd/app/main.buri"),
            "from \"core/host\" import { stdout };\n\
             from \"core/io\" import * as io;\n\
             export fn main(): Result<(), Str> {\n\
             \x20 match (io.println(stdout, \"reproducible\")) {\n\
             \x20   .Ok(_) => .Ok(()),\n\
             \x20   .Err(_) => .Err(\"could not write to standard output\"),\n\
             \x20 }\n\
             }\n",
        )
        .unwrap();
        dir
    };

    let buri = env!("CARGO_BIN_EXE_buri");
    let output = format!("--output=linux/{arch}");

    let mut linked = 0;
    for linker in ["mold", "lld"] {
        let driver = if linker == "lld" { "ld.lld" } else { linker };
        if !Command::new(driver).arg("--version").output().is_ok_and(|o| o.status.success()) {
            assert!(!ci::on(), "BURI_CI=1 and `{driver}` is not on PATH");
            eprintln!("ci: {linker} is not installed, so its image is not read");
            continue;
        }
        linked += 1;
        let dir = repo(linker);
        let out = Command::new(buri)
            .args(["build", "//cmd/app", &output])
            .env("BURI_LINKER", linker)
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "`buri build` with BURI_LINKER={linker} failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );

        let artifact = executable_under(&dir).unwrap_or_else(|| {
            panic!("BURI_LINKER={linker} left no linked artifact under {}", dir.display())
        });
        let image = Image::read(&artifact);

        // ELF type DYN, not EXEC: the artifact is position independent, which is
        // what `-static-pie` asks for and what `-static` alone would not give.
        assert_eq!(
            image.u16(16),
            3,
            "{linker}: the image is not a PIE, so the `-static-pie` link lost half of itself"
        );

        let segments = image.segments();
        // No PT_INTERP. A static PIE self-relocates through musl's `rcrt1.o`
        // before `main`; an INTERP header means the kernel hands the file to a
        // dynamic loader instead, and that loader is exactly what is not
        // guaranteed to exist on the machine the artifact was copied to.
        assert!(
            !segments.iter().any(|(kind, _)| *kind == 3),
            "{linker}: the image has a PT_INTERP header, so it is dynamically loaded"
        );
        assert!(
            image.needed().is_empty(),
            "{linker}: the image still needs shared libraries: {:?}",
            image.needed()
        );

        // One PT_GNU_STACK header, and its flags must not carry PF_X. A missing
        // header is also a failure: the kernel then falls back to the ABI
        // default, which for an executable with no PT_GNU_STACK is an executable
        // stack. `stencil/elf.rs` writes an empty `.note.GNU-stack` into every
        // unit object, and it is the absence of that section that produces this.
        let stack = segments.iter().find(|(kind, _)| *kind == 0x6474_e551);
        let (_, flags) = stack.unwrap_or_else(|| {
            panic!(
                "{linker}: the image has no PT_GNU_STACK header, which is exactly the state an \
                 absent `.note.GNU-stack` produces"
            )
        });
        assert_eq!(
            flags & 1,
            0,
            "{linker}: the stack is EXECUTABLE — `.note.GNU-stack` did not reach the linker"
        );

        // The Buri stack survived `--gc-sections`. The symbol's own `st_size` is
        // zero and that is correct rather than suspicious — `elf.rs` emits the
        // stack as the start of a zero-fill section rather than as a sized
        // object — so the reservation is asserted on the SECTION and only the
        // definedness on the symbol.
        assert!(
            image.defines("buri$stencil$stack"),
            "{linker}: `buri$stencil$stack` is not defined in the linked image — --gc-sections \
             collected the block the stack guard depends on, or the shim stopped naming it"
        );
        let bss = image
            .sections()
            .into_iter()
            .find(|(name, ..)| name == ".bss")
            .unwrap_or_else(|| panic!("{linker}: the image has no .bss at all"));
        assert_eq!(bss.1, 8, "{linker}: .bss is not NOBITS, so the reservation is bytes on disk");
        assert!(
            bss.3 >= 64 * 1024 * 1024,
            "{linker}: .bss is {} bytes, under the 64 MiB the Buri stack reserves — \
             --gc-sections took the block",
            bss.3
        );

        // And it still runs, linked by this linker, on this machine.
        let ran = Command::new(&artifact).output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&ran.stdout),
            "reproducible\n",
            "{linker}: the artifact printed the wrong answer"
        );
    }
    assert!(linked > 0, "neither mold nor ld.lld is installed, so no image was read");

    let dir = repo("reproducible");
    let out = Command::new(buri)
        .args(["build", "//cmd/app", &output, "--check-reproducible"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "`buri build --check-reproducible` failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The one linked executable under a directory.
///
/// Found by reading rather than by knowing the layout: object files are ELF too,
/// and `e_type` is what separates a relocatable from an image.
#[cfg(target_os = "linux")]
fn executable_under(dir: &Path) -> Option<PathBuf> {
    let mut found = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        for entry in std::fs::read_dir(&at).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else { continue };
            if bytes.len() < 20 || bytes[..4] != *b"\x7fELF" {
                continue;
            }
            // ET_EXEC or ET_DYN; ET_REL (1) is a unit object.
            let kind = u16::from_le_bytes([bytes[16], bytes[17]]);
            if kind == 2 || kind == 3 {
                found = Some(path);
            }
        }
    }
    found
}
