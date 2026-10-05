//! `buri clean`.
//!
//! Reaching for this to fix a build is worth noticing rather than automating:
//! the cache is keyed on what an action read, so a stale entry is a bug in the
//! key rather than a fact of life. *Where* it read it is in the key too —
//! `build/sources.rs::listing` hashes the root and every absolute path — so
//! renaming a checkout leaves the old entries unreachable, and this is what
//! removes them.
#![allow(
    clippy::print_stdout,
    reason = "what was dropped is this command's output; diagnostics still leave \
              through `Session::emit`"
)]

use crate::build::session::root_of_cwd;
use crate::commands::arguments;

#[expect(
    clippy::print_stderr,
    reason = "a missing repository is reported before there is a session to emit through"
)]
pub fn command_clean(args: &arguments::Args) -> i32 {
    // The root and nothing else. Opening a session would load the graph and
    // run every generator, building their tools, only for the next lines to
    // delete what that wrote.
    let root = match root_of_cwd() {
        Ok(root) => root,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };
    let mut removed = Vec::new();
    let out = root.join(".buri/out");
    if out.exists() {
        let _ = std::fs::remove_dir_all(&out);
        removed.push(".buri/out");
    }
    if !args.flags.outputs_only {
        let cache = root.join(".buri/cache");
        if cache.exists() {
            let _ = std::fs::remove_dir_all(&cache);
            removed.push(".buri/cache");
        }
        // The link directories hold the objects a native link was run over,
        // copied out of the cache under stable filenames. They go with the
        // cache rather than with the outputs, because that is what they are
        // (ARCHITECTURE.md §6.3), and they are named only when there are some:
        // a JavaScript-only repository has never had one.
        let links = root.join(".buri/link");
        if links.exists() {
            let _ = std::fs::remove_dir_all(&links);
            removed.push(".buri/link");
        }
    }
    let _ = std::fs::remove_file(root.join("out"));
    if removed.is_empty() {
        println!("nothing to clean");
    } else {
        println!("dropped {}", removed.join(" and "));
    }
    // Reaching for `buri clean` to fix a build is worth reporting: the cache is
    // keyed on what was read, so a stale entry is a bug rather than a fact of
    // life. A renamed checkout is the one honest reason to run this.
    0
}
