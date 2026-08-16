//! Repository cases: a whole repository, what the CLI does in it, and what
//! that prints.
//!
//! The `reject/` corpus builds every case as a single-package binary with no
//! dependencies, so no build-graph diagnostic can be expressed in it — which
//! is most of what the build system checks. A case here is instead a small
//! repository checked in whole, with a manifest naming the commands to run
//! against it and the exit code each must produce.
//!
//! ```text
//! cli/tests/repositories/build-files/missing_dep/
//!   CASE.textproto      the manifest
//!   repo/               the repository, copied into scratch verbatim
//!     REPO.buri
//!     cmd/app/{BUILD.buri,main.buri}
//!     lib/money/{BUILD.buri,lib.buri}
//!   expected/lint.txt   what `lint` prints, recorded
//! ```
//!
//! The manifest is textproto, read by the toolchain's own parser — every
//! declarative file in this repository already is one, and a second config
//! dialect would be a second thing to maintain. It is *not* named `.buri`:
//! only the tree under `repo/` is a Buri repository, and a manifest the
//! toolchain never reads should not wear the extension that says it does.
//!
//! ```text
//! doc:  "one line saying what the case is about"
//! run  { args: [...]  exit: 1  golden: "lint.txt"  stream: ALL  stdin: "session.jsonl" }
//! run  { args: ["build"]  exit: 0  cwd: "lib/money" }
//! edit { file: "cmd/app/BUILD.buri"  replace: "..."  with: "..." }
//! file { path: "cmd/f/main.buri"  golden: "formatted.buri" }
//! file { path: ".buri/out/js/cmd/app/app.mjs"  absent: "a marker" }
//! path { path: ".buri/out"  exists: false }
//! path { path: "out"  symlink: ".buri/out/js" }
//! ```
//!
//! `run { cwd }` is how the no-argument forms are covered: CLI.md says a
//! command with no target operates on the package containing the working
//! directory, and there is no way to ask that question from the root.
//!
//! `path` is for the commands whose contract is about what they leave on
//! disk. `buri clean --outputs` and the `out/` symlink print almost nothing,
//! and what they print is not the claim — an exit code cannot tell a cache
//! that survived from one that was deleted and rebuilt. Exactly one
//! expectation per step, spelled out: an assertion is never inferred.
//!
//! `file { contains }` and `file { absent }` are for the files a whole-file
//! golden would be the wrong record of — a build artifact is mostly runtime,
//! so recording one would move on every unrelated backend change and would
//! bury the claim. Neither is ever blessed.
//!
//! `exit` is hand-written and required, and `golden` names a file that
//! `BURI_BLESS=1` records. That split is deliberate: blessing can rewrite what
//! a diagnostic *says*, but it can never quietly turn a rejection into an
//! acceptance.
//!
//! Steps run in the order they are written, against one scratch copy, so a
//! case can show a rule firing and then show that the fix the diagnostic
//! printed actually works. **Every case that must stay clean ends with the
//! edit that makes it fire** — a positive result is only evidence when its
//! negative twin sits next to it and cannot drift away.

use std::path::{Path, PathBuf};

use super::{indent, run_in, Golden, Scratch};

use buri::build::textproto::{self, Msg, Value};
use buri::diagnostics::FileId;

#[derive(Clone, Copy, PartialEq)]
pub enum Stream {
    All,
    Out,
    Err,
}

pub enum Step {
    Run {
        args: Vec<String>,
        exit: i32,
        golden: Option<String>,
        stream: Stream,
        /// A file in the case directory holding one JSON-RPC request per line.
        /// The harness adds the `Content-Length` framing, so the fixture stays
        /// something a person can read and diff.
        stdin: Option<String>,
        /// A directory inside the repository to run from, for the forms that
        /// take no target and mean "the package I am standing in".
        cwd: Option<String>,
    },
    Edit {
        file: String,
        from: String,
        to: String,
    },
    File {
        path: String,
        golden: Option<String>,
        /// Text that must appear in the file, and text that must not.
        ///
        /// A whole-file golden is the right record of something a person
        /// reads. A build *artifact* is not: it is mostly runtime, so pinning
        /// it would move on every unrelated backend change and say nothing
        /// about the claim. A claim about an artifact is stated instead as the
        /// one string that settles it.
        contains: Option<String>,
        absent: Option<String>,
    },
    /// What a command left on disk, as opposed to what it printed.
    Path {
        path: String,
        expect: PathExpectation,
    },
}

