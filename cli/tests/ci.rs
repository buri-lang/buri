//! **The workflow is wired the way the suite believes it is.**
//!
//! Three mechanisms in this repository are a promise the test suite makes and
//! `.github/workflows/ci.yml` keeps, and each of them fails *silently* when the
//! workflow half is deleted:
//!
//! * **`BURI_CI=1`.** `cli/tests/harness/ci.rs` turns every
//!   `if !supported() { return; }` in the native suite into a panic when it is
//!   set. Unset, they go back to returning quietly — which is right on a
//!   laptop and is exactly the vacuous green a runner must not report. Delete
//!   the `env:` line and every native test on a runner with a broken toolchain
//!   passes again, and nothing anywhere says so.
//! * **The hoisted runtime-crate step.**
//!   `native::runtime::the_runtime_crate_answers_its_own_tests` shells a nested
//!   `cargo test` on a laptop and asserts a stamp on a runner, because the
//!   nested build is a minute of tokio and rustls that belongs in a step with a
//!   cache and a duration. The stamp is written by
//!   `.github/scripts/test-runtime-crate.sh`. That test catches a *missing*
//!   step by failing; what it cannot catch is a job that runs the suite without
//!   ever having had one.
//! * **The deferrals.** Two tests in `build/repositories.rs` assert
//!   milliseconds, are meaningless in a debug profile, and are run by the
//!   `language-server-budget` job instead of by the `test` matrix.
//!   `ci::deferred_to` names that job in a string. A string naming a job that
//!   has been renamed is a skip wearing a deferral's clothes.
//!
//! And one that is not about the workflow at all:
//!
//! * **The ignore list.** `.github/known-skips.txt` is every `#[ignore]` in the
//!   tree with the defect that removes it. This file holds the tree to that
//!   list exactly — a new `#[ignore]` fails here, on every machine, before it
//!   ever reaches a runner, and a row whose test is gone fails here too.
//!
//! Nothing in here compiles the toolchain or runs a program; it reads two text
//! files and the `.rs` files beside it, and it is the cheapest test in the
//! repository. That is deliberate: a guard nobody minds running is a guard that
//! stays.

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
/// `target/` is skipped because it holds generated sources — the assembled
/// runtime package among them — and a copy of a file is not a second site.
fn rust_sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if name == "target" || name == ".git" || name == "node_modules" {
                    continue;
                }
                walk(&path, out);
            } else if name.ends_with(".rs") {
                out.push(path);
            }
        }
    }
    let root = repo_root();
    let mut out = Vec::new();
    walk(&root, &mut out);
    out.sort();
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
    assert!(
        env.contains("BURI_RT_TESTS_STAMP:"),
        "the workflow-level `env:` block does not set `BURI_RT_TESTS_STAMP`. It is how \
         `.github/scripts/test-runtime-crate.sh` and \
         `native::runtime::the_runtime_crate_answers_its_own_tests` agree on where the step's \
         stamp goes.\n--- the block ---\n{env}"
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

/// Every job that runs the whole suite also runs the runtime-crate step and the
/// no-skips assertion.
///
/// "The whole suite" is `cargo test -p buri` with no name filter after it, and
/// it is the only shape in which
/// `native::runtime::the_runtime_crate_answers_its_own_tests` runs at all — the
/// linux jobs filter to `stencil::` and never reach it, which is why they are
/// not held to the step.
#[test]
fn every_job_that_runs_the_suite_runs_the_hoisted_step_and_the_guard() {
    let text = workflow();
    for (name, body) in jobs(&text) {
        let runs_everything = body
            .lines()
            .map(str::trim)
            .any(|line| line.contains("cargo test -p buri") && !line.contains("--test "));
        if !runs_everything {
            continue;
        }
        assert!(
            body.contains("test-runtime-crate.sh"),
            "the `{name}` job runs the whole suite and never runs \
             .github/scripts/test-runtime-crate.sh. With BURI_CI=1 set, \
             `the_runtime_crate_answers_its_own_tests` does not shell its own nested `cargo` — it \
             asserts that step's stamp — so this job would fail on it. Add the step after the \
             runtime-archive assertion."
        );
        assert!(
            body.contains("assert-no-skips.sh"),
            "the `{name}` job runs the whole suite and never runs \
             .github/scripts/assert-no-skips.sh, so an `#[ignore]` added tomorrow would ride \
             through it green. Pipe the run through `tee` and hand the log to that script."
        );
        println!("{name}: runs the suite, the hoisted step and the no-skips guard");
    }
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
/// `cargo test -p buri`, which does not read that file, and
/// `scripts/test-linux.sh` says why the runner must not be adopted to make it
/// fire — two of the three liveness gates parse libtest's own output and
/// nextest prints neither shape. The file was deleted; the cap it described
/// lives in `cli/tests/harness/hang.rs` and is proved to fire by a test beside
/// it. This is what stops the config coming back as a promise again.
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
        "ci.yml invokes `{runner}`. Two of the three liveness gates read libtest's own output — \
         `assert-no-skips.sh` sums the `ignored` and `filtered out` counts off every \
         `test result:` line, and `assert-suite-ran.sh` reads the census out of a `--nocapture` \
         log — and that runner prints neither shape, so adopting it would disarm both while \
         everything stayed green. `scripts/test-linux.sh` records the argument in full."
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
/// forty-one tests ran nowhere. The two mechanisms that exist to catch a test
/// that does not run were both blind to it — they carry no `#[ignore]`, so the
/// scan above sees nothing, and they reached no `suite.log`, so
/// `assert-no-skips.sh` sees nothing either. Both of those ask "was a test
/// skipped"; this asks the question underneath it, which is whether the suite
/// was ever asked for at all.
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
         skipped, and neither the `#[ignore]` scan nor assert-no-skips.sh can see it — one has \
         nothing to find and the other never gets a log.",
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
