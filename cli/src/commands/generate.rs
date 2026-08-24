//! `buri gen`.
//!
//! Rewrites the seven fields that restate the sources — `sources`,
//! `proto_sources`, `dependencies`, `test.sources`, `test.dependencies`,
//! `testing.sources`, `testing.dependencies` — and no others. `tags`, `platforms`,
//! `timeout_seconds`, `visibility`, `outputs`, `test.data`, `test.platforms`,
//! and every comment come back saying exactly what they said — see
//! `crate::build::regenerate`, which does the rewriting.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "what was rewritten, and what is out of date under `--check`, is this \
              command's output; diagnostics still leave through `Session::emit`"
)]

use crate::build::session;
use crate::build::workspace::PkgId;
use crate::commands::arguments;

/// Rewrites the fields that restate the sources, and no others. `tags`,
/// `platforms`, `timeout_seconds`, `visibility`, `outputs`, `test.data`,
/// `test.platforms`, and every comment come back saying exactly what they
/// said.
pub fn cmd_gen(args: &arguments::Args) -> i32 {
    let (mut s, targets) = match session::open_and_resolve(&args.flags, &args.targets) {
        Ok(both) => both,
        Err(c) => return c as i32,
    };

    let mut stale = Vec::new();
    let mut packages: Vec<PkgId> = targets.iter().map(|t| t.pkg).collect();
    packages.sort();
    packages.dedup();

    for pkg in packages {
        match crate::build::regenerate::regenerate(&mut s, pkg) {
            Ok(Some(update)) => {
                stale.push(s.ws.pkg(pkg).path.clone());
                if !args.flags.check {
                    let path = s.ws.pkg(pkg).build_path.clone();
                    if std::fs::write(&path, &update.text).is_ok() {
                        println!("updated {}/BUILD.buri", s.ws.pkg(pkg).path);
                        for line in &update.summary {
                            println!("  {line}");
                        }
                    }
                }
            }
            Ok(None) => {}
            Err(d) => {
                s.emit(&d);
                return 1;
            }
        }
    }

    if args.flags.check {
        for p in &stale {
            println!("{p}/BUILD.buri is out of date");
        }
        return if stale.is_empty() { 0 } else { 1 };
    }
    0
}
