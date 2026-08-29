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
//!   repo2/              a *second* repository, if the case needs two
//!   expected/lint.txt   what `lint` prints, recorded
//! ```
//!
//! `repo2/` is copied to a scratch tree of its own, named `<scratch2>` in a
//! session the way `repo/` is named `<scratch>`. A sibling rather than a
//! subdirectory, because a `REPO.buri` inside another repository is not a
//! second root: the outer one's package walk descends into it and claims what
//! is there. Only the language-server cases need it, and only because an editor
//! can hold two repositories open at once.
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
//! command with no target operates on the whole repository wherever it is run,
//! and there is no way to ask that question from the root.
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
//!
//! # Placeholders
//!
//! A case that is about *the platform this toolchain is not* cannot write the
//! platform down: `linux` names a machine the toolchain cannot build for from
//! a mac and names the host on a Linux runner, and the goldens are the text of
//! a message that says so. So a case writes `{{CROSS_PLATFORM}}` instead, and
//! the harness fills it in with a platform chosen from a table keyed on the
//! host ([`platforms_for`]).
//!
//! The substitution reaches all three places a platform can be written, which
//! is the only way it is worth anything:
//!
//! * the fixture files copied into the scratch repository — a `BUILD.buri`
//!   output list;
//! * every string in the manifest — `run.args`, `edit.replace`, `file.path`;
//! * and the recorded output, in reverse, so that what is compared and what
//!   `BURI_BLESS=1` writes both hold the placeholder rather than one host's
//!   answer.
//!
//! That last direction is the same trick [`super::normalise`] already plays
//! with the scratch path: the golden holds `<scratch>`, and what is compared
//! against it is the printed text with the real path put back. A golden here
//! holds `{{CROSS_PLATFORM}}` for the same reason and by the same mechanism.
use std::path::{Path, PathBuf};

use super::{indent, run_in, Golden, Scratch};

use buri::build::textproto::{self, Message, Value};
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
        ///
        /// A line beginning with `!` is the harness acting as the editor
        /// rather than a message: writing back an edit the server returned, or
        /// moving and deleting files the way the editor is about to. See
        /// [`drive_session`].
        stdin: Option<String>,
        /// A directory inside the repository to run from, for the forms that
        /// take no target and mean the repository whatever directory that is.
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
    /// The placeholders this case writes, and what they stand for here.
    pub subst: Subst,
}

// ---------------------------------------------------------------------------
// Placeholders: the platform that is not this host
// ---------------------------------------------------------------------------

/// A platform in the two spellings the toolchain writes: `linux` in a
/// diagnostic, an `--output=` selector and an artifact path, and `LINUX` in a
/// build file.
///
/// Both, because a case needs both and deriving one from the other would be
/// the string munging this table exists to avoid — `Platform::slug` and
/// `Platform::proto` are two functions in the product for the same reason.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Platform {
    pub slug: &'static str,
    pub proto: &'static str,
}

const LINUX: Platform = Platform { slug: "linux", proto: "LINUX" };
const MACOS: Platform = Platform { slug: "macos", proto: "MACOS" };

/// A platform and an architecture: everything `{ platform: .., arch: .. }` and
/// `--output=linux/x86_64` need.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PlatformAndArch {
    pub platform: Platform,
    pub arch: &'static str,
    pub arch_proto: &'static str,
}

const LINUX_X86_64: PlatformAndArch =
    PlatformAndArch { platform: LINUX, arch: "x86_64", arch_proto: "X86_64" };
const MACOS_X86_64: PlatformAndArch =
    PlatformAndArch { platform: MACOS, arch: "x86_64", arch_proto: "X86_64" };

