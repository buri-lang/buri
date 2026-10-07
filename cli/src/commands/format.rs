//! `buri format`.
//!
//! No options and no configuration file: a formatter with options is a
//! formatter whose output is a repository decision. This file is the command —
//! finding the files and writing them back. The printing itself is
//! `crate::formatting`, which the documentation renderer and `buri gen` use
//! too.
//!
//! # Everywhere somebody wrote Buri
//!
//! Four kinds of file, one command, one layout:
//!
//!   * **source** and **build files**, through the two printers below;
//!   * **JSON, JSONC, JSON5, `.proto` and text format** that a rule's `inputs` lists,
//!     through `crate::languages`. Any other such file is not the repository's
//!     to format;
//!   * **markdown**, where every ```` ```buri ```` fence is laid out and the
//!     prose around it is left exactly as it was written;
//!   * **a source file's own documentation comments**, where an example is what
//!     an editor shows on hover.
//!
//! The last two are `documentation::layout`, and they are here rather than in a
//! command of their own because the alternative is a repository whose sources
//! are formatted and whose examples are not — and the examples are what a
//! newcomer copies. `--check` gates all three the same way.
//!
//! Both spellings of "this part of the repository" get you there — a path, and
//! a target label — because this is the one command whose subject is a file and
//! whose caller is as likely to be holding a label. [`select`] is where the two
//! meet.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "the list of files it formatted, or would, is this command's output; \
              diagnostics still leave through `Session::emit`"
)]

use crate::build::cache::{Action, Cache, KeyBuilder};
use crate::build::session;
use crate::build::textproto;
use crate::build::workspace::{PackageId, Workspace};
use crate::commands::arguments;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Whether a file is a build file rather than source.
///
/// The name decides, not the extension: a build file is `.buri` too, because a
/// repository has one kind of file in it and one command that formats them.
fn is_build_file(name: &str) -> bool {
    name == "BUILD.buri" || name == "REPO.buri"
}

/// The canonical form of one file, whichever of the two it is, or `None` when
/// there is none.
///
/// One function, so that the command, the language server and the tests cannot
/// disagree about which printer a file goes through.
pub fn file(name: &str, text: &str) -> Option<String> {
    formatted(name, text).map(|f| f.text)
}

/// The same, and which declarations the parser could not read.
///
/// A source file with a syntax error still has a canonical form: what parsed
/// is laid out and what did not is printed as it was written. `None` is left
/// for the file there is nothing to be said about — a build file that does not
/// read, or source the formatter could not vouch for.
pub fn formatted(name: &str, text: &str) -> Option<crate::formatting::Formatted> {
    let languages = crate::languages::Languages::default();
    if languages.of(name).is_some() {
        let text = crate::languages::format(&languages, name, text)?;
        return Some(crate::formatting::Formatted { text, regions: Vec::new() });
    }
    if is_build_file(name) {
        let parsed = textproto::parse(text, crate::diagnostics::FileId(0));
        if !parsed.errors.is_empty() {
            return None;
        }
        let text = textproto::print(&parsed.document);
        return Some(crate::formatting::Formatted { text, regions: Vec::new() });
    }
    // A platform's surface declares its entries and production methods
    // without a body, as the standard library does.
    if name == "platform.buri" || name.ends_with("/platform.buri") {
        return crate::formatting::formatted(text, crate::formatting::Dialect::Std);
    }
    crate::formatting::source_with_regions(text)
}

/// A label, rather than a path.
///
/// Both spellings are repository-absolute and only one of them says so: a label
/// starts with `//`, and `@` is the external-repository form, which
/// [`crate::build::workspace::Pattern::parse`] refuses by name rather than
/// leaving to be read as a directory called `@other`. Nothing else can be
/// mistaken for a label, so nothing else is asked about here.
fn is_label(argument: &str) -> bool {
    argument.starts_with("//") || argument.starts_with('@')
}

