//! Parses every `.buri` source in the repository that is meant to compile: the
//! worked monorepo, the conformance suite, the abort corpus, and the small
//! programs the JavaScript goldens are recorded from. That is the body of
//! source the grammar was written against, so anything that fails to parse
//! here is either a parser bug or drift between the two.
//!
//! It is also where the formatter is held to its three properties: it is a
//! fixed point, it keeps every comment, and it preserves what the corpus
//! means.
//!
//! `tests/reject/` is left out on purpose — those files are supposed to be
//! turned away, some of them by the parser, and each one's expectation is
//! checked exactly by the reject harness in `language/conformance.rs`.
use crate::harness;

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <root>/cli.
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn buri_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            buri_sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "buri") {
            // BUILD.buri and REPO.buri are textproto, not source.
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            if name != "BUILD.buri" && name != "REPO.buri" {
                out.push(p);
            }
        }
    }
}

/// Every `.rs` file under `dir`, for the tests that read this repository's own
/// source rather than its corpora.
fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::path);
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            rust_sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Every `Cargo.toml` under `dir`, sorted, including `dir`'s own.
///
/// `target/` is skipped: `cli/build.rs` assembles the runtime's package there
/// under the name Cargo insists on, so a build directory is full of manifests
/// that are outputs rather than sources. Nothing under `target/` is in the
/// published crate either, which is what the caller is asking about.
fn manifests_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::path);
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            manifests_under(&p, out);
        } else if p.file_name().is_some_and(|n| n == "Cargo.toml") {
            out.push(p);
        }
    }
}

/// The files whose text is Buri source rather than textproto.
fn corpus(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    buri_sources(&root.join("cli/tests/example"), &mut files);
    buri_sources(&root.join("cli/tests/conformance"), &mut files);
    buri_sources(&root.join("cli/tests/crash"), &mut files);
    buri_sources(&root.join("cli/tests/golden_javascript"), &mut files);
    // The documentation's shared preambles are real source and are held to the
    // same standard: they parse, and `buri format` leaves them alone.
    buri_sources(&root.join("cli/src/docs/harness"), &mut files);
    files
}

#[test]
fn every_source_in_the_repository_parses() {
    let root = repo_root();
    let files = corpus(&root);
    assert!(files.len() > 30, "expected the corpus, found {} files", files.len());

    let mut failures = String::new();
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        let rel = path.strip_prefix(&root).unwrap().display().to_string();
        let mut map = buri::diagnostics::SourceMap::new();
        let id = map.add(rel.clone(), path.clone(), text.clone());
        let parsed = buri::parsing::parser::parse(&text, id);
        for e in &parsed.errors {
            failures.push_str(&map.render(e, false));
        }
    }
    assert!(failures.is_empty(), "the corpus does not parse:\n{failures}");
}

fn proto_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            proto_files(&p, out);
        } else {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            if name == "BUILD.buri" || name == "REPO.buri" {
                out.push(p);
            }
        }
    }
}

#[test]
fn every_build_file_reads() {
    let root = repo_root();
    let mut files = Vec::new();
    proto_files(&root.join("cli/tests/example"), &mut files);
    assert!(files.len() >= 7, "expected the example build files, found {}", files.len());

    let mut failures = String::new();
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        let rel = path.strip_prefix(&root).unwrap().display().to_string();
        let mut map = buri::diagnostics::SourceMap::new();
        let id = map.add(rel.clone(), path.clone(), text.clone());
        let errors = if path.file_name().unwrap() == "REPO.buri" {
            buri::build::buildfile::read_repo_config(&text, id).errors
        } else {
            buri::build::buildfile::read_build_file(&text, id).errors
        };
        for e in &errors {
            failures.push_str(&map.render(e, false));
        }
    }
    assert!(failures.is_empty(), "the example build files do not read:\n{failures}");
}

/// `buri format` and `buri gen` must never fight over a file, so formatting is
/// a fixed point: the example build files are already formatted.
#[test]
fn formatting_build_files_is_a_fixed_point() {
    let root = repo_root();
    let mut files = Vec::new();
    proto_files(&root.join("cli/tests/example"), &mut files);
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        let file = buri::diagnostics::FileId(0);
        let format = |source: &str| {
            buri::build::textproto::print(&buri::build::textproto::parse(source, file).document)
        };
        let once = format(&text);
        let twice = format(&once);
        assert_eq!(once, twice, "formatting {} is not stable", path.display());
    }
}

/// Where a `.buri` file is allowed not to be formatted, and why.
///
/// [`every_source_in_the_repository_is_formatted`] walks the whole tree, so a
/// corpus that arrives tomorrow arrives enforced; this list is the whole of the
/// permission to be exempt from it. Every row names a directory whose *subject*
/// is a file nobody laid out — input the formatter is asked a question about,
/// or a recording of what a command printed — and says which suite asks that
/// question instead. A corpus that cannot write such a row is a corpus that
/// should be formatted.
///
/// It is not the list for *generated* source. A corpus written by a tool in
/// this repository is held to the same layout as one written by a person — the
/// tool is where it is fixed, which is [`GENERATED_NOT_BLESSED`] below.
const UNFORMATTED_BY_DESIGN: &[(&str, &str)] = &[
    (
        "cli/tests/formatting",
        "the formatter's own cases. `input.buri` is badly laid out on purpose and \
         `expected.buri` is the one answer recorded for it, both checked by \
         `cargo test -p buri --test formatting`",
    ),
    (
        "cli/tests/checking",
        "seeds with one token deleted, inserted or exchanged. A file the parser could not \
         read whole has no canonical form to be checked against",
    ),
    (
        "cli/tests/recovery",
        "the same mutated population, held to invariants about what one mistake reads as",
    ),
    (
        "cli/tests/linting",
        "what `buri lint` still finds in a file that did not parse whole, so every case is \
         a file that did not",
    ),
    (
        "cli/tests/fuzz",
        "bytes drawn from the generator or spliced into a checked-in source. Nobody wrote \
         them and nobody reads them",
    ),
    (
        "cli/tests/reject",
        "whole programs the front end must turn away, some of them by the parser, each with \
         the diagnostics it draws recorded beside it at the column they point at",
    ),
    (
        "cli/tests/crash",
        "input that once crashed the toolchain, kept byte for byte. The bytes are the case",
    ),
    (
        "cli/tests/repositories/linting/a_bound_the_fix_cannot_reach",
        "the case's subject is a comment written *between two bounds*, which is a position \
         the formatter does not keep — it prints every comment on a line of its own, so \
         laying this file out would delete the question the case asks",
    ),
    (
        "cli/tests/repositories/cli/format_check",
        "the case that runs `buri format` itself: its repository is checked in misformatted \
         and its goldens record both halves of what the command did about that",
    ),
];

