//! `buri run`.
//!
//! Builds exactly one binary and executes it outside the sandbox, with the
//! real environment and the real filesystem. That is the point of the command:
//! it is the one that produces a program with authority.

use crate::build::actions;
use crate::build::session::open_or_exit;
use crate::build::workspace::RuleKind;
use crate::commands::arguments;

pub fn cmd_run(args: &arguments::Args) -> i32 {
    let mut s = match open_or_exit(&args.flags) {
        Ok(s) => s,
        Err(c) => return c as i32,
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
    let binaries: Vec<_> = targets.iter().copied().filter(|t| t.kind == RuleKind::Binary).collect();
    if binaries.len() != 1 {
        eprintln!(
            "error: `buri run` takes exactly one binary, and this matched {}",
            binaries.len()
        );
        return 2;
    }
    let target = binaries[0];
    let outputs = actions::selected_outputs(&s, target, &args.flags);
    // Builds it for the host configuration and executes it.
    let output = match outputs.iter().find(|o| {
        o.platform.as_ref().map(|p| p.value) == Some(crate::build::buildfile::Platform::Js)
    }) {
        Some(o) => o.clone(),
        None => {
            eprintln!("error: {} declares no JS output to run", s.ws.label(target));
            eprintln!("  = this toolchain emits JavaScript; add `{{ platform: JS }}` to outputs");
            return 2;
        }
    };

    let artifact = match actions::build_target(&mut s, target, &output, &args.flags) {
        Ok(a) => a,
        Err(diags) => {
            s.print(&diags);
            return 1;
        }
    };

    // Outside the sandbox, with the real environment and the real filesystem.
    // That is the point of `run`: it is the one command that produces a
    // program with authority.
    let status = std::process::Command::new(crate::commands::test::js_runtime())
        .arg(&artifact.path)
        .args(&args.passthrough)
        .status();
    match status {
        Ok(st) => st.code().unwrap_or(1),
        Err(e) => {
            eprintln!("error: cannot execute the artifact: {e}");
            eprintln!("  = `buri run` needs a JavaScript runtime; install bun, or set BURI_JS");
            2
        }
    }
}