/// The files one invocation is about: the sources and build files to lay out,
/// and the documents whose fences to lay out.
///
/// Two spellings reach the same repository. A **path** is a file or a
/// directory, and everything under it is formatted — the markdown included,
/// which no build file declares. A **label** names packages, and a package's
/// files are the sources its rules declare plus the `BUILD.buri` that declares
/// them: the set `buri gen` and `buri lint` already resolve a label to, so the
/// three commands can be asked about the same thing in the same words.
///
/// An argument that is neither is refused. Matching it against nothing and
/// exiting 0 is the one answer a `--check` must never give, because a silent
/// success reads as a tree that was looked at and found clean.
fn select(
    session: &session::Session,
    arguments: &[String],
) -> Result<(Vec<PathBuf>, Vec<PathBuf>), String> {
    let mut files = Vec::new();
    let mut documents = Vec::new();
    let referenced = referenced(session);
    if arguments.is_empty() {
        collect(&session.root, &mut files);
        files.extend(referenced);
        crate::documentation::layout::documents_under(&session.root, &mut documents);
        return Ok((files, documents));
    }

    let mut labels: Vec<String> = Vec::new();
    for argument in arguments {
        if is_label(argument) {
            labels.push(argument.clone());
            continue;
        }
        let path = session.root.join(argument);
        if !path.exists() {
            return Err(format!(
                "`{argument}` is not a path in this repository; `buri format` takes a path to a \
                 file or a directory, or a label such as `//lib/money/...`"
            ));
        }
        collect(&path, &mut files);
        files.extend(referenced.iter().filter(|f| f.starts_with(&path)).cloned());
        crate::documentation::layout::documents_under(&path, &mut documents);
    }
    // No label is not the same question with an empty answer: `resolve_targets`
    // reads an empty argument list as `//...`, which is right for a command
    // given no arguments and wrong for one given three paths.
    if labels.is_empty() {
        return Ok((files, documents));
    }
    let mut packages: Vec<PackageId> =
        session.resolve_targets(&labels)?.iter().map(|t| t.package).collect();
    packages.sort();
    packages.dedup();
    for id in packages {
        let package = session.workspace.package(id);
        files.push(package.build_path.clone());
        for source in session.workspace.declared_sources(id) {
            files.push(package.dir.join(source));
        }
        files.extend(referenced.iter().filter(|f| owned_by(session, id, f)).cloned());
    }
    Ok((files, documents))
}

