//! `buri run`.
//!
//! Builds exactly one binary and executes it outside the sandbox, with the
//! real environment and the real filesystem. That is the point of the command:
//! it is the one that produces a program with authority.
//!
//! **A page is the one output with no process to start**, and it is the one
//! output a person most wants to look at. So `run` builds it and serves it on
//! a local port, answering the files under the artifact directory as themselves
//! and the shell for every other path — which is what lets the page's own
//! router see the address the reader typed. `commands/serve.rs` is that server;
//! this file is where the command decides it is what `run` means here.
#![allow(
    clippy::print_stderr,
    reason = "a `buri run` that has nothing to run is a complaint about the \
              invocation, and this command's own output; diagnostics still leave \
              through `Session::emit`"
)]

use crate::build::actions;
use crate::build::buildfile::{Output, Platform};
use crate::build::session;
use crate::build::session::Session;
use crate::build::workspace::{RuleKind, TargetId};
use crate::commands::arguments;
use crate::commands::serve;
use crate::commands::watch;

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
    // A page has no process to start, so `run` serves it instead.
    if output.platform() == Platform::Web {
        return serve_page(session, target, &output, args);
    }
    // And the two flags that belong to that server are refused on everything
    // else, before a line is compiled: each is about the invocation rather
    // than about the code, and a flag quietly ignored is a loop that never
    // happened with nothing said about it.
    let serving_flag =
        if args.flags.watch { Some("watch") } else { args.flags.port.map(|_| "port") };
    if let Some(flag) = serving_flag {
        eprintln!(
            "error: `buri run` takes `--{flag}` only for a page, and {} runs its {} output as a \
             process",
            session.workspace.label(target),
            output.platform().slug()
        );
        eprintln!(
            "  = a page is served over HTTP, so a rebuild is what the next request answers from; \
             a program with a process of its own is started once"
        );
        eprintln!(
            "  = fix: drop `--{flag}`, or name a page — `--output=web` picks one where a binary \
             declares a page beside a native output"
        );
        return 2;
    }

    // What follows this asks whether the artifact is a process or a module a
    // JavaScript runtime is handed, and everything that reaches here is one or
    // the other.
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
        Ok(st) => {
            // A signal spends the artifact's identity, and the next `buri run`
            // in this repository would be killed before it started.
            if crate::build::link::killed_by_signal(&st) {
                crate::build::link::spend_identity(&artifact.path);
            }
            st.code().unwrap_or(1)
        }
        Err(e) => {
            eprintln!("error: cannot execute the artifact: {e}");
            if !native {
                eprintln!("  = `buri run` needs a JavaScript runtime; install bun, or set BURI_JS");
            }
            2
        }
    }
}

