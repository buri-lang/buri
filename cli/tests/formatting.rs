//! What `buri format` prints, one case per decision it makes.
//!
//! Each case is a directory with two files. `input.buri` is source somebody
//! might have typed — usually badly laid out, sometimes already right — and
//! `expected.buri` is the one output the formatter is allowed to produce for
//! it. There is no third file, because there is nothing to configure: a
//! formatter with options has no single right answer, and this suite exists to
//! say that this one does.
//!
//! A case whose name begins with `textproto_` is a **build file** rather than
//! source — `BUILD.buri` and `REPO.buri` go through the other of the two
//! printers `buri format` has. Its `input.buri` is still called that, because
//! a case is two files with fixed names; the case's own name is what says which
//! dialect it is in, which is also the first thing a reader of the directory
//! listing wants to know.
//!
//! Four claims per case, each its own test so that a failure names which of
//! them broke:
//!
//! 1. `format(input) == expected`, byte for byte.
//! 2. `format(expected) == expected`. A shape that moves when it is formatted
//!    again is a *formatter* bug and is reported as one, rather than as an
//!    expectation somebody got wrong.
//! 3. Every comment in the input is in the output, as a set — the leading
//!    import run is sorted, and a comment travels with the import it was
//!    written above, so the sequence may legally change and the set may not.
//! 4. Every token in the input is in the output, modulo the five things
//!    layout is allowed to change: a redundant parenthesis, an optional
//!    trailing comma, a block put around an expression body that will not fit
//!    beside its `=>`, the order of the leading import run and of the names
//!    inside a clause, and an empty type-argument list — `t<>` prints as `t`,
//!    because they are the same type and only one of them is how a reader
//!    writes it.
//!
//! ```text
//! BURI_BLESS=1 cargo test -p buri --test formatting    # record expected.buri
//! ```
//!
//! Blessing rewrites `expected.buri` and never `input.buri`, so the question a
//! case asks is fixed and only the answer is recorded. Read the diff: blessing
//! without reading it is the one way this suite proves nothing.
//!
//! This tree is deliberately not part of the repository-wide corpus. An
//! `input.buri` is misformatted on purpose, so a suite that asked "is every
//! source in the repository already formatted" would be asking these files a
//! question they exist to answer no to. `language/corpus.rs` reaches its files through
//! two explicit lists of directories rather than by walking `cli/tests`, and
//! neither names this one — which is also why the guards those suites apply to
//! the corpus are applied here, per case, rather than assumed.

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
              toolchain — that no input panics it — and a harness that drives \
              the toolchain is not the toolchain. A test that unwraps fails on \
              the line that broke, which is what a test is for, and threading \
              `?` through an assertion buys nothing. `clippy.toml` exempts \
              `#[test]` functions already; this covers the helpers around them."
)]
mod harness;

use buri::build::textproto;
use buri::diagnostics::{FileId, SourceMap};
use buri::formatting::{comment_shape, source, token_shape, Shape};
use harness::{case_dirs, tests_dir, Golden};
use std::path::{Path, PathBuf};

/// The floor stops a mis-set path from turning the whole suite into a pass.
const FLOOR: usize = 60;

fn cases() -> Vec<PathBuf> {
    case_dirs(&tests_dir().join("formatting"), "input.buri", FLOOR)
}

fn name(dir: &Path) -> String {
    dir.file_name().unwrap().to_string_lossy().to_string()
}

/// Which of the two things `buri format` formats a case is about.
///
/// The command decides by file name — `BUILD.buri` and `REPO.buri` are
/// textproto, everything else is source — and a case cannot say it that way,
/// because every case's input is `input.buri`. So the case's own name says it,
/// which is also the first thing a reader of the directory listing wants to
/// know.
fn is_textproto(case: &str) -> bool {
    case.starts_with("textproto_")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{} could not be read: {e}", path.display()))
}

