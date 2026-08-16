//! `buri version`.
//!
//! Prints the toolchain's version and what `REPO.buri` pins, and disagreeing
//! with the pin is an error: an exact version, never a range, because two
//! checkouts of one commit must not build with two different compilers.

use crate::build::session;
use crate::commands::arguments;

pub fn cmd_version(args: &arguments::Args) -> i32 {
    println!("buri {}", arguments::VERSION);
    // The hash a `REPO.buri` would have to write to pin this executable. There
    // is no other way to learn it, and the pin is the one field somebody has to
    // fill in by hand.
    if args.flags.verbose {
        println!(
            "this executable: sha256 {}",
            crate::build::toolchain::running_sha256().unwrap_or_else(|| "unreadable".into())
        );
    }
    if args.flags.self_check {
        let mut map = crate::diagnostics::SourceMap::new();
        let analysis = crate::compiler::driver::analyze_stdlib(&mut map);
        let mut errors = 0;
        for d in &analysis.diags.items {
            eprint!("{}", map.render(d, false));
            errors += 1;
        }
        if errors > 0 {
            eprintln!("the bundled standard library does not check");
            return 1;
        }
        println!("standard library: {} modules, checked", analysis.loaded.modules.len());
    }
    match session::open(&args.flags) {
        Ok(s) => {
            let t = &s.ws.repo.toolchain;
            if t.version.is_empty() {
                println!("REPO.buri pins no toolchain version");
            } else {
                println!("REPO.buri pins {}", t.version);
            }
            // Saying whether the pin is a pin, because "unpinned" is the state
            // a repository is in for as long as its toolchain has no published
            // release, and a reader should not have to know the sentinel to
            // find that out.
            if crate::build::toolchain::is_pinned(&t.sha256) {
                println!("REPO.buri pins sha256 {}", t.sha256);
            } else if !t.version.is_empty() {
                println!("REPO.buri pins no toolchain sha256 (unpinned)");
            }
            0
        }
        // Outside a repository there is nothing to pin against, which is fine.
        // A pin that this toolchain fails is not: `session::open` has already
        // decided that, and `version` is the command whose whole job is to
        // report the pin, so it must not be the one that swallows the refusal.
        Err(msg) if msg.starts_with("not in a Buri repository") => 0,
        Err(msg) => {
            eprintln!("error: {msg}");
            2
        }
    }
}
