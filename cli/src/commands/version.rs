//! `buri version`.
//!
//! Prints the toolchain's version, and under `--verbose` the identity of the
//! executable printing it — which build of that version this is, for a bug
//! report that has to name one.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "the version and the check are this command's output"
)]

use crate::build::cache::running_exe_identity;
use crate::commands::arguments;

/// Answered entirely from the binary. It opens no repository: the version is a
/// fact about the executable, and a command that reported one only where a
/// `REPO.buri` parsed would be a command that fails when it has nothing to say.
/// This used to open a session, because `REPO.buri` pinned a toolchain and
/// `version` was the command whose job was to report the pin; the pin is gone.
pub fn command_version(args: &arguments::Args) -> i32 {
    println!("buri {}", arguments::VERSION);
    if args.flags.verbose {
        // The same identity the cache key folds in ([`build::cache`]), which
        // is what tells two builds of one version apart. `unreadable` where
        // the binary can't be read, so the report says so rather than a lie.
        println!(
            "this executable: {}",
            running_exe_identity().unwrap_or("unreadable")
        );
    }
    if args.flags.self_check {
        let mut map = crate::diagnostics::SourceMap::new();
        let analysis = crate::compiler::driver::analyze_stdlib(&mut map);
        let problems = &analysis.diagnostics.items;
        for d in problems {
            eprint!("{}", map.render(d, false));
        }
        println!(
            "standard library: {} modules checked, {} problems",
            analysis.loaded.modules.len(),
            problems.len()
        );
        return i32::from(!problems.is_empty());
    }
    0
}
