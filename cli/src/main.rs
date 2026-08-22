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

fn main() -> ExitCode {
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

fn run() -> ExitCode {
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
