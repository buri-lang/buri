//! `buri gen`.
//!
//! Rewrites the fields that restate the sources, and no others. `tags`,
//! `platforms`, `timeout_seconds`, `visibility`, `outputs`, `test.data`, and
//! every comment come back saying exactly what they said — see
//! `crate::build::regenerate`, which does the rewriting.

use crate::build::session;
use crate::build::workspace::PkgId;
use crate::commands::arguments;

/// Rewrites the fields that restate the sources, and no others. `tags`,
/// `platforms`, `timeout_seconds`, `visibility`, `outputs`, `test.data`, and
/// every comment come back saying exactly what they said.
pub fn cmd_gen(args: &arguments::Args) -> i32 {
    let mut s = match session::open(&args.flags) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };
    if s.report() {
        return 2;
    }
    let targets = match s.resolve_targets(&args.targets) {
        Ok(t) => t,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
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
