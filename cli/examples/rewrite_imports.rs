//! The one-off that migrated every import in this repository to name a file.
//!
//! It is here for the length of the flag-day and is deleted in the commit that
//! lands the rewrite. What it does is not a text substitution: for every file
//! it finds the repository that file lives in, loads that repository's
//! `Workspace`, and asks the *resolver* what the import already resolved to —
//! `Workspace::file_form_of`. `//lib/money/cents` is a module in one fixture
//! repository and a library's surface in another, and no amount of reading the
//! string can tell those apart.
//!
//! ```text
//! cargo run -q -p buri --example rewrite_imports -- <files-from-stdin
//! cargo run -q -p buri --example rewrite_imports -- --dry-run <files
//! ```
//!
//! Most of the tree, though, is fixtures that stand outside any repository:
//! the formatter's cases, the mutation corpora, the benchmark's saved programs,
//! the documentation's fences. For those it consults `--table`, a file of
//! `old new` pairs **produced by the resolved pass over the repositories that
//! do exist** — so even the orphans are answered by resolution, just somebody
//! else's. A path the table maps two ways is not in it.
//!
//! Paths arrive on stdin, one per line, relative to the current directory.
//! Every rewrite is printed to stdout as `file:line old -> new`, which is the
//! resolution-identity record: it is the resolver's own answer, so a spot check
//! of that log is a check that the file an import named did not move.
//!
//! Anything it cannot map is printed to stderr and it exits 1, so the residue
//! is a list somebody read rather than a silent skip.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects,
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a one-off migration tool, deleted in the commit that lands the \
              rewrite. The lint set in `Cargo.toml` pins a promise about the \
              toolchain, and a script that edits the toolchain's own fixtures \
              is not the toolchain."
)]

use buri::build::workspace::Workspace;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Every `from "<path>" import|export` occurrence in a line, as byte ranges of
/// the path itself — the run between the quotes, whichever way the quotes are
/// written.
///
/// Two spellings, because the same import line lives in a `.buri` source, in a
/// Rust string literal (`from \"core/list\" import ...`), and in a JSON golden.
fn occurrences(line: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while let Some(at) = line[i..].find("from") {
        let start = i + at;
        i = start + 4;
        // `from` must be a word, and what follows it a quote after some space.
        // A source line quoted inside a JSON golden or a Rust string arrives as
        // `\nfrom \"...\" import`, so the `n` of an escape sequence is a word
        // boundary and not a letter.
        if start > 0 {
            let prev = bytes[start - 1];
            let escape = start >= 2
                && bytes[start - 2] == b'\\'
                && matches!(prev, b'n' | b't' | b'r');
            if !escape && (prev.is_ascii_alphanumeric() || prev == b'_') {
                continue;
            }
        }
        let mut j = start + 4;
        let mut spaced = false;
        while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
            j += 1;
            spaced = true;
        }
        if !spaced {
            continue;
        }
        // Either `"` or the escaped `\"`.
        let escaped = j + 1 < bytes.len() && bytes[j] == b'\\' && bytes[j + 1] == b'"';
        if escaped {
            j += 1;
        } else if bytes.get(j) != Some(&b'"') {
            continue;
        }
        let path_start = j + 1;
        let closer = if escaped { "\\\"" } else { "\"" };
        let Some(rel_end) = line[path_start..].find(closer) else { continue };
        let path_end = path_start + rel_end;
        let after = &line[path_end + closer.len()..];
        let after = after.trim_start();
        if !(after.starts_with("import") || after.starts_with("export")) {
            continue;
        }
        out.push((path_start, path_end));
        i = path_end;
    }
    out
}

/// The repository root a file sits in, if any, stopping at `stop`.
fn root_of(file: &Path, stop: &Path) -> Option<PathBuf> {
    let mut dir = file.parent()?.to_path_buf();
    loop {
        if dir.join("REPO.buri").is_file() {
            return Some(dir);
        }
        if dir == stop || !dir.pop() {
            return None;
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let mut table: HashMap<String, String> = HashMap::new();
    if let Some(at) = args.iter().position(|a| a == "--table") {
        let file = args.get(at + 1).expect("--table wants a file");
        let text = std::fs::read_to_string(file).expect("table");
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            if let (Some(old), Some(new)) = (parts.next(), parts.next()) {
                table.insert(old.to_string(), new.to_string());
            }
        }
    }

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).expect("stdin");
    let cwd = std::env::current_dir().expect("cwd");

    let mut workspaces: HashMap<PathBuf, Option<Workspace>> = HashMap::new();
    let mut unmapped: Vec<String> = Vec::new();
    let mut rewritten = 0usize;
    let mut touched = 0usize;

    for name in input.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let path = cwd.join(name);
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let root = root_of(&path, &cwd);
        let ws = match &root {
            Some(r) => {
                let entry = workspaces.entry(r.clone()).or_insert_with(|| {
                    let mut map = buri::diagnostics::SourceMap::new();
                    let mut diags = buri::diagnostics::Diagnostics::new();
                    Workspace::load(r, &mut map, &mut diags).ok()
                });
                entry.as_ref()
            }
            None => None,
        };

        let mut out = String::with_capacity(text.len());
        let mut changed = false;
        for (number, line) in text.split_inclusive('\n').enumerate() {
            let spans = occurrences(line);
            if spans.is_empty() {
                out.push_str(line);
                continue;
            }
            let mut cursor = 0usize;
            let mut rebuilt = String::new();
            for (a, b) in spans {
                let old = &line[a..b];
                rebuilt.push_str(&line[cursor..a]);
                cursor = b;
                // A path that already names a file, a relative path, and
                // anything that is not a module path at all are all left alone.
                if old.ends_with(".buri")
                    || old.ends_with(".proto")
                    || old.starts_with('.')
                    || old.contains('\\')
                    || !(old.starts_with("//")
                        || buri::compiler::standard_library::is_std_path(old))
                {
                    rebuilt.push_str(old);
                    continue;
                }
                let derived = ws
                    .and_then(|w| w.file_form_of(old))
                    .or_else(|| {
                        // The standard library has no directories on disk, so
                        // its rule is total and needs no evidence: a module is
                        // its root plus `/lib.buri`, whether or not the module
                        // exists. `core/hash` is a deliberate near miss in a
                        // `no-such-module` fixture, and it has to stay one.
                        buri::compiler::standard_library::is_std_path(old)
                            .then(|| format!("{old}/lib.buri"))
                    })
                    .or_else(|| table.get(old).cloned());
                match derived {
                    Some(new) => {
                        println!("{name}:{} {old} -> {new}", number + 1);
                        rebuilt.push_str(&new);
                        rewritten += 1;
                        changed = true;
                    }
                    None => {
                        unmapped.push(format!("{name}:{} {old}", number + 1));
                        rebuilt.push_str(old);
                    }
                }
            }
            rebuilt.push_str(&line[cursor..]);
            out.push_str(&rebuilt);
        }
        if changed {
            touched += 1;
            if !dry_run {
                std::fs::write(&path, &out).expect("write");
            }
        }
    }

    eprintln!("rewrote {rewritten} imports in {touched} files");
    if !unmapped.is_empty() {
        eprintln!("could not map {} imports:", unmapped.len());
        for u in &unmapped {
            eprintln!("  {u}");
        }
        std::process::exit(1);
    }
}