/// The formatter's output for one file, with every reason it might not have
/// one spelled out.
///
/// A case whose input does not parse is a mistake in the case rather than a
/// finding about the formatter, and it is worth saying so: `source` returns
/// `None` for that and for output it cannot vouch for, and those two want very
/// different fixes.
fn formatted(label: &str, path: &Path, text: &str) -> String {
    if is_textproto(label.split('/').next().unwrap_or_default()) {
        let parsed = textproto::parse(text, FileId(0));
        assert!(
            parsed.errors.is_empty(),
            "{label} does not read, so there is nothing to format.\n\
             A case's input must be a valid build file — badly laid out, not \
             broken: {:?}",
            parsed.errors.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
        );
        return textproto::print(&parsed.document);
    }
    let mut map = SourceMap::new();
    let id = map.add(label.to_string(), path.to_path_buf(), text.to_string());
    let parsed = buri::parsing::parser::parse(text, id);
    if !parsed.errors.is_empty() {
        let mut out = String::new();
        for e in &parsed.errors {
            out.push_str(&map.render(e, false));
        }
        panic!(
            "{label} does not parse, so there is nothing to format.\n\
             A case's input must be valid Buri — badly laid out, not broken:\n{out}"
        );
    }
    source(text).unwrap_or_else(|| {
        panic!(
            "{label} parses, but `format::source` refused its own output for it.\n\
             That means the output did not parse or lost a comment — a formatter \
             bug, and one this case has caught."
        )
    })
}

/// `token_shape` with the edits layout is allowed to make taken out.
///
/// A trailing comma is the formatter's to add and to drop, and so is a
/// parenthesis it can prove redundant. Braces go too, because a lambda body
/// and a match arm body that will not fit beside their `=>` are wrapped in the
/// block form the grammar already has — `e` and `{ e }` are the same
/// expression, and choosing between them is layout. Order goes because the
/// leading import run is sorted and so are the names inside a clause. An empty
/// type-argument list goes because `t<>` and `t` are the same type, and the
/// formatter prints the one a reader would write.
///
/// What is left is the claim that no name, keyword, operator or literal was
/// invented or lost.
fn tokens(text: &str) -> Vec<Shape> {
    const LAYOUT: &[&str] = &["`,`", "`(`", "`)`", "`{`", "`}`"];
    let mut out: Vec<Shape> = drop_empty_type_arguments(token_shape(text))
        .into_iter()
        .filter(|s| !matches!(s, Shape::Token(t) if LAYOUT.contains(&t.as_str())))
        .collect();
    out.sort();
    out
}

/// Every `<` immediately followed by `>` removed, in source order.
///
/// The pair goes as a pair rather than by filtering both tokens everywhere: a
/// generic list the formatter really did lose would then be invisible, which is
/// the opposite of what this comparison is for.
fn drop_empty_type_arguments(shapes: Vec<Shape>) -> Vec<Shape> {
    let mut out: Vec<Shape> = Vec::with_capacity(shapes.len());
    for s in shapes {
        if matches!(&s, Shape::Token(t) if t == "`>`")
            && matches!(out.last(), Some(Shape::Token(t)) if t == "`<`")
        {
            out.pop();
            continue;
        }
        out.push(s);
    }
    out
}

fn comments(text: &str) -> Vec<Shape> {
    let mut out = comment_shape(text);
    out.sort();
    out
}

/// The output the case pins. When blessing, that is whatever the formatter
/// prints right now; otherwise it is what is on disk, so a hand-edited
/// `expected.buri` is held to the same standard as a recorded one.
fn expected(dir: &Path, from_input: &str) -> String {
    let path = dir.join("expected.buri");
    if std::env::var_os("BURI_BLESS").is_some() || !path.is_file() {
        return from_input.to_string();
    }
    read(&path)
}

#[test]
fn formatting_produces_the_expected_output() {
    let mut g = Golden::new();
    let cases = cases();
    for dir in &cases {
        let case = name(dir);
        let input = dir.join("input.buri");
        let out = formatted(&format!("{case}/input.buri"), &input, &read(&input));
        g.check(&dir.join("expected.buri"), &format!("{case}/expected.buri"), &out);
    }
    g.finish("formatting", cases.len());
}

