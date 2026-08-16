//! `buri version`.
//!
//! Prints the toolchain's version and what `REPO.buri` pins, and disagreeing
//! with the pin is an error: an exact version, never a range, because two
//! checkouts of one commit must not build with two different compilers.

use crate::build::session;
use crate::commands::arguments;

pub fn cmd_version(args: &arguments::Args) -> i32 {
    println!("buri {}", arguments::VERSION);
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
                if t.version != arguments::VERSION {
                    eprintln!(
                        "error: this repository pins {} but this toolchain is {}",
                        t.version,
                        arguments::VERSION
                    );
                    eprintln!("  = an exact version, never a range: two checkouts of the same commit must not build with two different compilers");
                    return 2;
                }
            }
            0
        }
        // Outside a repository there is nothing to pin against, which is fine.
        Err(_) => 0,
    }
}
