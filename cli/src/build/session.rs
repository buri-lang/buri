//! The handle a command opens before it can do anything with a repository.
//!
//! Finding the root, reading `REPO.buri`, and loading every package happen
//! once, here, so that a command is written against a loaded graph rather than
//! against a directory. The source map and the diagnostics travel with it for
//! the same reason: every diagnostic the toolchain prints leaves through
//! `Session::emit`, which is why `--error-format=json` is one decision taken
//! in one place rather than a flag each command has to remember.

use crate::build::workspace::{find_root, Pattern, TargetId, Workspace};
use crate::commands::arguments::{ErrorFormat, Flags};
use crate::diagnostics::{Diagnostics, SourceMap};
use std::path::PathBuf;

/// How diagnostics are rendered.
///
/// Colour belongs to the human renderer and nowhere else — JSON carries its own
/// rendering, and an escape sequence in it would corrupt the stream. Putting the
/// flag inside the `Human` variant is what makes "colour, in JSON" a thing that
/// cannot be written down, rather than a correlation one line of `open` has to
/// keep enforcing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rendering {
    Human { color: bool },
    Json,
}

pub struct Session {
    pub root: PathBuf,
    pub map: SourceMap,
    /// Parses, kept for the life of the command. A command analyses one target
    /// at a time and every target imports the standard library, so without
    /// this the same files are lexed and parsed once per target.
    pub parsed: crate::parsing::parser::Cache,
    pub diagnostics: Diagnostics,
    pub workspace: Workspace,
    pub rendering: Rendering,
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
    let mut diagnostics = Diagnostics::new();
    let workspace = Workspace::load(&root, &mut map, &mut diagnostics).map_err(|e| e.to_string())?;
    let rendering = match flags.error_format {
        ErrorFormat::Json => Rendering::Json,
        ErrorFormat::Human => Rendering::Human {
            color: flags.color.unwrap_or_else(|| std::env::var("NO_COLOR").is_err()),
        },
    };
    // `--dense` means the same thing here as it does for `buri docs`: the
    // headings and the code, none of the prose.
    crate::diagnostics::print_bodies(!flags.dense);
    Ok(Session {
        root,
        map,
        parsed: crate::parsing::parser::Cache::new(),
        diagnostics,
        workspace,
        rendering,
    })
}

impl Session {
    /// Resolves the target arguments. With none, commands operate on the whole
    /// repository, so where you are standing is not part of what a command
    /// means.
    pub fn resolve_targets(&self, args: &[String]) -> Result<Vec<TargetId>, String> {
        let patterns: Vec<Pattern> = if args.is_empty() {
            vec![Pattern::All]
        } else {
            args.iter().map(|a| Pattern::parse(a)).collect::<Result<_, _>>()?
        };

        let mut out = Vec::new();
        for p in &patterns {
            let mut matched = false;
            for t in self.workspace.targets() {
                if p.matches(&self.workspace.package(t.package).path) {
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

    /// Emits everything the session has collected, in source order, and
    /// empties it. `true` when any of it was an error.
    pub fn report(&mut self) -> bool {
        self.diagnostics.sort(&self.map);
        let had_error = self.print(&self.diagnostics);
        self.diagnostics.clear();
        had_error
    }

    /// The same for a set of diagnostics the session does not own, which is
    /// what an analysis hands back. Left in the order it arrived: only the
    /// session's own collection is a mixture of several passes' findings.
    pub fn print(&self, diagnostics: &Diagnostics) -> bool {
        let mut had_error = false;
        for d in &diagnostics.items {
            self.emit(d);
            had_error |= d.is_error();
        }
        had_error
    }

    /// Every diagnostic leaves through here, so the format is chosen once.
    #[expect(
        clippy::print_stderr,
        reason = "this is the sink the `print_stderr` lint exists to funnel every diagnostic into; \
                  writing them is what it is for"
    )]
    pub fn emit(&self, d: &crate::diagnostics::Diagnostic) {
        match self.rendering {
            Rendering::Human { color } => eprint!("{}", self.map.render_with_body(d, color)),
            Rendering::Json => eprintln!("{}", self.map.to_json(d)),
        }
    }
}

/// `open`, with the message a command would otherwise repeat.
///
/// Not being in a repository is a problem with the invocation rather than with
/// the code, so it exits 2.
#[expect(
    clippy::print_stderr,
    reason = "a failure to open the repository is a failure to build the Session, so there is no \
              `emit` to route it through yet"
)]
pub fn open_or_exit(flags: &Flags) -> Result<Session, u8> {
    match open(flags) {
        Ok(session) => Ok(session),
        Err(msg) => {
            eprintln!("error: {msg}");
            Err(2)
        }
    }
}

/// `open_or_exit`, then the two things every command taking targets does next:
/// report what loading the repository found, and resolve the arguments to
/// targets.
///
/// All three failures exit 2, and for one reason: an unparseable build file, a
/// missing repository and a label naming nothing are each a problem with the
/// invocation rather than with the code.
#[expect(
    clippy::print_stderr,
    reason = "the same argument as `open_or_exit` above: a target argument that resolves to \
              nothing is not a diagnostic about a source file, so there is no `emit` for it"
)]
pub fn open_and_resolve(flags: &Flags, args: &[String]) -> Result<(Session, Vec<TargetId>), u8> {
    let mut session = open_or_exit(flags)?;
    if session.report() {
        return Err(2);
    }
    match session.resolve_targets(args) {
        Ok(targets) => Ok((session, targets)),
        Err(msg) => {
            eprintln!("error: {msg}");
            Err(2)
        }
    }
}