/// Formatting an output again must not move it.
///
/// Its own test, because a fixed point that is not one is a different fault
/// from a wrong answer: the answer above may be exactly what somebody wanted
/// and still be a shape the formatter cannot reproduce from itself, which
/// would make `buri format` and `buri format --check` disagree forever.
#[test]
fn formatting_every_expected_output_is_stable() {
    let cases = cases();
    let mut failures = Vec::new();
    for dir in &cases {
        let case = name(dir);
        let input = dir.join("input.buri");
        let once = expected(dir, &formatted(&format!("{case}/input.buri"), &input, &read(&input)));
        let twice = formatted(&format!("{case}/expected.buri"), &dir.join("expected.buri"), &once);
        if twice != once {
            failures.push(format!(
                "{case}: formatting the output again changes it, so the shape is not a \
                 fixed point. This is a formatter bug, not a wrong expectation.\n\
                 once:\n{}\ntwice:\n{}",
                harness::indent(&once),
                harness::indent(&twice)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} unstable case(s):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    eprintln!("formatting: {} outputs are fixed points", cases.len());
}

/// Nothing a case was written with is missing from what it prints.
/// Every `#` comment in a build file, sorted. A comment travels with the field
/// beneath it and the fields are reordered, so the set is what survives.
fn proto_comments(document: &textproto::Document) -> Vec<String> {
    fn walk(fields: &[textproto::Field], out: &mut Vec<String>) {
        for f in fields {
            out.extend(f.comments.iter().cloned());
            if let textproto::Value::Message(m, _) = &f.value {
                out.extend(m.trailing.iter().cloned());
                walk(&m.fields, out);
            }
        }
    }
    let mut out = document.trailing.clone();
    walk(&document.fields, &mut out);
    out.sort();
    out
}

/// Every leaf of a build file as `path = value`, sorted.
///
/// The textproto answer to `tokens`: reordering the fields is the whole point,
/// so the *set* of what the file says is what must not change. It catches a
/// field dropped, invented, renamed or given a different value.
fn proto_leaves(document: &textproto::Document) -> Vec<String> {
    fn value(v: &textproto::Value) -> String {
        match v {
            textproto::Value::Str(s, _) => format!("{s:?}"),
            textproto::Value::Int(n, _) => n.to_string(),
            textproto::Value::Ident(s, _) => s.clone(),
            textproto::Value::Message(..) => "{}".to_string(),
            textproto::Value::List(items, _) => {
                items.iter().map(value).collect::<Vec<_>>().join(",")
            }
        }
    }
    fn walk(fields: &[textproto::Field], path: &str, out: &mut Vec<String>) {
        for f in fields {
            let here = format!("{path}/{}", f.name);
            match &f.value {
                textproto::Value::Message(m, _) => walk(&m.fields, &here, out),
                v => out.push(format!("{here} = {}", value(v))),
            }
        }
    }
    let mut out = Vec::new();
    walk(&document.fields, "", &mut out);
    out.sort();
    out
}

#[test]
fn formatting_keeps_every_case_whole() {
    let cases = cases();
    let mut failures = Vec::new();
    for dir in &cases {
        let case = name(dir);
        let input_path = dir.join("input.buri");
        let input = read(&input_path);
        let once = expected(dir, &formatted(&format!("{case}/input.buri"), &input_path, &input));

        if is_textproto(&case) {
            let (before, after) =
                (textproto::parse(&input, FileId(0)), textproto::parse(&once, FileId(0)));
            for e in &after.errors {
                failures.push(format!("{case}: the output does not read: {}", e.message));
            }
            if proto_comments(&before.document) != proto_comments(&after.document) {
                failures.push(format!(
                    "{case}: the comments are not the same set before and after.\n                       before: {:?}\n  after:  {:?}",
                    proto_comments(&before.document),
                    proto_comments(&after.document)
                ));
            }
            if proto_leaves(&before.document) != proto_leaves(&after.document) {
                failures.push(format!(
                    "{case}: a field was invented, lost or changed. Layout may put the \
                     fields in the schema's order; it may not do this.\n                       before: {:?}\n  after:  {:?}",
                    proto_leaves(&before.document),
                    proto_leaves(&after.document)
                ));
            }
            continue;
        }

        // The output parses. `source` already refuses output that does not,
        // so this is here to say what would have gone wrong if it had.
        let mut map = SourceMap::new();
        let id = map.add(format!("{case}/expected.buri"), dir.join("expected.buri"), once.clone());
        let parsed = buri::parsing::parser::parse(&once, id);
        for e in &parsed.errors {
            failures.push(format!("{case}: the output does not parse:\n{}", map.render(e, false)));
        }

        if comments(&input) != comments(&once) {
            failures.push(format!(
                "{case}: the comments are not the same set before and after.\n  \
                 before: {:?}\n  after:  {:?}",
                comments(&input),
                comments(&once)
            ));
        }
        if tokens(&input) != tokens(&once) {
            failures.push(format!(
                "{case}: a token was invented or lost. Layout may drop a redundant \
                 parenthesis, add or drop a trailing comma, drop an empty \
                 type-argument list, and reorder the leading import run; it may not \
                 do this.\n  before: {:?}\n  after:  {:?}",
                tokens(&input),
                tokens(&once)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} broken case(s):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    eprintln!("formatting: {} cases keep their comments and tokens", cases.len());
}

/// Every line is inside the margin, except where a case is named for the fact
/// that it cannot be.
///
/// `width_*` is the exception and says so in its name: a string literal and a
/// comment are the author's text, and there is no shape a formatter can give a
/// ninety-column atom. Everything else that leaves this formatter fits.
#[test]
fn formatting_stays_inside_the_margin() {
    const WIDTH: usize = 88;
    let mut failures = Vec::new();
    for dir in &cases() {
        let case = name(dir);
        if case.starts_with("width_") {
            continue;
        }
        let input = dir.join("input.buri");
        let once = expected(dir, &formatted(&format!("{case}/input.buri"), &input, &read(&input)));
        for line in once.lines() {
            if line.chars().count() > WIDTH {
                failures.push(format!(
                    "{case}: a line is {} columns:\n{line}",
                    line.chars().count()
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} line(s) over the margin:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// Every build file checked into the repository is already what `buri format`
/// prints.
///
/// The paired cases above say what the printer does; this says the repository
/// agrees with it. Without it a build file drifts silently — nothing reads
/// `BUILD.buri` for its layout — until somebody runs `format --check` in CI and
/// finds two hundred files to review at once.
#[test]
fn every_checked_in_build_file_is_formatted() {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.filter_map(Result::ok) {
            let p = e.path();
            let n = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            if n.starts_with('.') || n == "target" || n == "node_modules" {
                continue;
            }
            if p.is_dir() {
                walk(&p, out);
            } else if n == "BUILD.buri" || n == "REPO.buri" {
                out.push(p);
            }
        }
    }

    let root = tests_dir().parent().unwrap().parent().unwrap().to_path_buf();
    let mut files = Vec::new();
    walk(&root.join("cli/tests"), &mut files);
    walk(&root.join("cli/src"), &mut files);
    files.sort();
    assert!(files.len() > 50, "expected the fixture repositories, found {}", files.len());

    // The two the suite keeps deliberately misformatted, each as the question
    // its case asks: `format --check` has to have something to report.
    let deliberate = ["cli/tests/repositories/cli/format_check/repo/cmd/f/BUILD.buri"];

    let mut stale = Vec::new();
    for path in &files {
        let rel = path.strip_prefix(&root).unwrap().display().to_string();
        if deliberate.contains(&rel.as_str()) {
            continue;
        }
        let text = read(path);
        // A fixture carrying `{{…}}` placeholders is a harness template — the
        // case runner substitutes host-dependent facts before the file is ever
        // read as textproto (see harness/case.rs) — so the formatted claim is
        // about what the harness writes, not these bytes.
        if text.contains("{{") {
            continue;
        }
        let parsed = textproto::parse(&text, FileId(0));
        assert!(parsed.errors.is_empty(), "{rel} does not read");
        if textproto::print(&parsed.document) != text {
            stale.push(rel);
        }
    }
    assert!(
        stale.is_empty(),
        "{} build file(s) are not formatted. `buri format` fixes every one of \
         them:\n  {}",
        stale.len(),
        stale.join("\n  ")
    );
}

/// A case is two files and no more. A stray third file is either a leftover or
/// somebody reaching for an option, and neither should sit here unremarked.
#[test]
fn formatting_cases_are_two_files() {
    for dir in &cases() {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "NOTES.md")
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec!["expected.buri".to_string(), "input.buri".to_string()],
            "{} holds something other than the two files a case is made of \
             (a `NOTES.md` explaining a shape is allowed)",
            name(dir)
        );
    }
}
