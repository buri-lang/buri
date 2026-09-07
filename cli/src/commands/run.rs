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
use crate::build::buildfile::Platform;
use crate::build::session;
use crate::build::workspace::RuleKind;
use crate::commands::arguments;

pub fn command_run(args: &arguments::Args) -> i32 {
    let (mut session, targets) = match session::open_and_resolve(&args.flags, &args.targets) {
        Ok(both) => both,
        Err(c) => return c as i32,
    };
    let binaries: Vec<_> = targets.iter().copied().filter(|t| t.kind == RuleKind::Binary).collect();
    let &[target] = binaries.as_slice() else {
        eprintln!(
            "error: `buri run` takes exactly one binary, and this matched {}",
            binaries.len()
        );
        // With no argument the match is the whole repository, so the several it
        // found are the choice the user has to make; naming them is the fix.
        if let Some(first) = binaries.first() {
            let labels: Vec<String> =
                binaries.iter().map(|&t| session.workspace.label(t)).collect();
            eprintln!("  = matched: {}", labels.join(", "));
            eprintln!("  = fix: name one, as in `buri run {}`", session.workspace.label(*first));
        } else {
            eprintln!("  = fix: name a package that declares a binary, as in `buri run //cmd/app`");
        }
        return 2;
    };
    let outputs = actions::selected_outputs(&session, target, &args.flags);
    let Some(output) = choose(&outputs, &args.flags) else {
        let declared: Vec<String> = outputs.iter().map(crate::build::buildfile::Output::dir).collect();
        eprintln!(
            "error: {} declares no output this toolchain can run",
            session.workspace.label(target)
        );
        if declared.is_empty() {
            eprintln!("  = it declares no outputs at all");
        } else {
            eprintln!("  = declared: {}", declared.join(", "));
        }
        // The one refusal here that is about the *kind* of artifact rather than
        // about this toolchain: a worker is called by its platform, once per
        // request, so a build makes it and nothing starts it.
        if !outputs.is_empty()
            && outputs.iter().all(|o| o.platform() == Platform::CloudflareWorker)
        {
            eprintln!(
                "  = a worker is called by its platform, once per request, so there is nothing \
                 to start"
            );
            eprintln!(
                "  = fix: build it with `buri build {}`, and let the platform call it",
                session.workspace.label(target)
            );
            return 2;
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

    let artifact = match actions::build_target(&mut session, target, &output, &args.flags) {
        Ok(a) => a,
        Err(diagnostics) => {
            session.print(&diagnostics);
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
        //
        // A worker is the exception, and it is not about JavaScript. `run`
        // starts a program; a worker is *called* by its platform, once per
        // request, so there is nothing for this command to start. A binary
        // that declares a page and a worker runs the page.
        .or_else(|| {
            outputs.iter().find(|o| {
                o.platform().is_javascript() && o.platform() != Platform::CloudflareWorker
            })
        })
        // A target that declares only an output this toolchain cannot produce
        // is built anyway, so that the refusal is the build's — which names the
        // platform, the backend and the feature — rather than a sentence this
        // command invented about outputs it can see. A worker is not in that
        // set: there is nothing to start, whatever this toolchain can build, so
        // a binary that declares one and nothing else has nothing to run.
        .or_else(|| outputs.iter().find(|o| o.platform() != Platform::CloudflareWorker))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::buildfile::{Output, Platform};
    use crate::diagnostics::Span;

    /// What `buri run` picks, in the two states a toolchain can be in.
    ///
    /// The second half is the promise this command owes every existing golden
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

    /// A worker is never what `run` starts.
    ///
    /// It is called by its platform, once per request, so a binary that
    /// declares one and a page runs the page, and one that declares only a
    /// worker has nothing to run at all — which is a refusal rather than a
    /// module handed to a JavaScript runtime that would start nothing.
    #[test]
    fn a_worker_is_not_a_program_to_start() {
        let flags = crate::commands::arguments::Flags::default();
        let worker = Output::for_platform(Platform::CloudflareWorker, Span::NONE);
        assert!(choose(std::slice::from_ref(&worker), &flags).is_none());

        let both = [worker, Output::for_platform(Platform::Web, Span::NONE)];
        assert_eq!(choose(&both, &flags).map(|o| o.platform()), Some(Platform::Web));
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