/// Where a `.buri` file is checked but must not be **rewritten** here, and how
/// to fix it instead.
///
/// `cli/benches/corpora` holds *output*: `benches/generate.rs` writes it and
/// `manifest.txt` pins its bytes and their digest (`design/PERFORMANCE.md`
/// §3.1). Since `GENERATOR_REVISION` 7 the last thing the generator does to a
/// module is hand it to `formatting::source`, so a saved corpus **is** what
/// `buri format` writes and belongs inside the walk rather than exempt from it.
///
/// What it does not belong inside is `BURI_BLESS`. Laying the file out where it
/// sits would write bytes no generator wrote, and the first thing anybody heard
/// about it would be `--validate` failing on a digest — one hash against
/// another, with the diff that explained it already blessed away. So a drift
/// here is reported with the command that regenerates it, and the file is left
/// exactly as it was.
const GENERATED_NOT_BLESSED: &[(&str, &str)] = &[(
    "cli/benches/corpora",
    "a saved benchmark corpus is output, so it is re-recorded rather than laid out: \
     `BURI_BLESS=1 cargo bench -p buri --bench compiler -- --record=<name>`. If \
     `benches/generate.rs` moved, `GENERATOR_REVISION` and the forty pinned manifests \
     move with it (`design/PERFORMANCE.md` §6)",
)];

/// Whether a path is generated output, and the row saying how to regenerate it.
fn regenerated_rather_than_blessed(rel: &str) -> Option<&'static str> {
    GENERATED_NOT_BLESSED
        .iter()
        .find(|(dir, _)| rel.starts_with(&format!("{dir}/")))
        .map(|(_, how)| *how)
}

/// Whether a file is a template rather than a file.
///
/// A repository case may hold a placeholder its harness fills in before the
/// toolchain ever sees the file — the cross-compilation cases name a platform
/// that is not the runner's that way. The bytes on disk are not Buri until that
/// substitution happens, so there is nothing for a formatter to be right or
/// wrong about.
fn is_template(text: &str) -> bool {
    text.contains("{{") && text.contains("}}")
}

/// Whether a path is a golden — a recording of what a command printed, checked
/// by the case that records it.
///
/// A repository case keeps its expectations in `expected/`, and some of them are
/// `.buri`: the file a command wrote, or the file it was asked to leave alone.
/// Formatting one would edit the recording rather than the source it is a
/// recording of.
fn is_golden(rel: &str) -> bool {
    rel.contains("/expected/")
}

/// The canonical form of one file, whichever of the three it is.
///
/// `commands::format::file` decides between source and textproto by the name,
/// which is what `buri format` does. The third is the standard library, whose
/// modules declare operations the backend supplies and so have `fn`s with no
/// body — a syntax error anywhere else, and the reason this test knows a fact
/// about a path that the command does not need to.
fn canonical(rel: &str, name: &str, text: &str) -> Option<String> {
    if rel.starts_with("cli/src/compiler/standard_library/sources/") {
        return buri::formatting::std_source(text);
    }
    buri::commands::format::file(name, text)
}

/// Every file the repository *has*, which is not the same question as every
/// file under its directory.
///
/// A walk of the tree finds whatever is on disk when it runs, and on a runner
/// that is more than the repository: `CARGO_TARGET_DIR` is `target/` **inside
/// the checkout** there, and half the suites materialize a repository under it
/// while they run — a reject case with a syntax error in it, a copy of the
/// conformance tree with its indentation taken off, a scratch monorepo a
/// command is about to rewrite. None of that is anybody's source, none of it
/// outlives the test that wrote it, and which of it exists at the moment of the
/// walk depends on which tests happen to be running beside this one. Four CI
/// jobs went red on exactly that, each naming a different `target/tmp/…` file,
/// and no run whose target directory lives outside the checkout could reproduce
/// any of it.
///
/// So the question is asked of the repository, which already answers it, and
/// the answer it gives is **`.gitignore`**: a file is this repository's unless
/// the repository says it is throwaway. Everything a build writes is under
/// `target/`, which is ignored, so the answer still cannot be moved by a build,
/// by a concurrent test, or by where `CARGO_TARGET_DIR` happens to point. It
/// also cannot go quietly empty — an empty list fails the floor below.
///
/// It is deliberately **not** the narrower question, "a file it *tracks*". A
/// file an author has only just written is not tracked until it is committed,
/// so a gate that asks that one is blind to exactly the files most likely to be
/// wrong — the new ones. It happened: a slice wrote the actor corpus's two new
/// sources, ran this gate green because neither file existed as far as
/// `git ls-files` was concerned, committed, and turned the *next* run red — one
/// tree after the one it was gating. Tracked
/// **plus** untracked-and-not-ignored is the same set one commit later, which
/// is the set the gate is supposed to be standing in front of.
///
/// A nested checkout is somebody else's repository — a linked worktree parked
/// under `.claude/`, a vendored clone — and `git` reports it as the directory
/// itself, with a trailing slash, rather than looking inside. Neither does
/// this: those files answer to that checkout's gate, not this one's.
///
/// This asks `git` for the list rather than reimplementing `.gitignore`, and it
/// is the one place in the suite that shells out to it. The suite already reads
/// `.github/`, `design/` and `formal/` off disk, so it already only runs inside
/// this repository; needing the checkout those came from is that assumption
/// said out loud.
fn repository_files(root: &Path) -> Vec<PathBuf> {
    let mut names = git_names(root, &["ls-files", "-z"]);
    names.extend(git_names(root, &["ls-files", "-z", "--others", "--exclude-standard"]));
    let mut out: Vec<PathBuf> = names
        .into_iter()
        .filter(|name| !name.ends_with('/'))
        .map(|name| root.join(name))
        .collect();
    // No path is both tracked and untracked, so the two answers are disjoint
    // today. This says so rather than depending on it, and sorts, because the
    // order files are checked in is the order a failure lists them.
    out.sort();
    out.dedup();
    out
}

