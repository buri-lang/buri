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

use buri::commands::{self, arguments};
use std::process::ExitCode;

fn main() -> ExitCode {
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