pub enum PathExpectation {
    Exists(bool),
    /// A symlink, and where it points. The link's own target is read rather
    /// than followed: `out -> .buri/out/js` is the claim, and a resolved path
    /// would pass just as well for a copied directory.
    Symlink(String),
}

pub struct Case {
    pub name: String,
    pub dir: PathBuf,
    pub doc: String,
    pub steps: Vec<Step>,
}

// ---------------------------------------------------------------------------
// Reading a manifest
// ---------------------------------------------------------------------------

/// A malformed manifest panics naming the case. It is a mistake in the test
/// rather than a finding about the product, so there is nothing to collect and
/// carry on from.
pub fn load_case(dir: &Path) -> Case {
    let name = dir.file_name().unwrap().to_string_lossy().to_string();
    let path = dir.join("CASE.textproto");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{name}: cannot read {}: {e}", path.display()));
    let parsed = textproto::parse(&text, FileId(0));
    assert!(
        parsed.errors.is_empty(),
        "{name}: CASE.textproto does not parse ({} error(s)); the first is at byte {}",
        parsed.errors.len(),
        parsed.errors[0].span.start
    );

    let mut doc = None;
    let mut steps = Vec::new();
    for field in &parsed.doc.fields {
        match field.name.as_str() {
            "doc" => doc = Some(as_str(&name, "doc", &field.value)),
            "run" => {
                let m = as_msg(&name, "run", &field.value);
                steps.push(Step::Run {
                    args: str_list(&name, "run.args", &m),
                    exit: req_int(&name, "run", "exit", &m),
                    golden: opt_str(&name, "run.golden", &m),
                    stdin: opt_str(&name, "run.stdin", &m),
                    cwd: opt_str(&name, "run.cwd", &m),
                    stream: match opt_ident(&name, "run.stream", &m).as_deref() {
                        None | Some("ALL") => Stream::All,
                        Some("OUT") => Stream::Out,
                        Some("ERR") => Stream::Err,
                        Some(other) => {
                            panic!("{name}: run.stream is {other}, not one of ALL, OUT, ERR")
                        }
                    },
                });
            }
            "edit" => {
                let m = as_msg(&name, "edit", &field.value);
                steps.push(Step::Edit {
                    file: req_str(&name, "edit", "file", &m),
                    from: req_str(&name, "edit", "replace", &m),
                    to: req_str(&name, "edit", "with", &m),
                });
            }
            "file" => {
                let m = as_msg(&name, "file", &field.value);
                let step = Step::File {
                    path: req_str(&name, "file", "path", &m),
                    golden: opt_str(&name, "file.golden", &m),
                    contains: opt_str(&name, "file.contains", &m),
                    absent: opt_str(&name, "file.absent", &m),
                };
                if let Step::File { golden: None, contains: None, absent: None, .. } = step {
                    panic!(
                        "{name}: a `file` step says nothing about the file; give it `golden`, \
                         `contains`, or `absent`"
                    );
                }
                steps.push(step);
            }
            "path" => {
                let m = as_msg(&name, "path", &field.value);
                let symlink = opt_str(&name, "path.symlink", &m);
                let exists = opt_ident(&name, "path.exists", &m).map(|v| match v.as_str() {
                    "true" => true,
                    "false" => false,
                    other => panic!("{name}: path.exists is {other}, not true or false"),
                });
                steps.push(Step::Path {
                    path: req_str(&name, "path", "path", &m),
                    expect: match (exists, symlink) {
                        (Some(_), Some(_)) => panic!(
                            "{name}: a `path` step says both `exists` and `symlink`; one claim each"
                        ),
                        (Some(b), None) => PathExpectation::Exists(b),
                        (None, Some(t)) => PathExpectation::Symlink(t),
                        (None, None) => panic!(
                            "{name}: a `path` step claims nothing — write `exists: false` or `symlink: \"...\"`"
                        ),
                    },
                });
            }
            other => panic!(
                "{name}: CASE.textproto has no field `{other}`; the forms are doc, run, edit, file, path"
            ),
        }
    }

    let doc = doc.unwrap_or_else(|| panic!("{name}: CASE.textproto has no `doc` saying what it is about"));
    assert!(
        !steps.is_empty(),
        "{name}: CASE.textproto runs nothing, so it proves nothing"
    );
    assert!(
        dir.join("repo/REPO.buri").is_file(),
        "{name}: repo/REPO.buri is missing, so the case has no repository"
    );
    Case { name, dir: dir.to_path_buf(), doc, steps }
}