/// One `git ls-files` question, asked with `-z` so a path with a newline or a
/// quote in it comes back as its own bytes rather than as git's quoting.
fn git_names(root: &Path, args: &[&str]) -> Vec<String> {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .expect("`git ls-files` runs: this suite runs inside the repository's own checkout");
    assert!(
        out.status.success(),
        "`git {}` failed in {}: {}",
        args.join(" "),
        root.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Every `.buri` file in the repository, and the reason the exempt ones are
/// exempt.
fn every_buri_file(root: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let all = repository_files(root);
    let mut checked = Vec::new();
    let mut exempt = Vec::new();
    for path in all {
        if path.extension().is_none_or(|x| x != "buri") {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        if UNFORMATTED_BY_DESIGN.iter().any(|(dir, _)| rel.starts_with(&format!("{dir}/")))
            || is_golden(&rel)
            || std::fs::read_to_string(&path).is_ok_and(|t| is_template(&t))
        {
            exempt.push(path);
        } else {
            checked.push(path);
        }
    }
    (checked, exempt)
}

/// **Every `.buri` file in the repository is already what `buri format` would
/// write.**
///
/// This is `buri format --check` over the tree, as a test: one indentation, one
/// line width, one place a comment sits, in the standard library, in the
/// corpora, in the fixtures a person reads to learn what a repository looks
/// like. A repository whose own sources are laid out four different ways is one
/// where the formatter's answer is a matter of which file you opened.
///
/// It goes through `commands::format::file`, which is the same entry point the
/// command uses, so this and `buri format` cannot disagree about which of the
/// two printers a file goes through — source or textproto, decided by the name.
/// The one file this knows about and the command does not is a **standard
/// library module**, where a `fn` may be declared without a body: no repository
/// but this one holds one, and `formatting::Dialect` is what says so.
///
/// The walk is the enforcement and [`UNFORMATTED_BY_DESIGN`] is the whole of
/// the exemption, so a corpus added tomorrow is checked without anybody
/// remembering to add it here. [`GENERATED_NOT_BLESSED`] is not a second
/// exemption: those files are checked like every other, and only the *fix* is
/// different — they are regenerated rather than laid out where they sit.
///
/// ```text
/// buri format                                                   # in a repository
/// BURI_BLESS=1 cargo test -p buri --test language corpus::       # over this tree
/// ```
#[test]
fn every_source_in_the_repository_is_formatted() {
    let root = repo_root();
    let (files, exempt) = every_buri_file(&root);
    assert!(files.len() > 300, "found {} files to check; the walk is broken", files.len());
    assert!(!exempt.is_empty(), "nothing is exempt; the exemption list has stopped matching");
    let blessing = std::env::var_os("BURI_BLESS").is_some();

    let mut drifted = Vec::new();
    let mut stale_output = Vec::new();
    let mut refused = Vec::new();
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        let rel = path.strip_prefix(&root).unwrap_or(path).display().to_string();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        match canonical(&rel, &name, &text) {
            Some(formatted) if formatted != text => {
                if let Some(how) = regenerated_rather_than_blessed(&rel) {
                    stale_output.push((rel, how));
                } else if blessing {
                    std::fs::write(path, &formatted).unwrap();
                } else {
                    drifted.push(rel);
                }
            }
            Some(_) => {}
            // What `buri format --check` says about the same file: a file the
            // formatter could not read whole is not a file it has checked, and
            // a check that passed one would be reporting a gate it did not run.
            None => refused.push(rel),
        }
    }

    assert!(
        refused.is_empty(),
        "the formatter cannot vouch for {} file(s), so nothing above checked them:\n  {}\n\
         Either the file has a syntax error, or its corpus belongs in \
         `UNFORMATTED_BY_DESIGN` with a row saying which suite asks it its question.",
        refused.len(),
        refused.join("\n  ")
    );
    assert!(
        stale_output.is_empty(),
        "{} generated `.buri` file(s) are not what `buri format` would write, and \
         blessing is not the fix for them:\n  {}\n{}",
        stale_output.len(),
        stale_output.iter().map(|(rel, _)| rel.as_str()).collect::<Vec<_>>().join("\n  "),
        stale_output.first().map(|(_, how)| *how).unwrap_or_default()
    );
    assert!(
        drifted.is_empty(),
        "{} of {} `.buri` file(s) are not what `buri format` would write:\n  {}\n\
         Run `buri format` in the repository, or add the corpus to \
         `UNFORMATTED_BY_DESIGN` with the reason its files are laid out by hand.",
        drifted.len(),
        files.len(),
        drifted.join("\n  ")
    );
    eprintln!("{} formatted, {} exempt by design", files.len(), exempt.len());
}

/// Runs one `git` command in `root` and insists it worked.
fn git(root: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("cannot run `git {}`: {e}", args.join(" ")));
    assert!(
        out.status.success(),
        "`git {}` failed in {}: {}",
        args.join(" "),
        root.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// **What [`repository_files`] answers, on a repository built to ask it four
/// questions at once.**
///
/// The gate above is only as wide as this list, and the list is what got the
/// slice that added the actor corpus: both of its new `.buri` files were
/// written, checked green, and committed, because a walk of *tracked* files
/// cannot see a file that has never been committed. The gate went red on the
/// next tree instead — after the commit it was standing in front of.
///
/// So the four questions, in one scratch repository:
///
/// * a **committed** file is in the list, as it always was;
/// * a file that exists but has **never been added** is in it too — this is the
///   assertion the old walk failed;
/// * nothing under **`target/`** is, because `.gitignore` says so, which is the
///   guarantee the tracked-only walk was bought for and this must not spend:
///   the suites materialize whole repositories under it while they run;
/// * nothing inside a **nested checkout** is, because those files answer to
///   that repository's gate — an agent's linked worktree parked under
///   `.claude/` is a tree full of somebody else's half-finished sources.
///
/// It asks the real thing rather than a re-reading of it: the file the walk is
/// blind to is genuinely misformatted, and the exemption list keeps working on
/// a file that is not committed either.
///
/// The repository is a [`harness::Scratch`], so it lives under
/// `CARGO_TARGET_TMPDIR` — inside the checkout when `CARGO_TARGET_DIR` is,
/// which is exactly the ignored ground this is testing — and its `Drop` takes
/// it away whether the test passed or panicked on the way out.
#[test]
fn the_walk_sees_a_file_that_is_not_committed_yet() {
    // Two spaces where the formatter writes four, so the canonical form of
    // this text is not this text.
    const DRIFTED: &str = "export struct Probe {\n  export flag: Bool,\n}\n";

    let scratch = harness::Scratch::empty("untracked-walk");
    let root = &scratch.root;
    let laid_out = canonical("cli/lib/probe.buri", "probe.buri", DRIFTED)
        .expect("the probe source parses, so it has a canonical form");
    assert_ne!(laid_out, DRIFTED, "the probe source is already formatted, so it proves nothing");

    scratch.write(".gitignore", "target/\n");
    scratch.write("cli/lib/committed.buri", &laid_out);
    scratch.write("cli/lib/fresh.buri", DRIFTED);
    // What a build and a concurrent suite leave behind, and what the formatting
    // suite's own corpus is: misformatted on purpose, and exempt by design.
    scratch.write("target/tmp/scratch/what_a_build_wrote.buri", DRIFTED);
    scratch.write("cli/tests/formatting/indent/input.buri", DRIFTED);
    scratch.write("worktrees/agent/cli/lib/somebody_elses.buri", DRIFTED);

    git(root, &["init", "-q"]);
    git(root, &["add", ".gitignore", "cli/lib/committed.buri"]);
    git(&root.join("worktrees/agent"), &["init", "-q"]);

    let (checked, exempt) = every_buri_file(root);
    let rel = |files: &[PathBuf]| -> Vec<String> {
        files
            .iter()
            .map(|p| p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/"))
            .collect()
    };
    let (checked, exempt) = (rel(&checked), rel(&exempt));

    assert!(
        checked.contains(&"cli/lib/fresh.buri".to_string()),
        "a `.buri` file that exists but has never been committed is not in the walk, \
         so the gate is blind to exactly the files most likely to be misformatted — \
         the new ones. Checked: {checked:?}"
    );
    assert!(
        checked.contains(&"cli/lib/committed.buri".to_string()),
        "a committed `.buri` file fell out of the walk. Checked: {checked:?}"
    );
    assert!(
        exempt.contains(&"cli/tests/formatting/indent/input.buri".to_string()),
        "`UNFORMATTED_BY_DESIGN` stopped covering a file that is not committed yet, so a \
         new case in one of those corpora would be reported as drift. Exempt: {exempt:?}"
    );
    for found in checked.iter().chain(&exempt) {
        assert!(
            !found.starts_with("target/"),
            "{found} is under `target/`, which `.gitignore` covers: what a build and the \
             suites running beside this one write is not this repository's source, and \
             which of it exists depends on what is running"
        );
        assert!(
            !found.starts_with("worktrees/"),
            "{found} is inside a nested checkout, whose files answer to that repository's \
             gate rather than this one's"
        );
    }
}

/// Formatting is a fixed point and never produces something that does not
/// parse. That the *meaning* survives is
/// `formatting_the_corpus_preserves_what_it_means`, below, which formats the
/// conformance repository and runs it again — a token comparison cannot
/// express it, because the formatter is allowed to drop a redundant
/// parenthesis and an optional trailing comma.
#[test]
fn formatting_is_a_fixed_point() {
    let root = repo_root();
    let files = corpus(&root);

    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        let Some(once) = buri::formatting::source(&text) else {
            panic!("{} does not format", path.display());
        };
        let twice = buri::formatting::source(&once).expect("formatted output re-formats");
        assert_eq!(once, twice, "formatting {} is not stable", path.display());
    }
}

/// Formatting keeps every comment.
///
/// A fixed point cannot see this on its own: `format(format(x)) == format(x)`
/// holds perfectly well when both sides have lost the same comment. So the
/// comments are compared against the *source*, as a set rather than a
/// sequence, because the leading import run is sorted and a comment travels
/// with the import it was written above.
///
/// This is what `token_shape` was built for. The token half of the shape is
/// deliberately not compared: the formatter may drop a redundant parenthesis
/// and add a trailing comma, and a test that forbade that would be a test
/// against formatting.
///
/// `source_unchecked` rather than `source`, on purpose. `source` refuses
/// output whose comments moved, so asking it this question can only ever get
/// the answer yes; the property belongs to the printer underneath.
#[test]
fn formatting_keeps_every_comment() {
    let root = repo_root();
    let files = corpus(&root);
    let mut comments = 0;

    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        let once = buri::formatting::source_unchecked(&text);
        let (mut before, mut after) =
            (buri::formatting::comment_shape(&text), buri::formatting::comment_shape(&once));
        comments += before.len();
        before.sort();
        after.sort();
        assert_eq!(
            before,
            after,
            "formatting {} does not keep its comments",
            path.strip_prefix(&root).unwrap().display()
        );
    }
    assert!(comments > 500, "only {comments} comments in the corpus; the scan is not working");
}

/// Formatting preserves *meaning*: the conformance repository, formatted from
/// end to end, still compiles and still gets the same answers.
///
/// This is the property `language/corpus.rs` used to name a file for and never had. It
/// cannot be a comparison of text, because the whole point of the formatter is
/// to change text, and it cannot be a comparison of tokens, because a
/// redundant parenthesis and an optional trailing comma are the formatter's to
/// drop. So it is the same suite run twice, once against the source as checked
/// in and once against the source as formatted.
///
/// A difference rather than a success, deliberately: a suite that is failing
/// for a reason of its own fails the same way in both copies, and what this
/// asks is only that formatting did not change the answer. The floor under the
/// count is what stops that from being vacuous.
///
/// **The copy is unformatted first, and that is new.** This used to format the
/// corpus as it was checked in, which worked while the corpus was laid out by
/// hand and asks nothing now that
/// [`every_source_in_the_repository_is_formatted`] holds: formatting a
/// formatted file rewrites nothing, and a test that rewrote nothing was
/// comparing a suite with itself. So every line's leading whitespace comes off
/// first — a change to layout and to nothing else, because Buri has no
/// multi-line string literal for an indent to be *inside* — and what is
/// formatted is that. It is the input the formatter exists to answer, and the
/// question is the one this test always asked.
#[test]
fn formatting_the_corpus_preserves_what_it_means() {
    let root = repo_root();
    let source = root.join("cli/tests/conformance");
    let plain = harness::Scratch::copy_of("meaning-plain", &source);
    let formatted = harness::Scratch::copy_of("meaning-formatted", &source);

    let mut files = Vec::new();
    buri_sources(&formatted.root, &mut files);
    assert!(files.len() > 30, "expected the suite, found {} files", files.len());
    let mut changed = 0;
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        let flattened: String =
            text.lines().map(|l| format!("{}\n", l.trim_start())).collect();
        let once = buri::formatting::source(&flattened)
            .unwrap_or_else(|| panic!("{} does not format", path.display()));
        if once != flattened {
            changed += 1;
        }
        std::fs::write(path, &once).unwrap();
    }
    assert!(changed > 10, "formatting rewrote only {changed} of the suite's files");

    let before = plain.run(&["test", "//...", "--force"]);
    let after = formatted.run(&["test", "//...", "--force"]);
    assert_eq!(
        after.code,
        before.code,
        "the formatted suite exits {} rather than {}:\n{}",
        after.code,
        before.code,
        harness::indent(&after.all())
    );
    let (want, got) = (before.tests_passed(), after.tests_passed());
    assert!(want > 500, "only {want} assertions ran; the comparison proves nothing");
    assert_eq!(got, want, "the formatted suite passes {got} assertions rather than {want}");
    eprintln!("formatting: {changed} files rewritten, {got} assertions unchanged");
}

/// The dependency **bar**, and that is a claim a test should hold rather than a
/// comment.
///
/// It used to be "no dependencies at all", and native code generation ended
/// that: a retargetable code generator is not something this repository can
/// write. So the promise is replaced by a bar rather than quietly weakened,
/// because a list is a thing people add to and a bar is not (the root
/// `Cargo.toml` states it in full):
///
///   A dependency is admissible only if it is a code generator or a platform
///   interface this repository could not reasonably write, it is behind a
///   cargo feature the default build can turn off, and its absence degrades
///   the toolchain rather than breaking it.
///
/// This checks the two halves a file can check. **The admitted set is closed**:
/// `inkwell`, and nothing else. And **it is optional**, so the default build
/// resolves nothing — which is the clause that makes the other two
/// enforceable, because a feature that could not be turned off is a dependency
/// by another name.
///
/// It stays load-bearing for the reason it was written: a Zed extension has no
/// choice about depending on `zed_extension_api`, so it lives outside the
/// workspace, and adding it to `members` is a one-line change that would make
/// `cargo test -p buri` resolve crates.io with nothing else noticing.
///
/// **Two manifests and two admitted sets, and the second half of this test is
/// the stricter one.** `cli/runtime/manifest.toml` is the native runtime's
/// package — `libburi_rt.a`, assembled and built by `cli/build.rs` and
/// `include_bytes!`d into the binary. No `cargo` command this suite runs
/// resolves it, so without this the first crate in the runtime would arrive in
/// complete silence; and the archive is linked into every native binary the
/// compiler produces, so a crate admitted there is a crate shipped inside every
/// program a *user* builds.
///
/// That set was empty until the `net` feature. It is now six crates, and the
/// clause that replaces emptiness is **exactness**: the assertion below is an
/// equality against a list written out here, not a prefix match and not a
/// count. A seventh crate fails, a rename fails, and a removal fails — the last
/// on purpose, because "tokio quietly left the runtime" is as much a change to
/// what every user ships as "quinn quietly joined it".
///
/// **Quinn has now joined it, and not quietly — which is the second half this
/// test grew.** `net-h3` is a feature the default build does *not* turn on, so
/// the exactness above is not enough on its own: a crate could be moved from
/// `net-h3` to `net` and the set would still be the set. So the h3 leg is
/// checked as well as the list — `net-h3` exists, it implies `net`, `default`
/// is still `net` alone, and `quinn` is behind `net-h3` and behind nothing
/// else. A user who never asks for HTTP/3 must not resolve a QUIC stack, and
/// that is a property of the *feature graph* rather than of the dependency
/// list.
///
/// The third assertion is the one that keeps the published crate whole. Cargo
/// skips any subdirectory of a package that holds a `Cargo.toml`, before
/// `include` is consulted and with no way to override it, so a manifest under
/// `cli/` by that name silently deletes its whole directory from `cargo package
/// -p buri` — which is how the runtime's sources stopped shipping once it
/// became a package, and why its manifest is `manifest.toml` today. The
/// invariant is cheap to state and exact: **`cli/Cargo.toml` is the only
/// `Cargo.toml` under `cli/`.**
#[test]
fn dependencies_stay_behind_the_bar() {
    /// The whole admitted set, by prefix.
    const ADMITTED: &[&str] = &["inkwell"];
    const BAR: &str = "The bar is in the root Cargo.toml: a dependency is admissible only if \
                       it is a code generator or a platform interface this repository could not \
                       reasonably write, it is behind a cargo feature the default build can turn \
                       off, and its absence degrades the toolchain rather than breaking it.";

    /// Every line under a `[dependencies]`-shaped table, as `(name, line)`.
    ///
    /// A hand-rolled reader rather than a TOML parser, because the bar is about
    /// there being no dependency to parse: a test that needed a crate to check
    /// that there are no crates would be its own counterexample.
    fn declared(manifest: &str) -> Vec<(String, String)> {
        let mut in_deps = false;
        let mut found = Vec::new();
        for line in manifest.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                // `[dependencies]` and every `[target.'...'.dependencies]`.
                in_deps = line.contains("dependencies");
                continue;
            }
            if !in_deps || line.is_empty() || line.starts_with('#') {
                continue;
            }
            // A declaration is `name = ...`, and a whole one is one line. Both
            // halves of that are a guard: a line with no `=` and a "name" that
            // is not a crate name are the continuation lines of a wrapped
            // inline table — `"http1",`, `] }` — which this would otherwise
            // report as dependencies called `"http1",` and `]`. Both manifests
            // owe the scanner one entry per line and both say so where their
            // tables are written.
            let Some((lhs, _)) = line.split_once('=') else { continue };
            let name = lhs.trim();
            if name.is_empty()
                || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                continue;
            }
            found.push((name.to_string(), line.to_string()));
        }
        found
    }

    let cli = std::fs::read_to_string(repo_root().join("cli/Cargo.toml")).expect("cli/Cargo.toml");
    let mut seen = 0;
    for (name, line) in declared(&cli) {
        assert!(
            ADMITTED.iter().any(|a| name.starts_with(a)),
            "cli/Cargo.toml declares `{name}`, which is not in the admitted set {ADMITTED:?}.\n{BAR}"
        );
        assert!(
            line.contains("optional = true"),
            "the dependency `{name}` is not optional, so the default build cannot turn it \
             off.\n{BAR}"
        );
        seen += 1;
    }
    assert!(seen > 0, "the admitted dependencies vanished; this test is now asserting nothing");

    // -- the runtime's set, which is closed by an exact list ------------------
    //
    // One step stronger than the toolchain's, and for a reason that is about
    // who pays: `cli/build.rs` links what this package builds into every native
    // binary `buri` produces, so a crate here is not a crate a contributor
    // installs — it is a crate every *user* ships.
    //
    // Pinned in full and compared as a set. Sorted, because the order in the
    // manifest is an argument's order and not a fact.
    //
    // `ring` is on this list although no line of the runtime names it: it is
    // `rustls`'s crypto provider and the code reaches it through
    // `rustls::crypto::ring`. It is declared as a direct dependency anyway, and
    // this is the reason — the crate ships inside every native binary this
    // compiler produces, and a crate that ships must be visible to the test
    // that guards what ships. A provider pulled in only by another crate's
    // feature flag would be 845 KB of object code no assertion here could see.
    //
    // `quinn` is the sixth and the only one behind `net-h3`. It is on the same
    // list as the other five because the list is of what the runtime *may*
    // link, not of what a default toolchain does — and the h3 leg below is what
    // holds the second half, that asking for it is a build-time choice nobody
    // makes by accident.
    const RUNTIME_ADMITTED: &[&str] =
        &["hyper", "quinn", "ring", "rustls", "tokio", "tungstenite"];
    let runtime = std::fs::read_to_string(repo_root().join("cli/runtime/manifest.toml"))
        .expect("cli/runtime/manifest.toml");
    let mut runtime_deps: Vec<String> =
        declared(&runtime).into_iter().map(|(n, _)| n).collect::<Vec<_>>();
    runtime_deps.sort();
    assert_eq!(
        runtime_deps, RUNTIME_ADMITTED,
        "cli/runtime/manifest.toml's dependency set is not the one this repository decided on. \
         The runtime's archive is linked into every native binary this compiler produces, so its \
         admitted set is closed by this exact list rather than by a prefix: a seventh crate, a \
         rename and a removal are all changes to what every user ships.\n{BAR}"
    );
    // Every one of them optional, and every one behind the one feature. Two
    // separate facts: `optional = true` is what lets the feature turn it off,
    // and `net = [...]` is what says which feature does.
    for (name, line) in declared(&runtime) {
        assert!(
            line.contains("optional = true"),
            "the runtime dependency `{name}` is not optional, so `net` cannot turn it off and \
             `cargo build --no-default-features` would still resolve it.\n{BAR}"
        );
        assert!(
            runtime.contains(&format!("\"dep:{name}\"")),
            "the runtime dependency `{name}` is optional but no feature enables it, so it is \
             resolved by nothing and shipped by nothing"
        );
    }
    assert!(
        runtime.contains("default = [\"net\"]"),
        "the runtime's default feature set is no longer `net`. A toolchain whose runtime cannot \
         speak the network by default makes `Net` a build-flag question for every user rather \
         than a capability question for every program"
    );

    // -- the h3 leg: a feature the default build does not turn on -------------
    //
    // The concurrency note gated HTTP/3 behind configuration until the crate is
    // trusted, and a cargo feature outside `default` is what that gate is. Four
    // facts, because the set-equality above cannot see any of them: that the
    // feature exists at all, that it implies `net` (QUIC carries TLS inside the
    // transport and wants the same reactor), that `quinn` is behind it, and
    // that `quinn` is behind *nothing else* — the last being the one that would
    // silently break, by a line moving from `net-h3 = [...]` up into
    // `net = [...]` and every user shipping a QUIC stack they never asked for.
    let feature_line = |name: &str| -> String {
        runtime
            .lines()
            .find(|line| line.trim_start().starts_with(&format!("{name} = [")))
            .unwrap_or_else(|| {
                panic!(
                    "cli/runtime/manifest.toml declares no `{name}` feature, so the h3 gate is \
                     not there to check"
                )
            })
            .to_string()
    };
    let net_feature = feature_line("net");
    let h3_feature = feature_line("net-h3");
    assert!(
        h3_feature.contains("\"net\""),
        "`net-h3` does not imply `net`: {h3_feature}. QUIC carries TLS 1.3 inside the transport \
         and runs on the same reactor, so an h3 build without the networking crates would be a \
         QUIC stack with no crypto provider and nowhere to run"
    );
    assert!(
        h3_feature.contains("\"dep:quinn\""),
        "`net-h3` does not enable `quinn`, so the feature turns nothing on: {h3_feature}"
    );
    assert!(
        !net_feature.contains("quinn"),
        "`quinn` is enabled by `net`, which is on by default, so every `cargo install buri` now \
         resolves and ships a QUIC stack: {net_feature}. HTTP/3 is gated behind `net-h3` \
         deliberately — cli/runtime/manifest.toml's feature block argues it"
    );
    let default_feature = feature_line("default");
    assert!(
        !default_feature.contains("net-h3"),
        "`net-h3` is in the runtime's default feature set, so the gate is open for everyone: \
         {default_feature}"
    );
    assert!(
        runtime.contains("[dependencies]"),
        "cli/runtime/manifest.toml no longer has a `[dependencies]` table, so the check above is \
         reading a manifest whose shape it does not recognise and asserting nothing"
    );

    // -- and the invariant that keeps the published crate whole ---------------
    //
    // `cargo package` skips a nested package unconditionally, so a second
    // `Cargo.toml` under `cli/` deletes its own directory from the tarball. The
    // walk is over the tree rather than over a list of known places, because
    // the failure this catches is a *new* manifest somebody adds.
    let mut manifests = Vec::new();
    manifests_under(&repo_root().join("cli"), &mut manifests);
    manifests.sort();
    let manifests: Vec<String> = manifests
        .iter()
        .filter_map(|p| p.strip_prefix(repo_root()).ok())
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    assert_eq!(
        manifests,
        vec!["cli/Cargo.toml".to_string()],
        "there is a `Cargo.toml` under `cli/` other than the package's own. `cargo package -p \
         buri` skips any subdirectory that holds one — before `include` is consulted, and with no \
         way to override it — so every file beside it silently stops shipping in the published \
         crate. The native runtime's manifest is `cli/runtime/manifest.toml` for exactly this \
         reason; see `cli/build.rs`'s header."
    );

    // The default feature set is the one `cargo install buri` gets, and it must
    // not be the one that needs LLVM installed (BUILD-AND-WATCH.md §2). The
    // assertion is on the *property* rather than on the exact list: what has to
    // hold is that a default install can compile a native debug binary with no
    // crate behind it, which is `backend-stencil`, and that it does not need
    // LLVM.
    let default = cli
        .lines()
        .find_map(|l| l.trim().strip_prefix("default = "))
        .expect("cli/Cargo.toml declares a default feature set");
    assert!(
        default.contains("backend-stencil"),
        "the default build no longer has a native backend that needs no crate: {default}"
    );
    assert!(
        !default.contains("backend-llvm"),
        "the default feature set changed; `cargo install buri` must not require LLVM"
    );
    assert!(
        cli.contains("backend-llvm = [\"dep:inkwell\"]"),
        "`backend-llvm` no longer gates inkwell, so a default build may now need LLVM 21"
    );

    let root = std::fs::read_to_string(repo_root().join("Cargo.toml")).expect("Cargo.toml");
    assert!(
        root.contains("exclude = [\"editors/zed\"]"),
        "the root Cargo.toml no longer excludes editors/zed, so the Zed extension's dependencies \
         are about to become the toolchain's. (`cli/runtime` used to be a second entry and needs \
         none: it is not a package in the tree, so there is nothing there to resolve.)"
    );
}

