//! `buri gen`.
//!
//! Rewrites the seven fields that restate the sources — `sources`,
//! `proto_sources`, `dependencies`, `test.sources`, `test.dependencies`,
//! `testing.sources`, `testing.dependencies` — and no others. `tags`, `platforms`,
//! `timeout_seconds`, `visibility`, `outputs`, `test.data`, `test.platforms`,
//! and every comment come back saying exactly what they said — see
//! `crate::build::regenerate`, which does the rewriting.
//!
//! With no target argument it regenerates the whole repository, where `build`,
//! `test` and `lint` with no argument mean the package containing the working
//! directory. This command does not answer a question about the code in front
//! of you; it restates what the tree contains, and a tree restated one
//! directory at a time is one where `gen --check` passes where you are
//! standing and fails one directory over. It is the default `buri format`
//! already has, and the two commands are meant to agree about a file.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "what was rewritten, and what is out of date under `--check`, is this \
              command's output; diagnostics still leave through `Session::emit`"
)]

use crate::build::session;
use crate::build::workspace::PackageId;
use crate::commands::arguments;

/// Rewrites the fields that restate the sources, and no others. `tags`,
/// `platforms`, `timeout_seconds`, `visibility`, `outputs`, `test.data`,
/// `test.platforms`, and every comment come back saying exactly what they
/// said.
pub fn command_generate(args: &arguments::Args) -> i32 {
    // `buri gen` with no argument is `buri gen //...`, rather than the working
    // directory's package: the module note above says why.
    let whole_repository = [String::from("//...")];
    let requested: &[String] =
        if args.targets.is_empty() { &whole_repository } else { &args.targets };
    let (mut session, targets) = match session::open_and_resolve(&args.flags, requested) {
        Ok(both) => both,
        Err(c) => return c as i32,
    };

    let mut stale = Vec::new();
    let mut packages: Vec<PackageId> = targets.iter().map(|t| t.package).collect();
    packages.sort();
    packages.dedup();

    for package in packages {
        match crate::build::regenerate::regenerate(&mut session, package) {
            Ok(Some(update)) => {
                stale.push(session.workspace.package(package).path.clone());
                if !args.flags.check {
                    let path = session.workspace.package(package).build_path.clone();
                    if std::fs::write(&path, &update.text).is_ok() {
                        println!("updated {}/BUILD.buri", session.workspace.package(package).path);
                        for line in &update.summary {
                            println!("  {line}");
                        }
                    }
                }
            }
            Ok(None) => {}
            Err(d) => {
                session.emit(&d);
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
