//! The handle a command opens before it can do anything with a repository.
//!
//! Finding the root, reading `REPO.buri`, and loading every package happen
//! once, here, so that a command is written against a loaded graph rather than
//! against a directory. The source map and the diagnostics travel with it for
//! the same reason: every diagnostic the toolchain prints leaves through
//! `Session::emit`, which is why `--error-format=json` is one decision taken
//! in one place rather than a flag each command has to remember.

use crate::build::workspace::{find_root, Pattern, RuleKind, TargetId, Workspace};
use crate::commands::arguments::{ErrorFormat, Flags};
use crate::diagnostics::{Diagnostics, SourceMap};
use std::path::PathBuf;

pub struct Session {
    pub root: PathBuf,
    pub map: SourceMap,
    pub diags: Diagnostics,
    pub ws: Workspace,
    pub color: bool,
    pub error_format: ErrorFormat,
}

/// Finds the repository root, reads `REPO.buri`, and loads every package.
pub fn open(flags: &Flags) -> Result<Session, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let Some(root) = find_root(&cwd) else {
        return Err(
            "not in a Buri repository: no REPO.buri in this directory or any above it".into()
        );
    };
    let mut map = SourceMap::new();
    let mut diags = Diagnostics::new();
    let ws = Workspace::load(&root, &mut map, &mut diags).map_err(|e| e.to_string())?;
    // Before anything is built: a repository pinned to a compiler this is not
    // gets a refusal rather than an artifact. Every command that needs a
    // repository comes through here, so the pin is checked once and cannot be
    // the one thing a new command forgets.
    crate::build::toolchain::verify(&ws.repo.toolchain)?;
    // JSON carries its own rendering, and escapes would corrupt it.
    let color = flags.error_format == ErrorFormat::Human
        && flags.color.unwrap_or_else(|| std::env::var("NO_COLOR").is_err());
    Ok(Session { root, map, diags, ws, color, error_format: flags.error_format })
}

impl Session {
    /// Resolves the target arguments. With none, commands operate on the
    /// package containing the working directory.
    pub fn resolve_targets(&self, args: &[String]) -> Result<Vec<TargetId>, String> {
        let patterns: Vec<Pattern> = if args.is_empty() {
            let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
            let rel = self.ws.rel_of(&cwd);
            let rel = if rel == "." { String::new() } else { rel };
            match self.ws.owning_package(&cwd.join("x")) {
                Some(id) => vec![Pattern::Package(self.ws.pkg(id).path.clone())],
                None => {
                    return Err(format!(
                        "no package at `{rel}`; name one, as in `buri build //lib/money`"
                    ))
                }
            }
        } else {
            args.iter().map(|a| Pattern::parse(a)).collect::<Result<_, _>>()?
        };

        let mut out = Vec::new();
        for p in &patterns {
            let mut matched = false;
            for t in self.ws.targets() {
                if p.matches(&self.ws.pkg(t.pkg).path) {
                    out.push(t);
                    matched = true;
                }
            }
            if !matched {
                return Err(match p {
                    Pattern::Package(path) => format!("no target in `//{path}`"),
                    Pattern::Recursive(path) => format!("no target under `//{path}/...`"),
                    Pattern::All => "this repository declares no targets".into(),
                });
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    pub fn label(&self, t: TargetId) -> String {
        let base = self.ws.pkg(t.pkg).label();
        match t.kind {
            RuleKind::Library => base,
            RuleKind::Binary => base,
        }
    }

    pub fn report(&mut self) -> bool {
        self.diags.sort(&self.map);
        let mut had_error = false;
        for d in &self.diags.items {
            self.emit(d);
            had_error |= d.is_error();
        }
        self.diags.items.clear();
        had_error
    }

    pub fn print(&self, diags: &Diagnostics) -> bool {
        let mut had_error = false;
        for d in &diags.items {
            self.emit(d);
            had_error |= d.is_error();
        }
        had_error
    }

    /// Every diagnostic leaves through here, so the format is chosen once.
    pub fn emit(&self, d: &crate::diagnostics::Diagnostic) {
        match self.error_format {
            ErrorFormat::Human => eprint!("{}", self.map.render(d, self.color)),
            ErrorFormat::Json => eprintln!("{}", self.map.to_json(d)),
        }
    }
}

/// `open`, with the message a command would otherwise repeat.
///
/// Not being in a repository is a problem with the invocation rather than with
/// the code, so it exits 2.
pub fn open_or_exit(flags: &Flags) -> Result<Session, u8> {
    match open(flags) {
        Ok(s) => Ok(s),
        Err(msg) => {
            eprintln!("error: {msg}");
            Err(2)
        }
    }
}