/// The editor integration is one directory, and its pieces refer to each other
/// by path. A rename that misses one presents as an extension that installs and
/// then does nothing, which is the hardest kind of breakage to notice.
#[test]
fn the_editor_integration_is_whole() {
    for rel in [
        "editors/tree-sitter-buri/grammar.js",
        "editors/tree-sitter-buri/src/scanner.c",
        "editors/tree-sitter-buri/check.sh",
        // The colour test and the one file it is about. `check.sh` runs it, so
        // a rename that misses either presents as a check that quietly stops
        // asserting anything.
        "editors/tree-sitter-buri/check_highlighting.sh",
        "editors/tree-sitter-buri/fixture/REPO.buri",
        "editors/tree-sitter-buri/fixture/lib/reference/sections.buri",
        "editors/zed/extension.toml",
        "editors/zed/src/lib.rs",
        "editors/zed/languages/buri/config.toml",
        "editors/zed/languages/buri/highlights.scm",
        "editors/zed/languages/buri/indents.scm",
        "editors/zed/languages/buri/outline.scm",
        // The second grammar and the second language: a BUILD.buri ends in
        // `.buri` and is textproto rather than Buri.
        "editors/tree-sitter-buri-build/grammar.js",
        "editors/tree-sitter-buri-build/check.sh",
        "editors/zed/languages/buri-build/config.toml",
        "editors/zed/languages/buri-build/highlights.scm",
        "editors/zed/languages/buri-build/indents.scm",
        "editors/zed/languages/buri-build/outline.scm",
    ] {
        assert!(repo_root().join(rel).is_file(), "{rel} is missing");
    }

    // The queries live once. `check.sh` compiles them from the Zed directory,
    // so a second copy under the grammar would be a second thing to keep in
    // step and nothing would report the drift.
    assert!(
        !repo_root().join("editors/tree-sitter-buri/queries").exists(),
        "there are two copies of the highlight queries; there must be one"
    );
}