fn as_str(case: &str, what: &str, v: &Value) -> String {
    match v {
        Value::Str(s, _) => s.clone(),
        other => panic!("{case}: {what} is {}, not a string", other.kind()),
    }
}

fn as_msg(case: &str, what: &str, v: &Value) -> Msg {
    match v {
        Value::Msg(m, _) => m.clone(),
        other => panic!("{case}: {what} is {}, not a message", other.kind()),
    }
}

fn req_str(case: &str, block: &str, field: &str, m: &Msg) -> String {
    let f = m
        .get(field)
        .unwrap_or_else(|| panic!("{case}: `{block}` has no `{field}`"));
    as_str(case, &format!("{block}.{field}"), &f.value)
}

fn opt_str(case: &str, what: &str, m: &Msg) -> Option<String> {
    let field = what.rsplit('.').next().unwrap();
    m.get(field).map(|f| as_str(case, what, &f.value))
}

fn opt_ident(case: &str, what: &str, m: &Msg) -> Option<String> {
    let field = what.rsplit('.').next().unwrap();
    m.get(field).map(|f| match &f.value {
        Value::Ident(s, _) => s.clone(),
        other => panic!("{case}: {what} is {}, not an identifier", other.kind()),
    })
}

fn req_int(case: &str, block: &str, field: &str, m: &Msg) -> i32 {
    let f = m
        .get(field)
        .unwrap_or_else(|| panic!("{case}: `{block}` has no `{field}` — an exit code is never inferred"));
    match &f.value {
        Value::Int(n, _) => *n as i32,
        other => panic!("{case}: {block}.{field} is {}, not a number", other.kind()),
    }
}

fn str_list(case: &str, what: &str, m: &Msg) -> Vec<String> {
    let field = what.rsplit('.').next().unwrap();
    let f = m
        .get(field)
        .unwrap_or_else(|| panic!("{case}: `run` has no `{field}`"));
    let Value::List(items, _) = &f.value else {
        panic!("{case}: {what} is {}, not a list", f.value.kind())
    };
    assert!(!items.is_empty(), "{case}: {what} is empty");
    items.iter().map(|v| as_str(case, what, v)).collect()
}

// ---------------------------------------------------------------------------
// Running one
// ---------------------------------------------------------------------------

