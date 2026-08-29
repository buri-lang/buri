//! **Scoped analysis answers what whole-closure analysis answers**, for the
//! file it was asked about.
//!
//! `driver::analyze_bodies_in` type-checks the bodies of one file and leaves
//! the rest of the closure's step 5 undone, which is what makes an editor query
//! cost the file under the cursor. Everything reading the result — hover,
//! definition, completion, the tokens, the hints, the colours — filters bodies
//! by file id already, so the claim that has to hold is narrow and exact:
//!
//!   * every body written in the chosen file is present, and renders
//!     identically to the one a whole-closure run produced;
//!   * so does every module-level `let` written in it;
//!   * and the diagnostics carrying a span in it are the same diagnostics, in
//!     the same order.
//!
//! Run over every repository the language-server goldens are recorded from,
//! plus the worked monorepo, one file at a time. A golden that changed after a
//! server switched to the scoped path would be this test failing first, and
//! that is the point of it: the goldens are downstream of this guarantee.
//!
//! One id is deliberately *not* compared: `CtxTypeId` is minted as context
//! literals are met, so it counts how many bodies were checked before this one.
//! It is renumbered by first appearance below. Nothing renders it — a context
//! shows as `a context` (`types::show`) — and nothing compares two of them
//! across bodies, so the number is an allocation order and not a fact about the
//! program.
//!
//! ```text
//! cargo test -p buri --test language scoped_bodies::
//! ```
use buri::build::session;
use buri::commands::arguments::Flags;
use buri::compiler::driver::{self, Analysis};
use buri::compiler::modules::Unit;
use buri::diagnostics::FileId;

use std::path::{Path, PathBuf};

fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

/// Every repository the language server is tested against, and the worked
/// monorepo — the largest body of Buri here, and the only one with a `ui/style`
/// closure spanning several files.
fn repositories() -> Vec<PathBuf> {
    let mut out = vec![tests_dir().join("example")];
    let lsp = tests_dir().join("repositories/lsp");
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&lsp)
        .expect("the lsp corpus")
        .filter_map(Result::ok)
        .map(|e| e.path().join("repo"))
        .filter(|p| p.join("REPO.buri").is_file())
        .collect();
    cases.sort();
    out.extend(cases);
    out
}

#[test]
fn a_scoped_analysis_answers_what_a_whole_closure_one_answers() {
    let mut repositories_checked = 0usize;
    let mut files_checked = 0usize;
    for root in repositories() {
        let Ok(mut open) = session::open_at(&root, &Flags::default()) else { continue };
        // A repository whose own build files do not read compiles nothing, and
        // the server says so rather than analysing it.
        if open.diagnostics.has_errors() {
            continue;
        }
        repositories_checked = repositories_checked.saturating_add(1);
        for target in open.workspace.targets() {
            let unit = Unit { target: Some(target), platform: None, with_tests: true };
            let full = driver::analyze(
                Some(&open.workspace),
                &mut open.map,
                &mut open.parsed,
                &unit,
            );
            // The repository's own sources, not the standard library's: those
            // are what an editor opens, and their file ids are stable across
            // analyses because the source map is reused.
            let files: Vec<FileId> = full
                .loaded
                .modules
                .iter()
                .filter(|m| m.disk.as_ref().is_some_and(|d| d.starts_with(&root)))
                .map(|m| m.file)
                .collect();
            for file in files {
                let scoped = driver::analyze_bodies_in(
                    Some(&open.workspace),
                    &mut open.map,
                    &mut open.parsed,
                    &unit,
                    &[file],
                );
                let name = open.map.name(file).to_string();
                let label = format!("{} ({name})", root.display());
                compare(&label, file, &full, &scoped);
                files_checked = files_checked.saturating_add(1);
            }
        }
    }
    assert!(
        repositories_checked > 60,
        "expected the lsp corpus and the worked monorepo, analysed {repositories_checked}"
    );
    assert!(files_checked > 150, "only {files_checked} files were compared");
}

