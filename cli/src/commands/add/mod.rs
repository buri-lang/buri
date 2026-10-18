//! `buri add` — what the toolchain writes into a repository that is already
//! there.
//!
//! A namespace rather than a command: `buri add skills` is the only thing it
//! writes today, and the things a toolchain can add to a checkout it did not
//! create are a family — `init` writes a repository once, and everything after
//! that is an addition to one. Spelling the family as a namespace means the
//! second one is an entry in `SUBCOMMANDS` rather than another hyphenated
//! top-level name nobody would think to look for.
pub mod skills;

use crate::commands::arguments::Args;
use crate::commands::{Command, Subcommand};

/// `buri add`'s line in `commands::COMMANDS`, which it dispatches through.
pub const COMMAND: Command = Command {
    name: "add",
    args: "<subcommand>",
    blurb: "write what this toolchain ships into an existing repository",
    doc: crate::documentation::embedded::cli::ADD,
    flags: &[],
    run: command_add,
    subcommands: SUBCOMMANDS,
    hidden: false,
};

/// The one table `buri add` dispatches through, and the one the help text and
/// the reference page are generated from — the same arrangement as
/// `commands::COMMANDS`, one level down.
pub const SUBCOMMANDS: &[Subcommand] = &[Subcommand {
    name: "skills",
    args: "[directory]",
    blurb: "write the agent skills for this toolchain into .agent/skills",
    run: skills::command_add_skills,
}];

pub fn command_add(args: &Args) -> i32 {
    crate::commands::dispatch(&COMMAND, args)
}
