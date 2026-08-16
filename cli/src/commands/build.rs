//! `buri build`.
//!
//! A binary is built; a library is checked. `buri build //lib/money` produces
//! no artifact and is not meant to — the library's outputs are whatever
//! depends on it — so what it reports is whether the code and the policy
//! around it hold.

use crate::build::actions;
use crate::build::session::open_or_exit;
use crate::build::workspace::RuleKind;
use crate::commands::arguments;

pub fn cmd_build(args: &arguments::Args) -> i32 {
    let mut s = match open_or_exit(&args.flags) {
        Ok(s) => s,
        Err(c) => return c as i32,
    };
    // An unparseable build file is a problem with the invocation, not with the
    // code, so it exits 2.
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

    let mut built = 0;
    let mut failed = false;
    for target in targets {
        // Only a binary produces an artifact; a library is checked, which is
        // what `buri build //lib/money` means.
        if target.kind == RuleKind::Library {
            let mut diags = crate::diagnostics::Diagnostics::new();
            actions::check_policy(&s, target, crate::build::buildfile::Platform::Js, &mut diags);
            if !diags.has_errors() {
                let unit = crate::compiler::modules::Unit {
                    target: Some(target),
                    platform: crate::build::buildfile::Platform::Js,
                    with_tests: false,
                };
                let analysis = crate::compiler::driver::analyze(Some(&s.ws), &mut s.map, &unit);
                diags.extend(analysis.diags.items);
            }
            failed |= s.print(&diags);
            continue;
        }
        for output in actions::selected_outputs(&s, target, &args.flags) {
            match actions::build_target(&mut s, target, &output, &args.flags) {
                Ok(a) => {
                    built += 1;
                    let rel = a.path.strip_prefix(&s.root).unwrap_or(&a.path);
                    let note = if a.cached { ", cached" } else { "" };
                    println!("{} ({} bytes{note})", rel.display(), a.bytes);
                }
                Err(diags) => {
                    failed |= s.print(&diags);
                }
            }
        }
    }
    if failed {
        return 1;
    }
    if built == 0 && args.flags.verbose {
        println!("nothing to build");
    }
    0
}