pub fn run_case(case: &Case, g: &mut Golden) {
    let scratch = Scratch::copy_of(&case.name, &case.dir.join("repo"));
    for (i, step) in case.steps.iter().enumerate() {
        match step {
            Step::Run { args, exit, golden, stream, stdin, cwd } => {
                let argv: Vec<&str> = args.iter().map(String::as_str).collect();
                // A `cwd` is a directory inside the scratch copy, so the run
                // is the one a user makes standing in a package.
                let from = match cwd {
                    None => scratch.root.clone(),
                    Some(rel) => {
                        let dir = scratch.path(rel);
                        assert!(
                            dir.is_dir(),
                            "{}: step {} runs in `{rel}`, which is not a directory in the case",
                            case.name,
                            i + 1
                        );
                        dir
                    }
                };
                let mut run = match stdin {
                    None => run_in(&from, &argv),
                    Some(file) => {
                        let path = case.dir.join(file);
                        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                            panic!("{}: cannot read {}: {e}", case.name, path.display())
                        });
                        // `<scratch>` in a request becomes the real path, and
                        // `normalised` turns it back for the golden — so a
                        // fixture can name a URI without knowing where the
                        // scratch tree landed.
                        let text = text.replace("<scratch>", &scratch.root.display().to_string());
                        let mut run = scratch.run_with_stdin(&argv, &frame_requests(&text));
                        // The protocol's framing is a byte count, which changes
                        // whenever a message does. Recording the decoded bodies
                        // instead keeps the golden about what was said.
                        run.stdout = unframe_responses(&run.stdout);
                        run
                    }
                };
                let _ = &mut run;
                if run.code != *exit {
                    g.fail(format!(
                        "{}: step {} `buri {}`{} exited {} rather than {exit}\n  {}\n  printed:\n{}",
                        case.name,
                        i + 1,
                        args.join(" "),
                        match cwd {
                            Some(rel) => format!(" (in {rel})"),
                            None => String::new(),
                        },
                        run.code,
                        case.doc,
                        indent(&run.all())
                    ));
                }
                if let Some(golden) = golden {
                    let printed = match stream {
                        Stream::All => run.normalised(&scratch.root),
                        Stream::Out => super::normalise(&run.stdout, &scratch.root),
                        Stream::Err => super::normalise(&run.stderr, &scratch.root),
                    };
                    g.check(
                        &case.dir.join("expected").join(golden),
                        &format!("{}/{golden}", case.name),
                        &printed,
                    );
                    // A diagnostic that cannot say what to do about it is not
                    // finished. The same rule the reject corpus enforces.
                    if golden.ends_with(".json") {
                        for (n, line) in printed.lines().enumerate() {
                            if line.starts_with('{') && !line.contains("\"fix\":") {
                                g.fail(format!(
                                    "{}/{golden}: diagnostic {} carries no `fix`:\n{}",
                                    case.name,
                                    n + 1,
                                    indent(line)
                                ));
                            }
                        }
                    }
                }
            }
            Step::Edit { file, from, to } => scratch.edit(file, from, to),
            Step::File { path, golden, contains, absent } => {
                let text = scratch.read(path);
                if let Some(golden) = golden {
                    g.check(
                        &case.dir.join("expected").join(golden),
                        &format!("{}/{golden}", case.name),
                        &text,
                    );
                }
                // Never blessed: `contains` and `absent` are hand-written the
                // way `exit` is, so a claim about an artifact cannot be
                // quietly rewritten into whatever the artifact happens to say.
                if let Some(needle) = contains {
                    if !text.contains(needle.as_str()) {
                        g.fail(format!(
                            "{}: {path} does not contain {needle:?}\n  {}",
                            case.name, case.doc
                        ));
                    }
                }
                if let Some(needle) = absent {
                    if text.contains(needle.as_str()) {
                        g.fail(format!(
                            "{}: {path} contains {needle:?}, and must not\n  {}",
                            case.name, case.doc
                        ));
                    }
                }
            }
            Step::Path { path, expect } => {
                let full = scratch.path(path);
                match expect {
                    // `symlink_metadata`, not `exists`: a dangling symlink is
                    // still something on disk, and reporting it as absent
                    // would hide the state `clean` is most likely to leave.
                    PathExpectation::Exists(want) => {
                        let there = full.symlink_metadata().is_ok();
                        if there != *want {
                            g.fail(format!(
                                "{}: step {} expected `{path}` to {}, and it does{}",
                                case.name,
                                i + 1,
                                if *want { "exist" } else { "be gone" },
                                if there { "" } else { " not" }
                            ));
                        }
                    }
                    PathExpectation::Symlink(want) => match std::fs::read_link(&full) {
                        Ok(actual) if actual == std::path::Path::new(want) => {}
                        Ok(actual) => g.fail(format!(
                            "{}: step {} expected `{path}` to point at `{want}`, and it points at `{}`",
                            case.name,
                            i + 1,
                            actual.display()
                        )),
                        Err(e) => g.fail(format!(
                            "{}: step {} expected `{path}` to be a symlink to `{want}`: {e}",
                            case.name,
                            i + 1
                        )),
                    },
                }
            }
        }
    }
}

/// The whole body of a corpus test.
pub fn run_corpus(dir: &Path, what: &str, floor: usize) {
    let mut g = Golden::new();
    let cases = super::case_dirs(dir, "CASE.textproto", floor);
    for dir in &cases {
        run_case(&load_case(dir), &mut g);
    }
    g.finish(what, cases.len());
}

// ---------------------------------------------------------------------------
// Language-server framing
// ---------------------------------------------------------------------------

/// One JSON request per line -> the `Content-Length` framing the protocol uses.
///
/// The fixture stays one message per line because that is what a person can
/// read and a diff can show; the framing is arithmetic, and arithmetic checked
/// into a fixture is arithmetic that goes stale the first time a message is
/// edited.
fn frame_requests(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        out.extend_from_slice(format!("Content-Length: {}\r\n\r\n", line.len()).as_bytes());
        out.extend_from_slice(line.as_bytes());
    }
    out
}

/// The reverse, for the golden: each response body on its own line.
fn unframe_responses(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while let Some(rel) = text[i..].find("\r\n\r\n") {
        let header = &text[i..i + rel];
        let Some(len) = header
            .rsplit("Content-Length:")
            .next()
            .and_then(|v| v.trim().parse::<usize>().ok())
        else {
            break;
        };
        let start = i + rel + 4;
        let end = (start + len).min(b.len());
        out.push_str(&text[start..end]);
        out.push('\n');
        i = end;
    }
    // Anything that was not framed is the server misbehaving, and hiding it
    // would hide exactly the bug this records.
    if i < text.len() && !text[i..].trim().is_empty() {
        out.push_str("<<unframed output>> ");
        out.push_str(text[i..].trim());
        out.push('\n');
    }
    out
}
