//! **The whole conformance corpus, run on both backends, verdict by verdict.**
//!
//! `agreement.rs` compares one hand-written program per row of VALUE-MODEL.md
//! §12. `conformance.rs` runs the corpus natively and asks whether it exits
//! zero. `language/conformance.rs` runs the same corpus on JavaScript through
//! `buri test` and asks whether it passes. Nothing put the last two side by
//! side, so the claim "the two backends agree about the corpus" was assembled
//! by a reader out of two files rather than asserted by one.
//!
//! This file asserts it. For every package in `cli/tests/conformance/lib`:
//!
//! ```text
//! test source -> monomorphize(Roots::Tests) -> prepare(Js)     -> main.mjs  -> engine  -> [ {name, ok}, ... ]
//!             \-> monomorphize(Roots::Tests) -> prepare(native) -> objects -> cc -> a.out -> exit status + records
//! ```
//!
//! and the **per-block verdicts** are compared: the same number of blocks, in
//! the same order, each passing on both sides or failing on both sides with
//! the same message.
//!
//! # Why per-block and not "it exited zero"
//!
//! A `test` block is the smallest thing a corpus file says. A native run
//! answers with one bit for the whole file, because a failed assertion is an
//! abort (SPEC 6.9) and one process cannot report two failures — so the
//! runtime's resume protocol is what turns that bit into a list:
//! `BURI_TEST_FROM` names the block to start at, an aborting block writes one
//! JSON line naming its index and its message, and the runner starts again
//! after it. `commands/test.rs` is the product's user of that protocol and
//! this is a second one, so a corpus file that failed three blocks natively
//! and one on JavaScript is three disagreements here rather than one.
//!
//! # What it observes
//!
//! Only what a user's program does: the bytes each artifact wrote and the
//! status it exited with. Nothing in this file reads an IR, a pass, a plan or
//! a symbol — the point of a net around `middle::rc` is that it keeps holding
//! while the pass under it is rewritten, and a test that read the pass would
//! be the first thing to break.
//!
//! # Every package, not the native set
//!
//! `conformance.rs`'s `PACKAGES` is a ledger of what the *native set* is, and a
//! file outside it is one the backend refuses today. This walks the corpus
//! directory instead and asks each backend for itself, so a file that becomes
//! compilable joins this sweep with no edit here — and the count it prints is
//! how a reader knows the sweep did not quietly shrink.
//!
//! # What it costs
//!
//! Almost nothing, because the native half is `conformance::linked`, which is
//! memoized on the source: the corpus is compiled and linked once for this
//! binary however many suites walk it. What this adds is one JavaScript
//! emission and one engine start per file, and one more run of each native
//! binary.

use buri::build::actions;
use buri::build::buildfile::Platform;
use buri::compiler::backend::{self, Options, Profile, Target};
use buri::compiler::driver;
use buri::compiler::middle::monomorphize;
use buri::compiler::modules::Role;
use buri::diagnostics::{Diagnostics, SourceMap};
use std::path::{Path, PathBuf};
use std::process::Command;

/// What one `test` block did, as either backend reports it.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Verdict {
    Passed,
    /// The message the block ended with. Byte-identical on both backends is
    /// the claim — `agreement.rs`'s abort rows are the same claim about the
    /// abort paths, and this is it over every assertion in the corpus.
    Failed(String),
}

impl Verdict {
    fn shown(&self) -> String {
        match self {
            Verdict::Passed => String::from("passed"),
            Verdict::Failed(m) => format!("failed: {m}"),
        }
    }
}

/// One package's test source, as `<package>/<file>.buri`.
fn corpus_files() -> Vec<String> {
    let root = crate::shared::conformance_corpus();
    let mut out = Vec::new();
    let Ok(packages) = std::fs::read_dir(&root) else { return out };
    let mut packages: Vec<_> = packages.filter_map(Result::ok).collect();
    packages.sort_by_key(std::fs::DirEntry::file_name);
    for package in packages {
        let tests = package.path().join("test");
        let Ok(entries) = std::fs::read_dir(&tests) else { continue };
        let mut files: Vec<_> = entries.filter_map(Result::ok).collect();
        files.sort_by_key(std::fs::DirEntry::file_name);
        for file in files {
            let name = file.file_name().to_string_lossy().to_string();
            if !name.ends_with(".buri") {
                continue;
            }
            out.push(format!("{}/{name}", package.file_name().to_string_lossy()));
        }
    }
    out
}

fn read(path: &str) -> String {
    let (package, file) = path.split_once('/').unwrap_or((path, ""));
    let at = crate::shared::conformance_corpus().join(package).join("test").join(file);
    std::fs::read_to_string(&at).unwrap_or_else(|e| panic!("{}: {e}", at.display()))
}

