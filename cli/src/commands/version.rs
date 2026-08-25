//! `buri version`.
//!
//! Prints the toolchain's version, and under `--verbose` the hash of the
//! executable printing it — which build of that version this is, for a bug
//! report that has to name one.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "the version is this command's output, and `--self-check` \
              renders the standard library's own diagnostics before any session \
              exists to emit them through"
)]

use crate::build::cache::hash_bytes;
use crate::commands::arguments;

/// The SHA-256 of the running executable. Two builds of one version are two
/// compilers, and this is the only way to tell which one is here.
fn running_sha256() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(hash_bytes(&std::fs::read(exe).ok()?))
}

/// Answered entirely from the binary. It opens no repository: the version is a
/// fact about the executable, and a command that reported one only where a
/// `REPO.buri` parsed would be a command that fails when it has nothing to say.
/// This used to open a session, because `REPO.buri` pinned a toolchain and
/// `version` was the command whose job was to report the pin; the pin is gone.
pub fn command_version(args: &arguments::Args) -> i32 {
    println!("buri {}", arguments::VERSION);
    if args.flags.verbose {
        println!(
            "this executable: sha256 {}",
            running_sha256().unwrap_or_else(|| "unreadable".into())
        );
    }
    if args.flags.self_check {
        let mut map = crate::diagnostics::SourceMap::new();
        let analysis = crate::compiler::driver::analyze_stdlib(&mut map);
        for d in &analysis.diagnostics.items {
            eprint!("{}", map.render(d, false));
        }
        if !analysis.diagnostics.items.is_empty() {
            eprintln!("the bundled standard library does not check");
            return 1;
        }
        println!("standard library: {} modules, checked", analysis.loaded.modules.len());
    }
    0
}