/// What the two analyses must agree on for one file.
fn compare(label: &str, file: FileId, full: &Analysis, scoped: &Analysis) {
    let want = bodies_in(full, file);
    let got = bodies_in(scoped, file);
    let missing: Vec<&String> = want.iter().map(|(k, _)| k).filter(|k| !has(&got, k)).collect();
    assert!(missing.is_empty(), "{label}: the scoped analysis is missing {missing:?}");
    let extra: Vec<&String> = got.iter().map(|(k, _)| k).filter(|k| !has(&want, k)).collect();
    assert!(extra.is_empty(), "{label}: the scoped analysis invented {extra:?}");
    for (name, rendering) in &want {
        let Some(theirs) = got.iter().find(|(k, _)| k == name).map(|(_, v)| v) else { continue };
        assert!(
            theirs == rendering,
            "{label}: `{name}` differs{}",
            first_difference(rendering, theirs)
        );
    }

    let want = findings_in(full, file);
    let got = findings_in(scoped, file);
    assert_eq!(got, want, "{label}: the diagnostics differ");
}

fn has(list: &[(String, String)], key: &str) -> bool {
    list.iter().any(|(k, _)| k == key)
}

/// Every body and every module-level `let` written in one file, named by its
/// declaration and rendered, sorted so the comparison does not depend on a
/// hash map's order.
fn bodies_in(analysis: &Analysis, file: FileId) -> Vec<(String, String)> {
    let tables = &analysis.checked.tables;
    let mut out = Vec::new();
    for (id, body) in &analysis.checked.bodies {
        let info = tables.fn_info(*id);
        if info.span.file == file {
            out.push((format!("fn {} @{}", info.name, info.span.start), render(&format!("{body:?}"))));
        }
    }
    for (id, expr) in &analysis.checked.consts {
        let info = tables.const_(*id);
        if info.span.file == file {
            out.push((format!("let {} @{}", info.name, info.span.start), render(&format!("{expr:?}"))));
        }
    }
    out.sort();
    out
}

/// Everything either analysis has to say about one file.
fn findings_in(analysis: &Analysis, file: FileId) -> Vec<String> {
    analysis
        .diagnostics
        .items
        .iter()
        .filter(|d| d.span.file == file)
        .map(|d| format!("{}..{} {:?} {}", d.span.start, d.span.end, d.severity, d.message))
        .collect()
}

/// The debug rendering, with every `CtxTypeId` renumbered by first appearance.
///
/// The two analyses check different numbers of bodies before this one, so the
/// counter these ids come off stands somewhere else. The structure around them
/// is identical either way, so first appearance is a bijection between the two
/// numberings — and if it is not, the strings differ and the test says so.
fn render(debug: &str) -> String {
    const MARK: &str = "CtxTypeId(";
    let mut out = String::with_capacity(debug.len());
    let mut names: Vec<&str> = Vec::new();
    let mut rest = debug;
    while let Some(at) = rest.find(MARK) {
        let (before, after) = rest.split_at(at.saturating_add(MARK.len()));
        out.push_str(before);
        let end = after.find(')').unwrap_or(after.len());
        let (number, tail) = after.split_at(end);
        let seen = match names.iter().position(|n| *n == number) {
            Some(i) => i,
            None => {
                names.push(number);
                names.len().saturating_sub(1)
            }
        };
        out.push_str(&format!("#{seen}"));
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// Where two renderings part company, with a little either side.
///
/// The whole of a checked body is tens of kilobytes of `Debug`, and a failure
/// that printed two of them would bury the one construct that moved.
fn first_difference(want: &str, got: &str) -> String {
    let at = want
        .bytes()
        .zip(got.bytes())
        .position(|(a, b)| a != b)
        .unwrap_or(want.len().min(got.len()));
    let from = at.saturating_sub(120);
    let window = |s: &str| {
        let to = at.saturating_add(240).min(s.len());
        let from = from.min(s.len());
        s.get(from..to).unwrap_or(s).to_string()
    };
    format!(", at byte {at}\n  whole closure: ...{}\n         scoped: ...{}", window(want), window(got))
}