/// **The `backend-llvm` delta is confined to the files the verification bar
/// names**, so the bar may run the feature suite as a *delta* rather than as a
/// second copy of the default one.
///
/// `cli/tests/README.md`'s "The five-minute budget" defines the bar, and its
/// feature leg runs only the LLVM-gated tests: the fifteen `backend::llvm` unit
/// tests, `native::llvm`, `native::agreement`, and the whole `fuzz` binary.
/// Everything else in a `--features backend-llvm` run is byte-for-byte the same
/// work the default leg just did, and running it twice buys nothing — which is
/// **dedup, not less coverage**, and only while this test passes.
///
/// The claim it holds is the one that makes that safe: a `cfg` on
/// `backend-llvm` exists nowhere but here. If it did exist somewhere else, some
/// test outside the delta would behave differently under the feature, the local
/// bar would stop exercising that behaviour, and nothing would say so. Now
/// something does, and the fix is either to widen the bar's feature leg or to
/// move the gate.
///
/// Two things this deliberately does **not** try to be:
///
///  * It is not a reachability analysis. `backend::select` returns the LLVM
///    backend for `(native, Release)` under the feature, so a test that drives
///    a native `--release` build through the CLI would differ without any `cfg`
///    of its own. Every `--release` in the suite today targets a `platform: JS`
///    output, and `native_ready` refuses `Js` before `select` is consulted, so
///    there is no such test — but that is an argument in the report and in the
///    README, not something this file can check.
///  * It is not CI's bar. **CI runs everything under both feature sets**; this
///    is the local wave loop, where the same run happening twice is the whole
///    of what is being removed.
#[test]
fn the_llvm_feature_is_confined_to_the_files_the_bar_names() {
    /// Every file allowed to gate on the feature. A prefix matches a directory:
    /// the backend's own modules are compiled only under it, so a `cfg` inside
    /// one cannot widen anything.
    const GATED: &[&str] = &[
        // `mod llvm`, and `select`'s release arm.
        "cli/src/compiler/backend/mod.rs",
        "cli/src/compiler/backend/llvm/",
        // The module declarations; `llvm` is the 59 tests.
        "cli/tests/native/main.rs",
        "cli/tests/native/llvm.rs",
        // Same-named tests that do *more* under the feature: both hold a
        // `NATIVES` table that gains an `llvm` row, so both are in the delta.
        "cli/tests/native/agreement.rs",
        "cli/tests/fuzz.rs",
        // The end-to-end tier, which builds each of its whole programs with
        // *whichever* native backend the toolchain has: the copy-and-patch one
        // on a default build, and LLVM under the feature. So its rows do more
        // under the feature rather than appearing as new ones, exactly as
        // `agreement`'s do, and the bar's feature leg selects `e2e::` beside
        // `llvm::` and `agreement::`.
        "cli/tests/native/e2e.rs",
    ];

    // Spelled in two pieces so that this file is not its own first hit, and so
    // the guard covers the file the guard is written in.
    let needle = format!("feature = {q}backend-llvm{q}", q = '"');

    let mut sources = Vec::new();
    rust_sources(&repo_root().join("cli/src"), &mut sources);
    rust_sources(&repo_root().join("cli/tests"), &mut sources);
    assert!(sources.len() > 50, "found {} Rust sources; the walk is broken", sources.len());

    let mut gates = 0;
    for path in &sources {
        let rel = path.strip_prefix(repo_root()).unwrap_or(path).to_string_lossy().to_string();
        let text = std::fs::read_to_string(path).unwrap();
        for (n, line) in text.lines().enumerate() {
            // Prose names the feature all over the repository, and prose
            // cannot change what a test does.
            if line.trim_start().starts_with("//") || !line.contains(&needle) {
                continue;
            }
            gates += 1;
            assert!(
                GATED.iter().any(|g| rel.starts_with(g)),
                "{rel}:{} gates on `backend-llvm`, and that file is not in the \
                 verification bar's feature leg. Either move the gate into one of \
                 {GATED:?}, or widen the leg in cli/tests/README.md's \
                 \"The five-minute budget\" and add the file here — but do not \
                 leave the local bar silently not running it.",
                n + 1
            );
        }
    }
    assert!(gates > 0, "no `backend-llvm` gate found at all; this test is asserting nothing");
    eprintln!("llvm delta: {gates} gates, all inside the bar's feature leg");
}