/// The host's platform, and a platform/arch pair that is deliberately **not**
/// the host's.
///
/// A table keyed on the host rather than anything computed, because what a
/// case needs from it is a promise — that the pair names a target this
/// toolchain refuses — and a promise is checked against a table by reading it.
/// The refusal it rests on is `build/link.rs::can_link`: a native build is
/// linked only when the host platform *is* the target's, because the runtime
/// archive `cli/build.rs` embeds is built for the host and there is no cross
/// runtime, cross libc or sysroot (`design/native/ARCHITECTURE.md` §9). So a
/// cross pair fails `native_ready`'s conjunction on this machine and on the CI
/// runner alike, whatever backend is compiled in.
///
/// The architecture is `x86_64` on both rows, and that is not an oversight. A
/// diagnostic's caret run is as wide as the source text it underlines, and a
/// golden records the carets; `MACOS`/`LINUX` are the same width but `ARM64`
/// and `X86_64` are not, so an arm64 row would move a caret run between hosts
/// and no placeholder can stand in for one. `macos/x86_64` is a real target
/// (an Intel mac) and is cross from every Linux host, which is all that is
/// asked of it.
pub fn platforms_for(host_os: &str) -> (Option<Platform>, PlatformAndArch) {
    match host_os {
        "macos" => (Some(MACOS), LINUX_X86_64),
        "linux" => (Some(LINUX), MACOS_X86_64),
        // A host `cli/build.rs` builds no runtime for. Neither platform is
        // this one, so either is a cross one, and this is the row the goldens
        // were written against.
        _ => (None, LINUX_X86_64),
    }
}

/// The whole vocabulary for a host, grouped into the facts it states.
///
/// A *fact* is "the platform that is not this host", and the toolchain writes
/// it two ways: `LINUX` in a build file and `linux` in the diagnostic about
/// that build file. A case names the fact by writing either spelling, and gets
/// both — because a case that declares the output in one spelling always has a
/// golden holding the other, and a placeholder that stopped at the spelling
/// the manifest happened to use would leave a bare `linux` in a recorded file
/// on one host and `macos` on the next.
///
/// The architecture is a second fact rather than part of the first, so that a
/// case naming only a platform does not have `x86_64` rewritten underneath it
/// somewhere it meant the host's.
///
/// There is no `HOST_ARCH`. A CI runner's architecture is not a property of
/// the toolchain, and a golden that named one would pin the machine rather
/// than the product.
pub fn families(host_os: &str) -> Vec<Vec<(&'static str, &'static str)>> {
    let (host, cross) = platforms_for(host_os);
    let mut v = vec![
        vec![
            ("CROSS_PLATFORM", cross.platform.slug),
            ("CROSS_PLATFORM_PROTO", cross.platform.proto),
        ],
        vec![("CROSS_ARCH", cross.arch), ("CROSS_ARCH_PROTO", cross.arch_proto)],
    ];
    if let Some(h) = host {
        v.push(vec![("HOST_PLATFORM", h.slug), ("HOST_PLATFORM_PROTO", h.proto)]);
    }
    v
}

/// The placeholders one case declared, and what they stand for on this host.
///
/// Only the families it declared: reverse substitution rewrites the
/// toolchain's own output, and a case that never wrote `{{CROSS_PLATFORM}}`
/// must get its goldens back byte for byte. An empty `Subst` is the identity
/// in both directions, which is what every other case in the corpus has.
#[derive(Clone, Default)]
pub struct Subst {
    used: Vec<(&'static str, &'static str)>,
}

impl Subst {
    /// The placeholders written anywhere in a case: its manifest, and every
    /// file of the repository it copies.
    pub fn for_case(case: &str, dir: &Path, manifest: &str) -> Subst {
        let mut texts = vec![manifest.to_string()];
        collect_texts(&dir.join("repo"), &mut texts);
        Subst::of(case, std::env::consts::OS, &texts)
    }

