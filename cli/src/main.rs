//! The `buri` toolchain.
//!
//! One binary. It builds, runs, tests, formats, lints, generates build files,
//! answers questions about the graph, and serves its own documentation. There
//! is no second tool to install, no package manager, no task runner, and no
//! configuration of the CLI itself beyond `REPO.buri`.
//!
//! This file is argument handling. Which commands exist, which flags each one
//! takes, and what every one of them means live in `commands::COMMANDS`, so
//! the help text and `buri docs cli` are generated from the same table that
//! dispatches.
#![allow(
    clippy::print_stderr,
    reason = "a malformed invocation is reported by the CLI itself, before there \
              is a session; every diagnostic about a repository still leaves \
              through `Session::emit`"
)]

use buri::commands::{self, arguments};
use std::process::ExitCode;

/// The stack the toolchain runs on.
///
/// Every stage after parsing walks the syntax tree by recursion, so the depth
/// of the tree is stack. The parser bounds that depth — see `MAX_DEPTH` and
/// `MAX_CHAIN` in `parsing::parser` — and this is the other half of the same
/// arrangement: the bound is chosen to sit an order of magnitude under what
/// this reserves, so that a tree the parser accepts is a tree every later
/// stage can walk. A generated protobuf decoder is the case that needs the
/// room; it is one `else if` per field, and a schema with a thousand fields is
/// a thousand-deep tree that is nobody's mistake.
///
/// The reservation is address space rather than memory: pages are committed as
/// they are touched, so a `buri version` that never recurses pays nothing for
/// it. The default main-thread stack is 8 MiB, which a decoder for a schema of
/// six hundred fields overflowed.
const STACK: usize = buri::parallel::STACK;

/// The coverage gate's build only: `buri run`'s server and the `--watch` loops
/// stop by a signal, so they write their profile before dying of it. `buri run`
/// puts its own handlers over these once it starts a program, and that path
/// exits normally.
#[cfg(buri_coverage)]
fn write_profile_on_signals() {
    unsafe extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
        fn raise(sig: i32) -> i32;
    }
    extern "C" fn written(sig: i32) {
        arguments::write_coverage_profile();
        // SAFETY: the default disposition back, then the signal again, so the
        // process ends the way it would have.
        unsafe {
            signal(sig, 0);
            raise(sig);
        }
    }
    // SIGHUP, SIGINT and SIGTERM.
    for sig in [1, 2, 15] {
        // SAFETY: an ordinary `signal` call with a function of ours.
        unsafe { signal(sig, written as *const () as usize) };
    }
}

fn main() -> ExitCode {
    #[cfg(buri_coverage)]
    write_profile_on_signals();
    // A `println!` whose reader has gone ends the process, from whichever
    // thread it ran on, before anything unwinds.
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if info.payload().downcast_ref::<String>().is_some_and(|m| arguments::is_reader_gone(m)) {
            arguments::reader_gone();
        }
        report(info);
    }));
    // The work happens on a thread of our own, because the main thread's stack
    // is fixed by the process that started us and cannot be asked for more.
    match std::thread::Builder::new().name("buri".into()).stack_size(STACK).spawn(run) {
        Ok(worker) => match worker.join() {
            Ok(code) => code,
            // A panic has already printed its own message; this is the exit
            // status for it, and 101 is what a panicking Rust process exits.
            Err(_) => ExitCode::from(101),
        },
        // No thread to be had is a machine problem rather than an input
        // problem, and there is nowhere to run the command.
        Err(e) => {
            eprintln!("error: cannot start the toolchain: {e}");
            ExitCode::from(70)
        }
    }
}

/// `BURI_PROFILE`'s allocation column (`buri::profile`). Off by default: it is
/// a thread-local increment on every allocation.
#[cfg(feature = "alloc-counter")]
#[global_allocator]
static COUNTING: buri::profile::Counting = buri::profile::Counting;

/// Per-thread size classes for small blocks (`buri::allocator`).
#[cfg(not(feature = "alloc-counter"))]
#[global_allocator]
static ALLOCATOR: buri::allocator::Allocator = buri::allocator::Allocator;

fn run() -> ExitCode {
    let started = std::time::Instant::now();
    let code = {
        let _phase = buri::profile::enter(buri::profile::Phase::Other);
        command()
    };
    if let Some(report) = buri::profile::report(started) {
        eprint!("{report}");
    }
    buri::build::link::settle();
    code
}

fn command() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match arguments::parse(&argv) {
        Ok(a) => a,
        Err(msg) => {
            if msg.is_empty() {
                arguments::out(&commands::usage());
                return ExitCode::from(if argv.is_empty() { 2 } else { 0 });
            }
            eprintln!("error: {msg}");
            eprintln!();
            eprint!("{}", commands::usage());
            return ExitCode::from(2);
        }
    };

    if matches!(args.command.as_str(), "help" | "--help" | "-h") {
        arguments::out(&commands::usage());
        return ExitCode::from(0);
    }

    let Some(command) = commands::find(&args.command) else {
        eprintln!("error: there is no command `{}`", args.command);
        let names: Vec<&str> = commands::COMMANDS.iter().map(|c| c.name).collect();
        if let Some(near) = buri::build::buildfile::nearest(&args.command, &names) {
            eprintln!("  = did you mean `buri {near}`?");
        }
        eprintln!();
        eprint!("{}", commands::usage());
        return ExitCode::from(2);
    };

    ExitCode::from((command.run)(&args) as u8)
}