// ---------------------------------------------------------------------------
// The grammar has one source
// ---------------------------------------------------------------------------

/// `editors/tree-sitter-buri/grammar.js` is generated from `grammar.ebnf`, and
/// this is what makes that true rather than aspirational: the file is
/// regenerated in memory and compared byte for byte with the one on disk.
///
/// It is checked in rather than built on demand because an editor installs the
/// grammar without the toolchain — Zed fetches the directory and runs
/// `tree-sitter generate` over it. A build product that ships has to be pinned
/// by something, and here that is this test.
///
/// After a deliberate change to the grammar:
///
/// ```text
/// BURI_BLESS=1 cargo test -p buri --test language corpus::the_tree_sitter_grammar
/// ```
///
/// Then run `editors/tree-sitter-buri/check.sh`, which is the half of the
/// guarantee that needs the tree-sitter CLI.
#[test]
fn the_tree_sitter_grammar_is_generated_from_the_ebnf() {
    let path = repo_root().join("editors/tree-sitter-buri/grammar.js");
    let generated = buri::documentation::grammar::generate(buri::documentation::topics::GRAMMAR)
        .unwrap_or_else(|e| panic!("cli/src/docs/grammar.ebnf does not generate:\n{e}"));

    if std::env::var_os("BURI_BLESS").is_some() {
        std::fs::write(&path, &generated).unwrap();
        eprintln!("editors/tree-sitter-buri/grammar.js: recorded {} bytes", generated.len());
        return;
    }
    let recorded = std::fs::read_to_string(&path).unwrap_or_default();
    if recorded == generated {
        return;
    }
    panic!(
        "editors/tree-sitter-buri/grammar.js is not what the EBNF generates.\n{}\n\
         Edit `cli/src/docs/grammar.ebnf`, never this file. Then record it and run\n\
         editors/tree-sitter-buri/check.sh:\n  \
         BURI_BLESS=1 cargo test -p buri --test language corpus::the_tree_sitter_grammar",
        first_differences(&recorded, &generated)
    );
}