    /// The same for a host named rather than detected, which is what makes the
    /// Linux answer testable from a mac.
    pub fn of(case: &str, host_os: &str, texts: &[String]) -> Subst {
        let vocabulary = families(host_os);
        let mut written: Vec<String> = Vec::new();
        for text in texts {
            written.extend(placeholders(text));
        }
        let mut used: Vec<(&'static str, &'static str)> = Vec::new();
        for name in &written {
            let Some(family) = vocabulary.iter().find(|f| f.iter().any(|(n, _)| n == name)) else {
                panic!(
                    "{case}: `{{{{{name}}}}}` is not a placeholder the harness knows on this \
                     host; they are {}",
                    vocabulary
                        .iter()
                        .flatten()
                        .map(|(n, _)| *n)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            // The whole family, not the spelling that happened to be written.
            for &(n, v) in family {
                if !used.iter().any(|(u, _)| *u == n) {
                    used.push((n, v));
                }
            }
        }
        // Two placeholders standing for the same text would make the reverse
        // direction a coin toss, and a golden would flip between them.
        for (i, (n, v)) in used.iter().enumerate() {
            for (m, w) in &used[i + 1..] {
                assert!(
                    v != w,
                    "{case}: `{{{{{n}}}}}` and `{{{{{m}}}}}` both stand for {v:?} on this host, so \
                     a recorded golden could not say which one it meant"
                );
            }
        }
        // Longest first, so a value that is a prefix of another cannot take
        // the shorter answer. Whole-token matching already rules that out;
        // this makes the order deterministic rather than declaration-order.
        used.sort_by(|(an, av), (bn, bv)| bv.len().cmp(&av.len()).then(an.cmp(bn)));
        Subst { used }
    }

    pub fn is_empty(&self) -> bool {
        self.used.is_empty()
    }

    /// `{{CROSS_PLATFORM}}` -> `linux`. What the toolchain is handed.
    pub fn fill(&self, text: &str) -> String {
        let mut s = text.to_string();
        for (name, value) in &self.used {
            s = s.replace(&format!("{{{{{name}}}}}"), value);
        }
        s
    }

    /// `linux` -> `{{CROSS_PLATFORM}}`. What is compared against a golden and
    /// what `BURI_BLESS=1` records, so that blessing writes the placeholder
    /// back rather than this host's answer.
    ///
    /// Whole tokens only: `linux` in `linux-x86_64` is one, `linux` in
    /// `linuxish` is not.
    pub fn hide(&self, text: &str) -> String {
        let mut s = text.to_string();
        for (name, value) in &self.used {
            s = replace_tokens(&s, value, &format!("{{{{{name}}}}}"));
        }
        s
    }

    /// Every file of the scratch copy, rewritten in place. A fixture states an
    /// output list the same way a golden states a message.
    pub fn fill_tree(&self, root: &Path) {
        if self.is_empty() {
            return;
        }
        let mut files = Vec::new();
        collect_files(root, &mut files);
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            if !text.contains("{{") {
                continue;
            }
            std::fs::write(&path, self.fill(&text))
                .unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
        }
    }
}

/// The names between `{{` and `}}`, in order.
///
/// A placeholder is spelled in capitals, so `{{` in a program — where it means
/// something to some other language — is not mistaken for one. A capitalised
/// name the vocabulary does not hold is a typo, and [`Subst::of`] panics on it
/// rather than leaving a token in a fixture that would never match anything.
fn placeholders(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("{{") {
        let after = &rest[i + 2..];
        let Some(j) = after.find("}}") else { break };
        let name = &after[..j];
        if !name.is_empty()
            && name.chars().all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
        {
            out.push(name.to_string());
        }
        rest = &after[j + 2..];
    }
    out
}

/// `from` -> `to`, but only where `from` is a whole token: neither neighbour is
/// a letter, a digit or an underscore. Hand-rolled, like `replace_between` in
/// the harness proper, because the toolchain has no regex and is not getting
/// one.
fn replace_tokens(text: &str, from: &str, to: &str) -> String {
    if from.is_empty() {
        return text.to_string();
    }
    let bytes = text.as_bytes();
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(rel) = text[i..].find(from) {
        let start = i + rel;
        let end = start + from.len();
        let before_ok = start == 0 || !word(bytes[start - 1]);
        let after_ok = end == bytes.len() || !word(bytes[end]);
        out.push_str(&text[i..start]);
        if before_ok && after_ok {
            out.push_str(to);
        } else {
            out.push_str(from);
        }
        i = end;
    }
    out.push_str(&text[i..]);
    out
}

/// Every file under `dir`, sorted, so a scan is the same on two machines.
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn collect_texts(dir: &Path, out: &mut Vec<String>) {
    let mut files = Vec::new();
    collect_files(dir, &mut files);
    for path in files {
        if let Ok(text) = std::fs::read_to_string(&path) {
            out.push(text);
        }
    }
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
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{name}: cannot read {}: {e}", path.display()));
    // Filled before parsing, so every string in the manifest is covered by one
    // substitution: an `--output=` argument, the text an `edit` looks for, a
    // path. Which placeholders the case declared is read from the manifest as
    // written, and from the fixtures, before any of it is filled in.
    let subst = Subst::for_case(&name, dir, &raw);
    let text = subst.fill(&raw);
    let parsed = textproto::parse(&text, FileId(0));
    assert!(
        parsed.errors.is_empty(),
        "{name}: CASE.textproto does not parse ({} error(s)); the first is at byte {}",
        parsed.errors.len(),
        parsed.errors[0].span.start
    );

    let mut doc = None;
    let mut steps = Vec::new();
    for field in &parsed.document.fields {
        match field.name.as_str() {
            "doc" => doc = Some(as_str(&name, "doc", &field.value)),
            "run" => {
                let message = as_message(&name, "run", &field.value);
                steps.push(Step::Run {
                    args: str_list(&name, "run.args", &message),
                    exit: required_int(&name, "run", "exit", &message),
                    golden: optional_str(&name, "run.golden", &message),
                    stdin: optional_str(&name, "run.stdin", &message),
                    cwd: optional_str(&name, "run.cwd", &message),
                    stream: match optional_ident(&name, "run.stream", &message).as_deref() {
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
                let message = as_message(&name, "edit", &field.value);
                steps.push(Step::Edit {
                    file: required_str(&name, "edit", "file", &message),
                    from: required_str(&name, "edit", "replace", &message),
                    to: required_str(&name, "edit", "with", &message),
                });
            }
            "file" => {
                let message = as_message(&name, "file", &field.value);
                let step = Step::File {
                    path: required_str(&name, "file", "path", &message),
                    golden: optional_str(&name, "file.golden", &message),
                    contains: optional_str(&name, "file.contains", &message),
                    absent: optional_str(&name, "file.absent", &message),
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
                let message = as_message(&name, "path", &field.value);
                let symlink = optional_str(&name, "path.symlink", &message);
                let raw_exists = optional_ident(&name, "path.exists", &message);
                let exists = raw_exists.map(|v| match v.as_str() {
                    "true" => true,
                    "false" => false,
                    other => panic!("{name}: path.exists is {other}, not true or false"),
                });
                steps.push(Step::Path {
                    path: required_str(&name, "path", "path", &message),
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
    Case { name, dir: dir.to_path_buf(), doc, steps, subst }
}

fn as_str(case: &str, what: &str, v: &Value) -> String {
    match v {
        Value::Str(s, _) => s.clone(),
        other => panic!("{case}: {what} is {}, not a string", other.kind()),
    }
}

fn as_message(case: &str, what: &str, v: &Value) -> Message {
    match v {
        Value::Message(m, _) => m.clone(),
        other => panic!("{case}: {what} is {}, not a message", other.kind()),
    }
}

fn required_str(case: &str, block: &str, field: &str, message: &Message) -> String {
    let f = message
        .get(field)
        .unwrap_or_else(|| panic!("{case}: `{block}` has no `{field}`"));
    as_str(case, &format!("{block}.{field}"), &f.value)
}

fn optional_str(case: &str, what: &str, message: &Message) -> Option<String> {
    let field = what.rsplit('.').next().unwrap();
    message.get(field).map(|f| as_str(case, what, &f.value))
}

fn optional_ident(case: &str, what: &str, message: &Message) -> Option<String> {
    let field = what.rsplit('.').next().unwrap();
    message.get(field).map(|f| match &f.value {
        Value::Ident(s, _) => s.clone(),
        other => panic!("{case}: {what} is {}, not an identifier", other.kind()),
    })
}

fn required_int(case: &str, block: &str, field: &str, message: &Message) -> i32 {
    let f = message
        .get(field)
        .unwrap_or_else(|| panic!("{case}: `{block}` has no `{field}` — an exit code is never inferred"));
    match &f.value {
        Value::Int(n, _) => *n as i32,
        other => panic!("{case}: {block}.{field} is {}, not a number", other.kind()),
    }
}

fn str_list(case: &str, what: &str, message: &Message) -> Vec<String> {
    let field = what.rsplit('.').next().unwrap();
    let f = message
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
    // A case may carry a *second* repository, in `repo2/`, and it is copied to
    // a scratch tree of its own rather than to a directory inside the first
    // one. Two Buri repositories are two roots, and a root nested inside
    // another is not two: the outer one's package walk descends into it and
    // claims its packages. A session names it `<scratch2>`.
    let second_source = case.dir.join("repo2");
    let second = second_source
        .is_dir()
        .then(|| Scratch::copy_of(&format!("{}-second", case.name), &second_source));
    // The fixtures are filled in on the copy, never in the checked-in tree:
    // what is in the repository is the placeholder, which is the thing that
    // reads the same on both hosts.
    case.subst.fill_tree(&scratch.root);
    if let Some(second) = &second {
        case.subst.fill_tree(&second.root);
    }
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
                        // scratch tree landed. `<scratch2>` is the same for a
                        // case holding a second repository.
                        let mut text =
                            text.replace("<scratch>", &scratch.root.display().to_string());
                        if let Some(second) = &second {
                            text =
                                text.replace("<scratch2>", &second.root.display().to_string());
                        }
                        let mut run = drive_session(&case.name, &from, &argv, &text);
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
                    let raw = match stream {
                        Stream::All => run.all(),
                        Stream::Out => run.stdout.clone(),
                        Stream::Err => run.stderr.clone(),
                    };
                    // The second repository's path goes back first: it is a
                    // sibling of the first, so neither is a prefix of the
                    // other and the order is only a matter of saying so.
                    let raw = match &second {
                        Some(s) => raw.replace(&s.root.display().to_string(), "<scratch2>"),
                        None => raw,
                    };
                    let printed = super::normalise(&raw, &scratch.root);
                    // The placeholder goes back in before the golden is either
                    // compared or recorded — one text, so blessing writes
                    // exactly what a later run compares.
                    let printed = case.subst.hide(&printed);
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
                        &case.subst.hide(&text),
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

/// Drives one language-server session, and acts on disk partway through it
/// when the fixture asks.
///
/// A session with no `!` directive is written whole and the pipe closed, which
/// is what every recorded session but one needs. The exception is the round
/// trip a code action is *for*: the server offers an edit, the client writes
/// it, and the server has to hear about the write. The client here is this
/// function, and the waiting is the whole of why it exists — piping the session
/// and editing a file partway through would leave which side of the edit each
/// request landed on to the scheduler rather than to the case.
///
/// ```text
/// !apply 2       wait for the response to request 2, then write the workspace
///                edit its first code action carried
/// !edit 2        the same, for a response that is a workspace edit itself
/// !wait 2        read up to the response to request 2 and do nothing else
/// !move a.buri b.buri   move a file, as the editor is about to
/// !remove a.buri        delete one
/// ```
fn drive_session(case: &str, dir: &Path, args: &[&str], session: &str) -> super::Run {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};

    let mut argv: Vec<&str> = args.to_vec();
    argv.push("--color=never");
    let mut child = Command::new(super::buri())
        .args(&argv)
        // A recorded session is a byte stream, and the language server sweeps
        // whole repositories on a worker thread — so where a sweep's publishes
        // land would otherwise be the scheduler's decision. This is the
        // schedule that answers each message completely before reading the
        // next. A session that wants the worker says so in its
        // `initializationOptions`; see `language_server::sweep`.
        .env("BURI_LSP_ANALYSIS", "synchronous")
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the buri binary runs");
    let mut input = child.stdin.take().expect("stdin is piped");
    let mut output = child.stdout.take().expect("stdout is piped");
    // Every byte read while waiting, kept framed: what the golden records is
    // this stream and the rest of it, in the order the server wrote them.
    let mut read_early = String::new();

    let mut pending = String::new();
    for line in session.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(directive) = line.strip_prefix('!') else {
            pending.push_str(line);
            pending.push('\n');
            continue;
        };
        input.write_all(&frame_requests(&pending)).expect("writing the session");
        pending.clear();
        act(case, dir, directive, &mut output, &mut read_early);
    }
    input.write_all(&frame_requests(&pending)).expect("writing the session");
    // Dropped here, which closes the pipe and is what tells the server the
    // session is over.
    drop(input);
    let mut rest = Vec::new();
    output.read_to_end(&mut rest).expect("reading the rest of the session");
    let status = child.wait().expect("the buri binary finishes");
    let mut stderr = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut stderr);
    }
    super::Run {
        code: status.code().unwrap_or(-1),
        stdout: super::strip_ansi(&format!("{read_early}{}", String::from_utf8_lossy(&rest))),
        stderr: super::strip_ansi(&stderr),
        what: format!("buri {}", args.join(" ")),
    }
}

/// One `!` line: the harness doing what an editor would do between messages.
///
/// The three that wait take a request id and read up to its response first,
/// which is the whole reason these exist — piping the session and touching the
/// disk partway through would leave which side of the change each request
/// landed on to the scheduler rather than to the case. A `!move` or `!remove`
/// that has to land after a particular answer says so with a `!wait` in front
/// of it.
fn act(
    case: &str,
    dir: &Path,
    directive: &str,
    output: &mut impl std::io::Read,
    read_early: &mut String,
) {
    let (verb, rest) = directive.split_once(' ').unwrap_or((directive, ""));
    let rest = rest.trim();
    match verb {
        // The first action's edit, because a fixture that asked for this asked
        // about one fix. A `will*Files` answer *is* the edit, with no action
        // wrapped around it, which is the difference between the two.
        "apply" | "edit" => {
            let id = rest
                .parse::<u32>()
                .unwrap_or_else(|_| panic!("{case}: `!{directive}` does not name a request id"));
            let body = read_until(output, id, read_early)
                .unwrap_or_else(|| panic!("{case}: the server never answered request {id}"));
            let parsed = buri::json::parse(&body)
                .unwrap_or_else(|e| panic!("{case}: the response is not JSON: {e}"));
            let edit = match verb {
                "apply" => parsed
                    .at("result")
                    .and_then(|r| r.as_array())
                    .and_then(|actions| actions.first())
                    .unwrap_or_else(|| {
                        panic!("{case}: the response carries no code action:\n{body}")
                    })
                    .at("edit"),
                _ => parsed.at("result"),
            };
            apply_workspace_edit(case, edit, &body);
        }
        // The answer itself is the golden's business; this only says that it
        // arrived before whatever the next line does to the file tree.
        "wait" => {
            let id = rest
                .parse::<u32>()
                .unwrap_or_else(|_| panic!("{case}: `!{directive}` does not name a request id"));
            read_until(output, id, read_early)
                .unwrap_or_else(|| panic!("{case}: the server never answered request {id}"));
        }
        "move" => {
            let (from, to) = rest
                .split_once(' ')
                .map(|(a, b)| (dir.join(a.trim()), dir.join(b.trim())))
                .unwrap_or_else(|| panic!("{case}: `!{directive}` does not name two paths"));
            if let Some(parent) = to.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::rename(&from, &to).unwrap_or_else(|e| {
                panic!("{case}: cannot move {}: {e}", from.display());
            });
        }
        "remove" => {
            let path = dir.join(rest);
            std::fs::remove_file(&path)
                .unwrap_or_else(|e| panic!("{case}: cannot remove {}: {e}", path.display()));
        }
        _ => panic!("{case}: `!{directive}` is not a directive this harness has"),
    }
}

/// Reads framed messages until the response with `id` arrives, keeping every
/// byte read so that nothing said along the way is lost to the golden.
fn read_until(output: &mut impl std::io::Read, id: u32, kept: &mut String) -> Option<String> {
    loop {
        let (framed, body) = read_frame(output)?;
        kept.push_str(&framed);
        let parsed = buri::json::parse(&body).ok()?;
        if parsed.get("id").and_then(|v| v.as_u32()) == Some(id) {
            return Some(body);
        }
    }
}

/// One `Content-Length` message: the bytes as they arrived, and the body.
///
/// A byte at a time, for the reason the server itself reads that way — a
/// buffered reader would swallow the body of the next message along with the
/// headers of this one.
fn read_frame(output: &mut impl std::io::Read) -> Option<(String, String)> {
    let mut raw = String::new();
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        loop {
            let mut byte = [0u8; 1];
            match output.read(&mut byte) {
                Ok(0) | Err(_) => return None,
                Ok(_) => {
                    line.push(byte[0] as char);
                    if byte[0] == b'\n' {
                        break;
                    }
                }
            }
        }
        raw.push_str(&line);
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(v) = trimmed.strip_prefix("Content-Length:") {
            length = v.trim().parse().ok();
        }
    }
    let mut body = vec![0u8; length?];
    output.read_exact(&mut body).ok()?;
    let body = String::from_utf8(body).ok()?;
    raw.push_str(&body);
    Some((raw, body))
}

/// Writes a `WorkspaceEdit`, the way an editor would when someone accepts it.
///
/// Later edits are applied first so that an earlier one's offsets are still the
/// ones the server measured.
fn apply_workspace_edit(case: &str, edit: Option<&buri::json::Value>, response: &str) {
    let changes = edit
        .and_then(|e| e.at("changes"))
        .unwrap_or_else(|| panic!("{case}: the response carries no workspace edit:\n{response}"));
    let buri::json::Value::Object(files) = changes else {
        panic!("{case}: `edit.changes` is not an object:\n{response}")
    };
    for (uri, edits) in files {
        let path = PathBuf::from(
            uri.strip_prefix("file://")
                .unwrap_or_else(|| panic!("{case}: `{uri}` is not a file URI")),
        );
        let mut text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{case}: cannot read {}: {e}", path.display()));
        let mut edits: Vec<&buri::json::Value> =
            edits.as_array().unwrap_or_default().iter().collect();
        edits.sort_by_key(|e| std::cmp::Reverse(offset_key(e, "range.start")));
        for edit in edits {
            let start = offset_of(&text, edit, "range.start");
            let end = offset_of(&text, edit, "range.end");
            let new = edit.at("newText").and_then(|t| t.as_str()).unwrap_or_default();
            text.replace_range(start..end, new);
        }
        std::fs::write(&path, &text)
            .unwrap_or_else(|e| panic!("{case}: cannot write {}: {e}", path.display()));
    }
}

/// A position as a sortable pair, for ordering edits without the text.
fn offset_key(edit: &buri::json::Value, at: &str) -> (u32, u32) {
    let line = edit.at(&format!("{at}.line")).and_then(|v| v.as_u32()).unwrap_or(0);
    let character = edit.at(&format!("{at}.character")).and_then(|v| v.as_u32()).unwrap_or(0);
    (line, character)
}

/// A protocol position as a byte offset into the text.
///
/// A character is one code unit here, which is the protocol's `utf-16` encoding
/// only for text inside the basic plane. A fixture is a program someone reads,
/// so that is the whole of what these cases hold.
fn offset_of(text: &str, edit: &buri::json::Value, at: &str) -> usize {
    let (line, character) = offset_key(edit, at);
    let mut offset = 0;
    for _ in 0..line {
        match text[offset..].find('\n') {
            Some(i) => offset += i + 1,
            None => return text.len(),
        }
    }
    let rest = &text[offset..];
    let end = rest.find('\n').map_or(rest.len(), |i| i);
    let column: usize = rest[..end]
        .chars()
        .take(character as usize)
        .map(char::len_utf8)
        .sum();
    (offset + column).min(text.len())
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

// ---------------------------------------------------------------------------
// The substitution, on both hosts
// ---------------------------------------------------------------------------

#[cfg(test)]
mod placeholder_tests {
    use super::*;

    /// Everything a case writes, in one string, so the substitution is
    /// exercised on the shapes it actually meets.
    const CASE: &str = "run { args: [\"build\", \"//cmd/native\", \
                        \"--output={{CROSS_PLATFORM}}/{{CROSS_ARCH}}\"] }\n\
                        run { args: [\"--output={{CROSS_PLATFORM}}-{{CROSS_ARCH}}\"] }\n\
                        { platform: {{CROSS_PLATFORM_PROTO}}, arch: {{CROSS_ARCH_PROTO}} },\n";

    fn subst(host: &str) -> Subst {
        Subst::of("test", host, &[CASE.to_string()])
    }

    /// The promise the table makes, and the only one a case rests on: the pair
    /// is not the host, so `link.rs::can_link` refuses it — the runtime
    /// archive is the host's and there is no cross one.
    #[test]
    fn the_cross_pair_is_never_the_host() {
        for host in ["macos", "linux", "windows"] {
            let (host_plat, cross) = platforms_for(host);
            assert_ne!(
                host_plat.map(|p| p.slug),
                Some(cross.platform.slug),
                "{host}: the cross platform is the host's, so the build would succeed"
            );
        }
    }

    /// The one thing in a golden no placeholder can stand in for is a caret
    /// run: `^^^^` is as wide as the source text it underlines, and that text
    /// is a filled-in fixture. So the two hosts have to spell every fact at the
    /// same width, or a golden recorded on one would not hold on the other.
    #[test]
    fn the_two_hosts_spell_every_fact_at_the_same_width() {
        for (mac, lin) in families("macos").iter().zip(&families("linux")) {
            for (&(name, here), &(also, there)) in mac.iter().zip(lin) {
                assert_eq!(name, also, "the two hosts have different vocabularies");
                assert_eq!(
                    here.len(),
                    there.len(),
                    "`{{{{{name}}}}}` is {here:?} here and {there:?} on Linux, and the two are \
                     different widths — a recorded caret run would not line up"
                );
            }
        }
    }

    /// A mac fills in Linux; the CI runner this is written for fills in macOS.
    /// The Linux answer is the one that cannot be observed by running the
    /// suite here, so it is asserted rather than run.
    #[test]
    fn each_host_gets_the_other_platform() {
        assert_eq!(
            subst("macos").fill("--output={{CROSS_PLATFORM}}/{{CROSS_ARCH}}"),
            "--output=linux/x86_64"
        );
        assert_eq!(
            subst("linux").fill("--output={{CROSS_PLATFORM}}/{{CROSS_ARCH}}"),
            "--output=macos/x86_64"
        );
        assert_eq!(
            subst("linux").fill("{ platform: {{CROSS_PLATFORM_PROTO}} }"),
            "{ platform: MACOS }"
        );
    }

    /// What a Linux runner records is what the repository already holds. This
    /// is the round trip `BURI_BLESS=1` performs, with the host forced.
    #[test]
    fn blessing_records_the_placeholder_on_either_host() {
        let recorded = "error: the {{CROSS_PLATFORM}} backend is not implemented\n \
                        --> cmd/native/BUILD.buri:6:9\n  \
                        = fix: drop the {{CROSS_PLATFORM}} output\n\
                        .buri/out/{{CROSS_PLATFORM}}-{{CROSS_ARCH}}/cmd/native/native\n";
        for host in ["macos", "linux"] {
            let s = subst(host);
            let printed = s.fill(recorded);
            assert!(!printed.contains("{{"), "{host}: a placeholder survived the fill");
            assert_eq!(s.hide(&printed), recorded, "{host}: the round trip lost something");
        }
    }

    /// Reverse substitution rewrites the toolchain's own output, so it may only
    /// touch whole tokens — and only for a case that declared the placeholder.
    #[test]
    fn only_whole_tokens_of_a_declared_placeholder_are_hidden() {
        let s = subst("macos");
        assert_eq!(s.hide("linuxish linux-x86_64 delinux /linux/"),
                   "linuxish {{CROSS_PLATFORM}}-{{CROSS_ARCH}} delinux /{{CROSS_PLATFORM}}/");
        // A macOS host's own platform is not in this case's vocabulary, so it
        // is left exactly as the toolchain printed it.
        assert_eq!(s.hide("built for macos"), "built for macos");
        // And a case that declares nothing is the identity, which is what the
        // other hundred-odd cases in the corpus rely on.
        let none = Subst::of("test", "macos", &["doc: \"nothing\"".to_string()]);
        assert!(none.is_empty());
        assert_eq!(none.hide("linux x86_64 macos"), "linux x86_64 macos");
    }

    /// A capitalised name nobody defined is a typo, and a typo that survived
    /// would be a fixture holding a token no diagnostic can ever match.
    #[test]
    #[should_panic(expected = "is not a placeholder the harness knows")]
    fn an_unknown_placeholder_is_a_mistake_in_the_case() {
        Subst::of("test", "macos", &["--output={{CROSS_PLATFROM}}".to_string()]);
    }

    /// `{{` means something in other languages, and a fixture is a program.
    #[test]
    fn lowercase_braces_are_not_placeholders() {
        assert!(placeholders("let x = {{ a: 1 }};").is_empty());
        assert_eq!(placeholders("{{CROSS_ARCH}}"), vec!["CROSS_ARCH".to_string()]);
    }
}
