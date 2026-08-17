//! `buri clean`.
//!
//! Reaching for this to fix a build is worth noticing rather than automating:
//! the cache is keyed on content, so a stale entry is a bug in the key, not a
//! fact of life.
#![allow(
    clippy::print_stdout,
    reason = "what was dropped is this command's output; diagnostics still leave \
              through `Session::emit`"
)]

use crate::build::session::open_or_exit;
use crate::commands::arguments;

pub fn cmd_clean(args: &arguments::Args) -> i32 {
    let s = match open_or_exit(&args.flags) {
        Ok(s) => s,
        Err(c) => return c as i32,
    };
    let mut removed = Vec::new();
    let out = s.root.join(".buri/out");
    if out.exists() {
        let _ = std::fs::remove_dir_all(&out);
        removed.push(".buri/out");
    }
    if !args.flags.outputs_only {
        let cache = s.root.join(".buri/cache");
        if cache.exists() {
            let _ = std::fs::remove_dir_all(&cache);
            removed.push(".buri/cache");
        }
        // The link directories hold the objects a native link was run over,
        // copied out of the cache under stable filenames. They go with the
        // cache rather than with the outputs, because that is what they are
        // (ARCHITECTURE.md §6.3), and they are named only when there are some:
        // a JavaScript-only repository has never had one.
        let links = s.root.join(".buri/link");
        if links.exists() {
            let _ = std::fs::remove_dir_all(&links);
            removed.push(".buri/link");
        }
    }
    let _ = std::fs::remove_file(s.root.join("out"));
    if removed.is_empty() {
        println!("nothing to clean");
    } else {
        println!("dropped {}", removed.join(" and "));
    }
    // Reaching for `buri clean` to fix a build is worth reporting: the cache is
    // keyed on content, so a stale entry is a bug rather than a fact of life.
    0
}