fn workspace(name: &str) -> PathBuf {
    crate::sweep::once();
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("corpus-differential-{}", std::process::id()))
        .join(format!("{}-{n}", name.replace('/', "-")));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The block names and verdicts the **reference** backend reports.
///
/// The artifact the JavaScript backend emits for a `Roots::Tests` program
/// exposes `$run`, which answers an array of records, and prints nothing on
/// its own — `commands/test.rs` appends the one line that calls it, and so
/// does this. Both epilogues are the same line for the same reason: what is
/// being compared has to be what the product runs.
fn javascript(name: &str, source: &str) -> Result<Vec<(String, Verdict)>, String> {
    let mut map = SourceMap::new();
    let analysis = analyze(name, source, &mut map);
    if analysis.diagnostics.has_errors() {
        return Err(String::from("the front end refused it"));
    }
    let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diagnostics = Diagnostics::new();
    let mut program = monomorphize::run(
        &analysis.checked,
        paths,
        &mut diagnostics,
        monomorphize::Roots::Tests,
    );
    if diagnostics.has_errors() {
        return Err(String::from("monomorphization failed"));
    }
    let target = Target { platform: Platform::Js, arch: None };
    actions::prepare(&mut program, target);
    let opts = Options { profile: Profile::Debug, target, unit_prefix: "" };
    let mut backend =
        backend::select(target, Profile::Debug).map_err(|e| format!("no JavaScript backend: {e}"))?;
    let units = match backend.emit(&program, &analysis.checked.tables, &opts) {
        Ok(units) => units,
        Err(d) => {
            return Err(format!(
                "the JavaScript backend refused it: {}",
                d.items.iter().map(|i| i.message.clone()).collect::<Vec<_>>().join("; ")
            ))
        }
    };
    let dir = workspace(&format!("{name}-js"));
    let artifact = dir.join("test.mjs");
    let mut text = String::from_utf8_lossy(&units.first().expect("one unit").bytes).to_string();
    // The runner's own epilogue, spelled the way `commands/test.rs` spells it:
    // `$run` is `async` and this is module top level of an `.mjs`, where
    // `await` is available.
    text.push_str("\n$write(1,JSON.stringify(await $run(null)));\n");
    std::fs::write(&artifact, &text).unwrap();
    let engine = crate::shared::js_engine().ok_or("no JavaScript engine")?;
    let out = Command::new(&engine).arg(&artifact).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    if !out.status.success() {
        return Err(format!(
            "the reference artifact exited {:?}:\n{}\n{stdout}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(records(&stdout))
}

/// One conformance file, checked as a test source of its own package —
/// `conformance.rs::analyze`, which cannot be shared because it is behind that
/// module's feature gate and this one is behind the same gate for a different
/// backend.
fn analyze(path: &str, source: &str, map: &mut SourceMap) -> driver::Analysis {
    let repository = crate::shared::conformance_repository();
    let package = repository.and_then(|w| {
        let (package, _) = path.split_once('/')?;
        w.package_by_path(&format!("lib/{package}"))
    });
    let mut cache = buri::parsing::parser::Cache::new();
    driver::analyze_snippet_as(
        repository,
        package,
        map,
        &mut cache,
        "main",
        source,
        Role::TestSource,
    )
}

/// `[{"name":…,"ok":true}, …]` as verdicts, in order.
///
/// Read without a JSON library for `commands/test.rs::parse_results`' reason:
/// the shape is fixed and known, and a dependency for one array would be a
/// dependency in the toolchain's set.
fn records(text: &str) -> Vec<(String, Verdict)> {
    let mut out = Vec::new();
    for chunk in objects(text) {
        let name = field(&chunk, "name").unwrap_or_default();
        let verdict = if chunk.contains("\"ok\":true") {
            Verdict::Passed
        } else {
            Verdict::Failed(field(&chunk, "message").unwrap_or_default())
        };
        out.push((name, verdict));
    }
    out
}

/// Every top-level `{...}` in `text`, as text. Strings are skipped over, so a
/// brace inside a message does not split a record.
fn objects(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut depth, mut start, mut in_str) = (0i32, 0usize, false);
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while let Some(&b) = bytes.get(i) {
        match b {
            b'\\' if in_str => i += 1,
            b'"' => in_str = !in_str,
            b'{' if !in_str => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            b'}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    if let Some(object) = text.get(start..=i) {
                        out.push(object.to_string());
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

fn field(chunk: &str, name: &str) -> Option<String> {
    let key = format!("\"{name}\":\"");
    let rest = chunk.get(chunk.find(&key)? + key.len()..)?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => return Some(out),
            },
            other => out.push(other),
        }
    }
    Some(out)
}

/// The verdicts the **native** backend reports, one process per failing block.
///
/// `Ok(None)` where the native backend has no answer for this file — a missing
/// intrinsic or a refusal, which `conformance.rs`'s ledger already accounts
/// for and which is not a disagreement.
fn native(name: &str, source: &str, blocks: usize) -> Result<Option<Vec<Verdict>>, String> {
    let Some((binary, declared)) = crate::conformance::linked_if_supported(name, source) else {
        return Ok(None);
    };
    if declared != blocks {
        return Err(format!(
            "the two pipelines monomorphized a different number of `test` blocks: \
             JavaScript {blocks}, native {declared}"
        ));
    }
    let mut out = vec![Verdict::Passed; blocks];
    let mut from = 0i64;
    // One process per failing block, and never more than one per block: a
    // process either reports a failure and stops, or reaches the end.
    for _ in 0..=blocks {
        let mut cmd = Command::new(&binary);
        crate::shared::heap_checked(&mut cmd);
        cmd.env("BURI_TEST_FROM", from.to_string());
        let ran = crate::shared::ran_command(&mut cmd);
        // The heap check stops a program *after* its last block returned, so
        // it says nothing about a verdict. `conformance.rs` is what owns that
        // claim; here it means the run got to the end.
        if ran.status == 0 || ran.status == crate::shared::HEAP_CHECK_STATUS {
            return Ok(Some(out));
        }
        let record = objects(&ran.stdout);
        let Some(chunk) = record.last() else {
            return Err(format!(
                "the native binary exited {} and wrote no record:\nstdout:\n{}\nstderr:\n{}",
                ran.status, ran.stdout, ran.stderr
            ));
        };
        let at: usize = field_number(chunk, "i").ok_or_else(|| {
            format!("the native binary's record names no block: {chunk}")
        })? as usize;
        let message = field(chunk, "message").unwrap_or_default();
        let slot = out.get_mut(at).ok_or_else(|| {
            format!("the native binary reported block {at} of {blocks}")
        })?;
        *slot = Verdict::Failed(message);
        from = at as i64 + 1;
    }
    Err(String::from("the native binary reported more failures than it has blocks"))
}

fn field_number(chunk: &str, name: &str) -> Option<i64> {
    let key = format!("\"{name}\":");
    let rest = chunk.get(chunk.find(&key)? + key.len()..)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// **Every corpus package, both backends, verdict by verdict.**
#[test]
fn every_corpus_package_agrees_with_the_reference_backend() {
    if let Some(why) = crate::conformance::skip_reason() {
        crate::ci::skipped("corpus differential", &why);
        return;
    }
    if crate::shared::js_engine().is_none() {
        crate::ci::skipped("corpus differential", "no JavaScript engine on PATH");
        return;
    }
    let mut compared = 0usize;
    let mut blocks = 0usize;
    let mut skipped: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for path in corpus_files() {
        let source = read(&path);
        let reference = match javascript(&path, &source) {
            Ok(v) => v,
            // A file the *reference* pipeline cannot answer for is not a
            // disagreement: the corpus is shared with the JavaScript suite and
            // may be mid-change, and that suite is where a refusal is a
            // failure.
            Err(why) => {
                skipped.push(format!("{path} (javascript: {why})"));
                continue;
            }
        };
        let native = match native(&path, &source, reference.len()) {
            Ok(Some(v)) => v,
            Ok(None) => {
                skipped.push(format!("{path} (the native backend has no answer for it)"));
                continue;
            }
            Err(why) => {
                failures.push(format!("`{path}`: {why}"));
                continue;
            }
        };
        for (i, ((name, want), got)) in reference.iter().zip(native.iter()).enumerate() {
            if want != got {
                failures.push(format!(
                    "`{path}` block {i} (`{name}`) disagrees:\n  javascript: {}\n  native:     {}",
                    want.shown(),
                    got.shown()
                ));
            }
        }
        compared += 1;
        blocks += reference.len();
    }
    for s in &skipped {
        eprintln!("corpus differential: skipped {s}");
    }
    assert!(
        failures.is_empty(),
        "{} disagreement(s) between the backends:\n{}",
        failures.len(),
        failures.join("\n")
    );
    // A sweep that compared nothing passes, so what it covered is asserted as
    // well as printed. The floor is the size of the native set on the day this
    // was written, less a little slack for a file the corpus may take out.
    eprintln!("corpus differential: {compared} files, {blocks} test blocks, 0 disagreements");
    assert!(
        compared >= 40,
        "the sweep only compared {compared} files, which is fewer than the corpus has"
    );
}
