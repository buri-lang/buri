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
/// there is none — the file does not parse, and a file that does not parse is
/// left exactly as it is.
///
/// One function, so that the command, the language server and the tests cannot
/// disagree about which printer a file goes through.
pub fn file(name: &str, text: &str) -> Option<String> {
    if is_build_file(name) {
        let parsed = textproto::parse(text, crate::diagnostics::FileId(0));
        if !parsed.errors.is_empty() {
            return None;
        }
        return Some(textproto::print(&parsed.document));
    }
    crate::formatting::source(text)
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
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        let Some(formatted) = file(&name, &text) else {
            // A build file that does not read is a hard error, because nothing
            // else in the repository will work until it is fixed. Source that
            // does not parse is left exactly as it is: it is being edited.
            if is_build_file(&name) {
                eprintln!("error: {} does not parse", session.workspace.rel_of(path));
                return 2;
            }
            continue;
        };
        if formatted != text {
            changed.push(session.workspace.rel_of(path));
            if !args.flags.check {
                let _ = std::fs::write(path, formatted);
            }
        }
    }

    if args.flags.check {
        // The CI form: exit non-zero on any file that would change.
        for c in &changed {
            println!("{c}");
        }
        return if changed.is_empty() { 0 } else { 1 };
    }
    for c in &changed {
        println!("formatted {c}");
    }
    0
}

/// Every `.buri` file under `dir`, skipping the directories nothing in a
/// repository is written by hand into.
///
/// Shared with the language server's fingerprint, so that what `buri format`
/// considers part of the repository and what an analysis is keyed on are one
/// list rather than two that can drift apart.
pub fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
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
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "buri") {
            out.push(p);
        }
    }
}
