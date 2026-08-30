//! `buri format`.
//!
//! No options and no configuration file: a formatter with options is a
//! formatter whose output is a repository decision. This file is the command —
//! finding the files and writing them back. The printing itself is
//! `crate::formatting`, which the documentation renderer and `buri gen` use
//! too.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "the list of files it formatted, or would, is this command's output; \
              diagnostics still leave through `Session::emit`"
)]

use crate::build::session;
use crate::build::textproto;
use crate::commands::arguments;
use std::path::{Path, PathBuf};

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
    if is_build_file(name) {
        let parsed = textproto::parse(text, crate::diagnostics::FileId(0));
        if !parsed.errors.is_empty() {
            return None;
        }
        let text = textproto::print(&parsed.document);
        return Some(crate::formatting::Formatted { text, regions: Vec::new() });
    }
    crate::formatting::source_with_regions(text)
}

/// Formats `.buri` sources and build files, with no options and no
/// configuration file. A formatter with options is a formatter whose output is
/// a repository decision.
pub fn command_format(args: &arguments::Args) -> i32 {
    let session = match session::open_or_exit(&args.flags) {
        Ok(session) => session,
        Err(c) => return c as i32,
    };
    let roots: Vec<PathBuf> = if args.targets.is_empty() {
        vec![session.root.clone()]
    } else {
        args.targets.iter().map(|t| session.root.join(t.trim_start_matches("//"))).collect()
    };

    let mut files = Vec::new();
    for r in &roots {
        collect(r, &mut files);
    }
    files.sort();

    let mut changed = Vec::new();
    // The files a syntax error kept part or all of out of the formatter's
    // hands. They are named rather than skipped in silence: a file the
    // formatter could not read whole is not a file it has checked, and a
    // `--check` that passed one would be reporting a gate it did not run.
    let mut unread = Vec::new();
    let mut refused = Vec::new();
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        let Some(out) = formatted(&name, &text) else {
            // A build file that does not read is a hard error, because nothing
            // else in the repository will work until it is fixed.
            if is_build_file(&name) {
                eprintln!("error: {} does not parse", session.workspace.rel_of(path));
                return 2;
            }
            refused.push(session.workspace.rel_of(path));
            continue;
        };
        if !out.regions.is_empty() {
            unread.push(session.workspace.rel_of(path));
        }
        if out.text != text {
            changed.push(session.workspace.rel_of(path));
            if !args.flags.check {
                let _ = std::fs::write(path, out.text);
            }
        }
    }

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
/// on the one above because it is compiled and not formatted.
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
