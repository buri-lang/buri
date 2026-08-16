//! What this toolchain's parser says about a list of sources, one line each.
//!
//! `editors/tree-sitter-buri/check.sh` asks this and compares the answer with
//! what tree-sitter's syntax tree has in it: a source the parser accepts must
//! have no `ERROR` and no `MISSING` node, and one it rejects must have one.
//! That is the half of the grammar's guarantee a `cargo test` cannot make,
//! because it needs the tree-sitter CLI and the toolchain may not depend on an
//! external tool.
//!
//! It is asked rather than recorded. A checked-in file of verdicts would be a
//! readout of what the toolchain does, and a readout goes stale the moment the
//! toolchain changes — which is exactly when the answer matters.
//!
//! ```text
//! cargo run -q -p buri --example parse_verdicts < paths
//! cargo run -q -p buri --example parse_verdicts -- --stdlib < paths
//! ```
//!
//! Paths arrive on stdin, one per line, and one line comes back for each in
//! the order it was given:
//!
//! ```text
//! parses   cli/tests/example/lib/money/cents.buri
//! rejects  cli/tests/reject/export_star/main.buri
//! ```
//!
//! `--stdlib` reads them as bundled standard library modules, where a `fn` may
//! be declared with no body — `parsing::parser::parse_stdlib` rather than
//! `parse`. Outside such a module that form parses and is then turned away by
//! a rule, so asking the wrong one of the two would report a rejection the
//! compiler never makes of these files.
//!
//! This is an example rather than a `buri` subcommand because it is a seam for
//! the repository's own tests, not a surface anybody should have to learn. If
//! it ever earns one, `buri parse` is the name.

use std::io::Read;
use std::path::Path;

fn main() {
    let stdlib = std::env::args().skip(1).any(|a| a == "--stdlib");

    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        eprintln!("error: could not read the list of paths from stdin");
        std::process::exit(2);
    }

    let mut unreadable = 0;
    let mut out = String::new();
    for line in input.lines() {
        let path = line.trim();
        if path.is_empty() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(Path::new(path)) else {
            eprintln!("error: {path} cannot be read");
            unreadable += 1;
            continue;
        };
        // A `SourceMap` per file: nothing here renders a diagnostic, and the
        // question is only whether there is one.
        let mut map = buri::diagnostics::SourceMap::new();
        let id = map.add(path.to_string(), std::path::PathBuf::from(path), text.clone());
        let parsed = if stdlib {
            buri::parsing::parser::parse_stdlib(&text, id)
        } else {
            buri::parsing::parser::parse(&text, id)
        };
        let verdict = if parsed.errors.is_empty() { "parses" } else { "rejects" };
        out.push_str(&format!("{verdict:<9}{path}\n"));
    }

    print!("{out}");
    if unreadable > 0 {
        std::process::exit(2);
    }
}