/// A diff short enough to read. `harness::Golden` prints both files whole,
/// which is right for a recorded diagnostic and useless for a five-hundred-
/// line one.
fn first_differences(recorded: &str, generated: &str) -> String {
    let mut out = String::new();
    let want: Vec<&str> = recorded.lines().collect();
    let got: Vec<&str> = generated.lines().collect();
    for i in 0..want.len().max(got.len()) {
        let (a, b) = (want.get(i).copied(), got.get(i).copied());
        if a != b {
            out.push_str(&format!(
                "  line {}:\n    on disk:   {}\n    generated: {}\n",
                i + 1,
                a.unwrap_or("<end of file>"),
                b.unwrap_or("<end of file>")
            ));
            if out.lines().count() > 40 {
                out.push_str("  ...\n");
                break;
            }
        }
    }
    out
}

/// Every non-terminal the EBNF names is a production the EBNF declares.
///
/// Nothing used to execute the EBNF, so a renamed production could leave a
/// reference behind and the file would still read as though it meant
/// something. Generation would fail on it, but this says which name and where
/// rather than failing somewhere downstream.
#[test]
fn every_reference_in_the_ebnf_resolves() {
    let ebnf = buri::documentation::grammar::parse(buri::documentation::topics::GRAMMAR)
        .unwrap_or_else(|e| panic!("cli/src/docs/grammar.ebnf does not parse:\n{e}"));
    let dangling = buri::documentation::grammar::dangling_references(&ebnf);
    assert!(dangling.is_empty(), "cli/src/docs/grammar.ebnf:\n  {}", dangling.join("\n  "));
    assert!(
        ebnf.productions.len() > 80,
        "only {} productions were read; the file is not being parsed",
        ebnf.productions.len()
    );
}

