//! `buri run`.
//!
//! Builds exactly one binary and executes it outside the sandbox, with the
//! real environment and the real filesystem. That is the point of the command:
//! it is the one that produces a program with authority.
#![allow(
    clippy::print_stderr,
    reason = "a `buri run` that has nothing to run is a complaint about the \
              invocation, and this command's own output; diagnostics still leave \
              through `Session::emit`"
)]

use crate::build::actions;
use crate::build::session;
use crate::build::workspace::RuleKind;
use crate::commands::arguments;

pub fn cmd_run(args: &arguments::Args) -> i32 {
    let (mut s, targets) = match session::open_and_resolve(&args.flags, &args.targets) {
        Ok(both) => both,
        Err(c) => return c as i32,
    };
    let binaries: Vec<_> = targets.iter().copied().filter(|t| t.kind == RuleKind::Binary).collect();
    let &[target] = binaries.as_slice() else {
        eprintln!(
            "error: `buri run` takes exactly one binary, and this matched {}",
            binaries.len()
        );
        return 2;
    };
    let outputs = actions::selected_outputs(&s, target, &args.flags);
    let Some(output) = choose(&outputs, &args.flags) else {
        let declared: Vec<String> = outputs.iter().map(crate::build::buildfile::Output::dir).collect();
        eprintln!("error: {} declares no output this toolchain can run", s.ws.label(target));
        if declared.is_empty() {
            eprintln!("  = it declares no outputs at all");
        } else {
            eprintln!("  = declared: {}", declared.join(", "));
        }
        eprintln!(
            "  = fix: add `{{ platform: JS }}` to outputs, or declare the host's platform and \
             build a toolchain with a native backend"
        );
        return 2;
    };
    // What follows this asks whether the artifact is a process or a module a
    // JavaScript runtime is handed, and a WEB artifact is the latter: it runs
    // headlessly under `bun` and `node`, which is what makes `buri run` on a
    // page mean something rather than being refused.
    let native = output.platform().is_native();

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
    //
    // A native artifact *is* the process; a JavaScript one is a module a
    // runtime has to be handed. That is the whole difference between the two
    // here, and it is one word of the command line.
    let mut command = if native {
        std::process::Command::new(&artifact.path)
    } else {
        let mut c = std::process::Command::new(crate::commands::test::js_runtime());
        c.arg(&artifact.path);
        c
    };
    match command.args(&args.passthrough).status() {
        Ok(st) => st.code().unwrap_or(1),
        Err(e) => {
            eprintln!("error: cannot execute the artifact: {e}");
            if !native {
                eprintln!("  = `buri run` needs a JavaScript runtime; install bun, or set BURI_JS");
            }
            2
        }
    }
}

/// Which of a target's outputs `buri run` executes.
///
/// The host's own platform first, where this toolchain can build and link for
/// it, and JavaScript otherwise. That order is the `host_platform()` switch
/// applied to the one command whose answer a user watches
/// (`design/native/ARCHITECTURE.md` §4): a repository that declares a native
/// output for this machine gets a native process, and one that declares only
/// `JS` gets what it always got.
///
/// It is a *preference among declared outputs* rather than a default, so
/// nothing here invents an output a rule did not ask for — `selected_outputs`
/// is what supplies one for a binary that declares none, and `--output` has
/// already filtered this list by the time it arrives.
fn choose(
    outputs: &[crate::build::buildfile::Output],
    flags: &crate::commands::arguments::Flags,
) -> Option<crate::build::buildfile::Output> {
    let host = crate::compiler::driver::host_native_platform();
    let runnable = |o: &&crate::build::buildfile::Output| {
        actions::native_ready(actions::target_of(o), actions::profile_of(flags))
    };
    outputs
        .iter()
        .find(|o| o.platform() == host && runnable(o))
        // Any JavaScript output is runnable here, `WEB` included: the runtime
        // supplies a document where there is none, so a page runs to its first
        // paint and prints whatever `main` printed.
        .or_else(|| outputs.iter().find(|o| o.platform().is_javascript()))
        // A target that declares only an output this toolchain cannot produce
        // is built anyway, so that the refusal is the build's — which names the
        // platform, the backend and the feature — rather than a sentence this
        // command invented about outputs it can see.
        .or_else(|| outputs.first())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::buildfile::{Output, Platform};
    use crate::diagnostics::Span;

    /// What `buri run` picks, in the two states a toolchain can be in.
    ///
    /// The second half is the promise this wave owes every existing golden
    /// file: where a native artifact cannot be produced, the command chooses
    /// exactly what it chose before, which is the JavaScript output.
    #[test]
    fn a_native_output_is_preferred_only_where_it_can_be_produced() {
        let flags = crate::commands::arguments::Flags::default();
        let host = crate::compiler::driver::host_native_platform();
        let both = [Output::js(Span::NONE), Output::for_platform(host, Span::NONE)];
        let ready = actions::native_ready(
            actions::target_of(&Output::for_platform(host, Span::NONE)),
            actions::profile_of(&flags),
        );
        let picked = choose(&both, &flags).map(|o| o.platform());
        assert_eq!(picked, Some(if ready { host } else { Platform::Js }));

        // JavaScript alone is JavaScript, whatever this toolchain can do.
        assert_eq!(
            choose(&[Output::js(Span::NONE)], &flags).map(|o| o.platform()),
            Some(Platform::Js)
        );
        // And nothing declared is nothing to run, which is the caller's to
        // report rather than something to invent an output for.
        assert!(choose(&[], &flags).is_none());
    }

    /// A target that declares only what this toolchain cannot produce is still
    /// handed to the build, so that the refusal names the platform and the
    /// feature rather than the outputs this command could see.
    #[test]
    fn an_unbuildable_output_is_chosen_so_the_build_can_refuse_it() {
        let flags = crate::commands::arguments::Flags::default();
        let cross = if crate::compiler::driver::host_native_platform() == Platform::Macos {
            Platform::Linux
        } else {
            Platform::Macos
        };
        let only = [Output::for_platform(cross, Span::NONE)];
        assert_eq!(choose(&only, &flags).map(|o| o.platform()), Some(cross));
    }
}
