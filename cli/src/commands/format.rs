//! `buri format`.
//!
//! No options and no configuration file: a formatter with options is a
//! formatter whose output is a repository decision. This file is the command —
//! finding the files and writing them back. The printing itself is
//! `crate::formatting`, which the documentation renderer and `buri gen` use
//! too.

use crate::build::session;
use crate::build::textproto;
use crate::commands::arguments;
use std::path::{Path, PathBuf};

/// Formats `.buri` sources and `BUILD.buri` files, with no options and no
/// configuration file. A formatter with options is a formatter whose output is
/// a repository decision.
pub fn cmd_format(args: &arguments::Args) -> i32 {
    let s = match session::open(&args.flags) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };
    let roots: Vec<PathBuf> = if args.targets.is_empty() {
        vec![s.root.clone()]
    } else {
        args.targets.iter().map(|t| s.root.join(t.trim_start_matches("//"))).collect()
    };

    let mut files = Vec::new();
    for r in &roots {
        collect(r, &mut files);
    }
    files.sort();

    let mut changed = Vec::new();
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let formatted = if name == "BUILD.buri" || name == "REPO.buri" {
            let parsed = textproto::parse(&text, crate::diagnostics::FileId(0));
            if !parsed.errors.is_empty() {
                eprintln!("error: {} does not parse", s.ws.rel_of(path));
                return 2;
            }
            textproto::print(&parsed.doc)
        } else {
            match crate::formatting::source(&text) {
                Some(f) => f,
                None => {
                    // A file that does not parse is left exactly as it is.
                    continue;
                }
            }
        };
        if formatted != text {
            changed.push(s.ws.rel_of(path));
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

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
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