// ---------------------------------------------------------------------------
// The removed backend leaves no dangling pointers
// ---------------------------------------------------------------------------

/// Every file under `dir` whose bytes are text this repository writes by hand
/// or records as a golden, for the tests that read the tree itself.
fn text_files(dir: &Path, out: &mut Vec<PathBuf>) {
    const KINDS: &[&str] =
        &["rs", "c", "h", "md", "toml", "buri", "proto", "json", "jsonl", "txt", "sh", "js"];
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::path);
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            // Not into what a build writes. `CARGO_TARGET_DIR` may name a
            // directory *inside* the checkout, and what is under it is output
            // and scratch trees rather than anybody's source — see
            // [`repository_files`], which is how the file-list question is asked
            // where the answer has to be exact.
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            text_files(&p, out);
        } else if p.extension().is_some_and(|x| KINDS.iter().any(|k| x == *k)) {
            out.push(p);
        }
    }
}

/// No file under `cli/` names the backend that was removed on 2026-08-29,
/// outside the allow-list below.
///
/// Its directory under `cli/src/compiler/backend/` and its document under
/// `design/native/` are both gone (`design/native/CODEGEN-STENCIL.md` §13).
/// Ninety-nine comments went on pointing at files inside them — a reader who
/// follows one opens nothing — and nothing held the count down, so it could
/// only grow. This is what holds it at the allow-list.
///
/// A comparison that has lost its subject is deleted; one that still has a
/// surviving twin points at the twin, under `stencil/` or `llvm/`; a fact
/// about the history is kept without the name. The history itself lives in
/// `design/`, which is outside this walk, and so is `reference/`, which cites
/// the upstream wasmtime tree rather than this one.
#[test]
fn the_removed_backend_is_not_cross_referenced() {
    /// Every file allowed to name it, and why.
    const ALLOWED: &[&str] = &[
        // The backend's name is a term of the codegen key, and the test that
        // pins that has to write down a name no toolchain answers to any more.
        "cli/src/build/actions.rs",
    ];

    // Spelled in two pieces so this file is not its own first hit.
    let needle = format!("crane{}", "lift");

    let mut files = Vec::new();
    text_files(&repo_root().join("cli"), &mut files);
    assert!(files.len() > 500, "found {} files under cli/; the walk is broken", files.len());

    let mut hits = 0;
    for path in &files {
        let rel = path.strip_prefix(repo_root()).unwrap_or(path).to_string_lossy().to_string();
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        for (n, line) in text.lines().enumerate() {
            if !line.to_ascii_lowercase().contains(&needle) {
                continue;
            }
            hits += 1;
            assert!(
                ALLOWED.contains(&rel.as_str()),
                "{rel}:{} names the removed backend, and that file is not on the \
                 allow-list {ALLOWED:?}. Say what is true now — the surviving \
                 counterpart under `stencil/` or `llvm/`, or the claim without the \
                 pointer — rather than citing a file the tree no longer has. \
                 `design/native/CODEGEN-STENCIL.md` §13 is where the history lives.",
                n + 1
            );
        }
    }
    eprintln!("removed-backend mentions under cli/: {hits}, all on the allow-list");
}
