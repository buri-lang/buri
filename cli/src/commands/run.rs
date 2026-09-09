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
    command.args(&args.passthrough);
    match stopping::start(&mut command) {
        Ok(mut child) => {
            let Ok(st) = stopping::wait(&mut child) else {
                eprintln!("error: cannot wait for the artifact");
                return 2;
            };
            // A signal spends the artifact's identity, and the next `buri run`
            // in this repository would be killed before it started.
            if crate::build::link::killed_by_signal(&st) {
                crate::build::link::spend_identity(&artifact.path);
            }
            stopping::status(&st)
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

/// **Stopping `buri run` stops the program it started.**
///
/// The artifact is a child process, so a `SIGTERM` that reaches only this
/// command leaves the program running, reparented to `init`, still holding
/// whatever it holds — a port, a lock file, an open write-ahead log — and never
/// told to stop (buri-lang/buri#91). So the signals a program is stopped with
/// are caught here and sent on to the child, and this command exits on the wait
/// for that child: the program's own drain is the bound, and there is no clock
/// here that could cut one short.
///
/// **The program gets the signal once.** A terminal sends `SIGINT` and `SIGHUP`
/// to every process in its foreground group, which is where a child that
/// inherited this process's group already is — and a second one is the
/// operating system's, because `cli/runtime/net.rs` restores the default
/// disposition before it drains (design/native/DECISIONS.md). Forwarding
/// blindly would kill, on the second delivery, the server the first was meant
/// to drain. So the group the child runs in decides what is forwarded:
///
///   * with a terminal in front of the command the program keeps this process's
///     group, which is what lets it read that terminal, and only `SIGTERM` —
///     which no terminal generates — is sent on;
///   * with no terminal the program gets a group of its own, so nothing aimed
///     at this one reaches it, and all three are sent on.
mod stopping {
    use std::process::{Child, Command, ExitStatus};
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

    /// `SIGHUP` — the terminal went away. `SIGINT` — a person at one.
    /// `SIGTERM` — a supervisor, a container runtime, an `init`. One, two and
    /// fifteen on both platforms this toolchain admits, and `cli/runtime/net.rs`
    /// writes the second and third on the other side of the C ABI.
    const SIGHUP: i32 = 1;
    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;

    /// The three, in the order they are installed.
    const CAUGHT: [i32; 3] = [SIGHUP, SIGINT, SIGTERM];

    /// `SIG_ERR`, which `signal` answers when it will not do what it was asked.
    const SIG_ERR: usize = usize::MAX;

    // The calls this module makes into the C library, declared rather than
    // depended on — `cli/runtime/net.rs`'s `shutdown` module is the precedent,
    // and the argument is the same one: a dependency for a declaration is a
    // dependency.
    //
    // `signal` rather than `sigaction` because the two platforms lay `struct
    // sigaction` out differently and nothing here needs a field of it, and both
    // give `signal` BSD semantics — the handler stays installed, and the wait
    // on the child is restarted rather than answered `EINTR`, which is what
    // makes the drain the bound and not the signal.
    unsafe extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
        fn kill(pid: i32, sig: i32) -> i32;
        fn setpgid(pid: i32, pgid: i32) -> i32;
        fn getpgrp() -> i32;
        fn tcgetpgrp(fd: i32) -> i32;
    }

    #[cfg(target_os = "macos")]
    unsafe extern "C" {
        /// The address of this thread's `errno`.
        fn __error() -> *mut i32;
    }

    #[cfg(not(target_os = "macos"))]
    unsafe extern "C" {
        /// The same, spelled the way glibc and musl spell it.
        fn __errno_location() -> *mut i32;
    }

    fn errno_slot() -> *mut i32 {
        #[cfg(target_os = "macos")]
        // SAFETY: a thread-local address, and nothing else.
        unsafe {
            __error()
        }
        #[cfg(not(target_os = "macos"))]
        // SAFETY: as above.
        unsafe {
            __errno_location()
        }
    }

    /// The program's process id, or `0` while there is none to signal.
    static CHILD: AtomicI32 = AtomicI32::new(0);

    /// Whether the program is in this process's group, which it is exactly when
    /// there is a terminal to share.
    static SHARES_THE_GROUP: AtomicBool = AtomicBool::new(false);

    /// The handler. **Everything it does is on POSIX's async-signal-safe list**,
    /// which for one `kill` is the whole of the argument the module header makes
    /// at greater length for the runtime's.
    extern "C" fn forward(sig: i32) {
        let slot = errno_slot();
        // SAFETY: `errno_slot` answers this thread's own `errno`.
        let saved = unsafe { slot.read() };
        let child = CHILD.load(Ordering::Relaxed);
        // A terminal's signal has already reached a child in this group, and
        // sending it again is what would kill the drain it started.
        let arrived_already = SHARES_THE_GROUP.load(Ordering::Relaxed) && sig != SIGTERM;
        if child > 0 && !arrived_already {
            // SAFETY: an ordinary `kill` on this process's own child.
            unsafe { kill(child, sig) };
        }
        // SAFETY: as above. Restored last, so nothing between the two reads a
        // value this handler produced.
        unsafe { slot.write(saved) };
    }

    /// Whether a terminal is in front of this command.
    ///
    /// `tcgetpgrp` answers the foreground process group of the terminal behind
    /// a descriptor, so a match on any of the three standard ones means a
    /// keystroke reaches this process group — and everything in it.
    fn a_terminal_in_front() -> bool {
        // SAFETY: this reads the calling process's own group and nothing else.
        let group = unsafe { getpgrp() };
        // SAFETY: `tcgetpgrp` reads a descriptor this process holds, and
        // answers -1 for one that is not a terminal.
        (0..3).any(|fd| unsafe { tcgetpgrp(fd) } == group)
    }

    /// Starts the program, and takes the signals that stop it.
    pub fn start(command: &mut Command) -> std::io::Result<Child> {
        use std::os::unix::process::CommandExt as _;

        let shares = a_terminal_in_front();
        if !shares {
            // SAFETY: `setpgid` is on POSIX's async-signal-safe list, which is
            // the whole of what a `pre_exec` closure may call.
            unsafe {
                command.pre_exec(|| {
                    setpgid(0, 0);
                    Ok(())
                })
            };
        }
        let child = command.spawn()?;
        SHARES_THE_GROUP.store(shares, Ordering::Relaxed);
        CHILD.store(i32::try_from(child.id()).unwrap_or(0), Ordering::Relaxed);
        // Taken after the child is known rather than before it. A signal in the
        // moment between the two would otherwise be one this command caught
        // with nothing to forward and then waited out for ever; ending the way
        // it always did is the better of the two.
        for sig in CAUGHT {
            // SAFETY: an ordinary `signal` call with a function this module
            // owns. None of the three is a signal that may not be caught.
            let previous = unsafe { signal(sig, forward as *const () as usize) };
            debug_assert!(previous != SIG_ERR, "SIGHUP, SIGINT and SIGTERM can be caught");
        }
        Ok(child)
    }

    /// Waits for the program to stop, however long its own shutdown takes.
    pub fn wait(child: &mut Child) -> std::io::Result<ExitStatus> {
        let stopped = child.wait();
        // A reaped pid is the operating system's to hand out again, and a
        // signal arriving after that must not reach a stranger.
        CHILD.store(0, Ordering::Relaxed);
        stopped
    }

    /// What this command exits with: the program's code where it returned one,
    /// and 128 plus the signal where one ended it.
    ///
    /// `ExitStatus::code` is `None` for exactly those, so passing it straight
    /// through would report the same status for a program that was killed and
    /// one that chose to fail.
    pub fn status(stopped: &ExitStatus) -> i32 {
        use std::os::unix::process::ExitStatusExt as _;
        stopped
            .code()
            .unwrap_or_else(|| 128_i32.saturating_add(stopped.signal().unwrap_or(0)))
    }
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