/// Every file some rule's `inputs` lists in a language this repository knows:
/// the whole set of non-Buri files `buri format` touches.
fn referenced(session: &session::Session) -> Vec<PathBuf> {
    let workspace = &session.workspace;
    let mut out: Vec<PathBuf> = Vec::new();
    for target in workspace.targets() {
        let dir = &workspace.package(target.package).dir;
        for input in crate::build::generators::inputs(workspace, target) {
            if workspace.repo.languages.of(&input).is_some() {
                out.push(dir.join(input));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Whether a referenced file is one a rule of this package lists.
fn owned_by(session: &session::Session, id: PackageId, file: &Path) -> bool {
    let workspace = &session.workspace;
    let dir = &workspace.package(id).dir;
    workspace
        .targets()
        .into_iter()
        .filter(|t| t.package == id)
        .any(|t| crate::build::generators::inputs(workspace, t).iter().any(|i| dir.join(i) == file))
}

/// Formats `.buri` sources, build files, and the Buri written in documentation,
/// with no options and no configuration file. A formatter with options is a
/// formatter whose output is a repository decision.
pub fn command_format(args: &arguments::Args) -> i32 {
    let code = format_all(args);
    if crate::profile::enabled() {
        eprintln!("files formatted {}", FORMATTED.load(Ordering::Relaxed));
    }
    code
}

/// The files the formatter ran over this command, rather than answered from
/// what it remembered: `files formatted` in a `BURI_PROFILE` report.
static FORMATTED: AtomicU64 = AtomicU64::new(0);

/// What laying out one file came to.
#[derive(Default)]
struct Outcome {
    /// The canonical text differs from the file's.
    changed: bool,
    /// That text, when it is to be written back.
    text: Option<String>,
    /// A syntax error kept part of the file out of the formatter's hands.
    unread: bool,
    /// The formatter would not lay the file out at all.
    refused: bool,
    /// A build file that does not parse, which stops the command.
    broken_build: bool,
}

/// One file's part in the command, worked out on any thread.
enum Work {
    Skip,
    /// A repository's own language, whose tool lays it out. Asked in order on
    /// this thread, because asking may build the tool.
    Tool(String),
    Done(String, Outcome),
}

fn format_all(args: &arguments::Args) -> i32 {
    let session = match session::open_or_exit(&args.flags) {
        Ok(session) => session,
        Err(c) => return c as i32,
    };
    let (mut files, mut documents) = match select(&session, &args.targets) {
        Ok(both) => both,
        Err(message) => {
            eprintln!("error: {message}");
            return 2;
        }
    };
    files.sort();
    files.dedup();
    let referenced = referenced(&session);
    let write = !args.flags.check;
    let cache = Cache::open(&session.root);
    let workspace = &*session.workspace;
    let work = crate::parallel::map(files.len(), |i| {
        files.get(i).map_or(Work::Skip, |path| lay_out(workspace, &referenced, &cache, path, write))
    });

    let mut changed = Vec::new();
    // The files a syntax error kept part or all of out of the formatter's
    // hands. They are named rather than skipped in silence: a file the
    // formatter could not read whole is not a file it has checked, and a
    // `--check` that passed one would be reporting a gate it did not run.
    let mut unread = Vec::new();
    let mut refused = Vec::new();
    for (path, work) in files.iter().zip(work) {
        let (rel, outcome) = match work {
            Work::Skip => continue,
            Work::Done(rel, outcome) => (rel, outcome),
            Work::Tool(rel) => {
                let Ok(text) = std::fs::read_to_string(path) else { continue };
                use crate::build::tools::Formatted;
                match crate::build::tools::format_file(&session, &rel, &text, &args.flags) {
                    Formatted::Refused => refused.push(rel),
                    Formatted::Text(out) if out != text => {
                        changed.push(rel);
                        if write {
                            let _ = std::fs::write(path, out);
                        }
                    }
                    Formatted::Text(_) | Formatted::Unformatted => {}
                }
                continue;
            }
        };
        // A build file that does not read is a hard error, because nothing
        // else in the repository will work until it is fixed.
        if outcome.broken_build {
            eprintln!("error: {rel} does not parse");
            return 2;
        }
        if outcome.refused {
            refused.push(rel);
            continue;
        }
        if outcome.unread {
            unread.push(rel.clone());
        }
        if outcome.changed {
            if let Some(text) = outcome.text {
                let _ = std::fs::write(path, text);
            }
            changed.push(rel);
        }
    }

    // The documents. A fence body is laid out and nothing else on the page is
    // touched — the prose is the author's.
    documents.sort();
    documents.dedup();
    let laid_out = crate::parallel::map(documents.len(), |i| {
        let path = documents.get(i)?;
        let text = std::fs::read_to_string(path).ok()?;
        let rel = workspace.rel_of(path);
        let outcome = remembered(&cache, "document", &rel, &text, write, || {
            let out = crate::documentation::layout::format_document(&rel, &text);
            Outcome { changed: out.is_some(), text: out, ..Outcome::default() }
        });
        Some((rel, outcome))
    });
    for (path, done) in documents.iter().zip(laid_out) {
        let Some((rel, outcome)) = done else { continue };
        if !outcome.changed {
            continue;
        }
        if let Some(text) = outcome.text {
            let _ = std::fs::write(path, text);
        }
        changed.push(rel);
    }
    changed.sort();

    if args.flags.check {
        // The CI form: exit non-zero on any file that would change, and on any
        // the formatter could not read.
        for c in &changed {
            println!("{c}");
        }
        report(&unread, &refused);
        return i32::from(!changed.is_empty() || !unread.is_empty() || !refused.is_empty());
    }
    for c in &changed {
        println!("formatted {c}");
    }
    report(&unread, &refused);
    0
}

/// Lays out one source, build file or referenced file, or says why not.
fn lay_out(
    workspace: &Workspace,
    referenced: &[PathBuf],
    cache: &Cache,
    path: &Path,
    write: bool,
) -> Work {
    let rel = workspace.rel_of(path);
    if let Some(language) = workspace.repo.languages.of(&rel) {
        if !referenced.iter().any(|r| r == path) {
            return Work::Skip;
        }
        if matches!(language.kind, crate::languages::Kind::Custom(_)) {
            return Work::Tool(rel);
        }
        let Ok(text) = std::fs::read_to_string(path) else { return Work::Skip };
        let settings = format!("{:?}", language.kind);
        let outcome = remembered(cache, &settings, &rel, &text, write, || {
            use crate::build::tools::Formatted;
            match crate::build::tools::format_in_process(&language.kind, &text) {
                Some(Formatted::Refused) => Outcome { refused: true, ..Outcome::default() },
                Some(Formatted::Text(out)) if out != text => {
                    Outcome { changed: true, text: Some(out), ..Outcome::default() }
                }
                _ => Outcome::default(),
            }
        });
        return Work::Done(rel, outcome);
    }
    let Ok(text) = std::fs::read_to_string(path) else { return Work::Skip };
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
        return Work::Skip;
    };
    let outcome = remembered(cache, "buri", &rel, &text, write, || {
        let Some(out) = formatted(&name, &text) else {
            return Outcome { broken_build: is_build_file(&name), refused: true, ..Outcome::default() };
        };
        // And the examples in this file's documentation comments, through the
        // same printer. The layout pass never reaches inside a comment, so the
        // fences are still where they were when this looks for them.
        let laid_out = crate::documentation::layout::format_doc_comments(&out.text).unwrap_or(out.text);
        let changed = laid_out != text;
        Outcome { changed, text: changed.then_some(laid_out), unread: !out.regions.is_empty(), ..Outcome::default() }
    });
    Work::Done(rel, outcome)
}

/// The shape of a remembered answer, so a change to it is a miss rather than
/// a misreading.
const VERDICT: &[u8] = b"buri-format-verdict-1\n";

/// `lay_out`'s answer for one file, from the cache when this toolchain has
/// laid out these exact bytes under this name and these settings before.
///
/// Only the verdict is kept, not the text, so a remembered change that is to
/// be written back is laid out again. A build file that does not parse is
/// never kept: it stops the command, and says so every time.
fn remembered(
    cache: &Cache,
    settings: &str,
    rel: &str,
    text: &str,
    write: bool,
    lay_out: impl FnOnce() -> Outcome,
) -> Outcome {
    let mut k = KeyBuilder::new(Action::Format, arguments::BuildMode::Debug);
    k.input("in-process", settings.as_bytes());
    k.input(rel, text.as_bytes());
    let key = k.finish();
    let hit = cache.get(&key).and_then(|bytes| {
        let flags = bytes.strip_prefix(VERDICT)?;
        let [changed, unread, refused] = *flags else { return None };
        Some(Outcome { changed: changed == 1, unread: unread == 1, refused: refused == 1, ..Outcome::default() })
    });
    if let Some(hit) = hit {
        if !(write && hit.changed) {
            return hit;
        }
    }
    FORMATTED.fetch_add(1, Ordering::Relaxed);
    let mut outcome = lay_out();
    if !write {
        outcome.text = None;
    }
    if !outcome.broken_build {
        let flags = [outcome.changed, outcome.unread, outcome.refused].map(u8::from);
        cache.put(&key, &[VERDICT, &flags].concat());
    }
    outcome
}

/// What the formatter could not read, said once per file.
fn report(unread: &[String], refused: &[String]) {
    for u in unread {
        println!("{u}: has a syntax error; what did not parse was left as it was written");
    }
    for r in refused {
        println!("{r}: does not parse, and was left exactly as it is");
    }
}

/// Every `.buri` file under `dir`, skipping the directories nothing in a
/// repository is written by hand into.
pub fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    walk(dir, out, false);
}

/// The same, and every `.proto` beside them: the whole set of files an
/// analysis reads.
///
/// Shared with the language server's fingerprint, so that what `buri format`
/// considers part of the repository and what an analysis is keyed on are one
/// list rather than two that can drift apart. A schema is on this list and not
/// on the one above because only one a rule's `inputs` lists is formatted.
pub fn collect_with_schemas(dir: &Path, out: &mut Vec<PathBuf>) {
    walk(dir, out, true);
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, schemas: bool) {
    if dir.is_file() {
        out.push(dir.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.filter_map(Result::ok) {
        let p = e.path();
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if p.is_dir() {
            walk(&p, out, schemas);
        } else if p.extension().is_some_and(|x| x == "buri" || (schemas && x == "proto")) {
            out.push(p);
        }
    }
}