/// Builds the page and answers requests for it until the process is stopped.
///
/// The order is the contract. **The port is taken before anything is
/// compiled**, so a port that is already in use is a refusal a reader gets at
/// once rather than after a build; **the address is printed once, before
/// anything blocks**, so a script and a person both know when to open it. Under
/// `--watch` it is printed from the loop's arming callback instead, which is
/// the instant the declared set has been stamped — an edit made after reading
/// that line is therefore an edit the next pass sees, rather than one racing
/// the stamp.
fn serve_page(
    mut session: Session,
    target: TargetId,
    output: &Output,
    args: &arguments::Args,
) -> i32 {
    let asked = args.flags.port.unwrap_or(serve::DEFAULT_PORT);
    let listener = match serve::bind(asked) {
        Ok(listener) => listener,
        Err(why) => {
            eprintln!("error: {why}");
            eprintln!(
                "  = fix: stop what is holding it, or name another with `--port=<port>`; \
                 `--port=0` takes whatever is free"
            );
            return 2;
        }
    };
    // What `--port=0` resolved to. Asking the listener rather than remembering
    // what was requested is the whole reason a test can use it.
    let port = listener.local_addr().map_or(asked, |address| address.port());

    let artifact = match actions::build_target(&mut session, target, output, &args.flags) {
        Ok(artifact) => artifact,
        Err(diagnostics) => {
            session.print(&diagnostics);
            return 1;
        }
    };
    let label = session.workspace.label(target);
    let page = match serve::Page::beside(&artifact.path) {
        Ok(page) => std::sync::Arc::new(page),
        Err(why) => {
            eprintln!("error: {label} has no page to serve: {why}");
            eprintln!("  = fix: build it again with `buri build {label}`");
            return 2;
        }
    };

    if !args.flags.watch {
        serve::announce(&label, port);
        serve::serve(&listener, &page);
        return 0;
    }

    // The set this build's keys were computed from, which is the loop's
    // opening set: the pass that produced it has already run, so pass 1 is the
    // one that hands it over and says nothing.
    let opening = watch::inputs(&session, &[target]);
    let root = session.root.clone();
    drop(session);
    let mut sources = crate::build::sources::Sources::at(&root, args.flags.clone());
    let mut listener = Some(listener);
    let serving = std::sync::Arc::clone(&page);
    watch::Watch::on(root, args.flags.explain).drive_armed(
        |trigger| {
            if trigger.pass == 1 {
                return watch::Pass {
                    code: 0,
                    inputs: opening.clone(),
                    output: String::new(),
                    quiet: true,
                };
            }
            rebuild(&mut sources, args, &page)
        },
        |pass| {
            if pass != 1 {
                return;
            }
            serve::announce(&label, port);
            if let Some(listener) = listener.take() {
                let page = std::sync::Arc::clone(&serving);
                std::thread::spawn(move || serve::serve(&listener, &page));
            }
        },
    )
}

/// One pass of the watch loop: build the page again, into the same directory
/// the server is answering out of.
///
/// The lock is held across the rebuild, so a request that lands in the middle
/// of one waits for it rather than reading a file half written — and what it
/// then gets is the new page rather than the old one, which is the answer a
/// reader who just saved wanted anyway. A rebuild that fails leaves the
/// previous artifact where it was and says why: a page you can still reload is
/// better than a blank one.
///
/// A pass served entirely from the cache is silent, which is `buri test
/// --watch`'s rule and the same one: an edit that changed no key changed
/// nothing a reader could reload for.
fn rebuild(
    sources: &mut crate::build::sources::Sources,
    args: &arguments::Args,
    page: &std::sync::Arc<serve::Page>,
) -> watch::Pass {
    // A pass that could not get as far as a build: it says so, and it hands
    // back no input set, which is what makes the loop keep watching the one it
    // already had — including the file whose repair is what it is waiting for.
    let stalled =
        |code| watch::Pass { code, inputs: Vec::new(), output: String::new(), quiet: false };
    sources.begin_round();
    let Ok(opened) = session::resume_or_exit(sources) else { return stalled(2) };
    let Ok((mut session, targets)) = session::resolve_in(opened, &args.targets) else {
        return stalled(2);
    };
    let binaries: Vec<_> = targets.iter().copied().filter(|t| t.kind == RuleKind::Binary).collect();
    // The same choice `command_run` made, made again: a `BUILD.buri` is a
    // declared input, so an edit to one can take the page away — and a loop
    // that kept building whatever it found would serve something nobody asked
    // for out of the directory a page was in.
    let still_a_page = match binaries.as_slice() {
        &[target] => choose(&actions::selected_outputs(&session, target, &args.flags), &args.flags)
            .filter(|output| output.platform() == Platform::Web)
            .map(|output| (target, output)),
        _ => None,
    };
    let Some((target, output)) = still_a_page else {
        eprintln!("error: this no longer names one page; the last build is still being served");
        return stalled(2);
    };
    let inputs = watch::inputs(&session, &[target]);
    let built = {
        let _writing = page.building();
        actions::build_target(&mut session, target, &output, &args.flags)
    };
    match built {
        Ok(artifact) => {
            watch::Pass { code: 0, inputs, output: String::new(), quiet: artifact.cached }
        }
        Err(diagnostics) => {
            session.print(&diagnostics);
            watch::Pass { code: 1, inputs, output: String::new(), quiet: false }
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
