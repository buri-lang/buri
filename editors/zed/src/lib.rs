//! The Buri extension for Zed.
//!
//! It does two things: it names the grammar, and it starts `buri lsp`. The
//! server is the toolchain binary itself, so there is nothing to install
//! separately and no second copy of the language's understanding of a program
//! to keep in step.

use zed_extension_api::{self as zed, Command, LanguageServerId, Result, Worktree};

struct BuriExtension;

impl zed::Extension for BuriExtension {
    fn new() -> Self {
        BuriExtension
    }

    fn language_server_command(
        &mut self,
        _id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Command> {
        // `buri` from the user's PATH rather than a downloaded release: an
        // extension that fetched its own would be answering about a different
        // compiler than the one `buri build` uses.
        let path = worktree
            .which("buri")
            .ok_or_else(|| "`buri` is not on PATH; install the toolchain".to_string())?;

        Ok(Command {
            command: path,
            args: vec!["lsp".into()],
            env: worktree.shell_env(),
        })
    }
}

zed::register_extension!(BuriExtension);
