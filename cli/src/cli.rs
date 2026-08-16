//! Argument parsing and the command dispatch.
//!
//! Exit codes (CLI.md):
//!   0  success; for `test`, every test passed
//!   1  build, lint, or test failure — the thing you asked about is wrong
//!   2  malformed invocation, unparseable build file — the thing you asked
//!      *with* is wrong

use crate::diag::{Diagnostics, SourceMap};
use crate::workspace::{find_root, Pattern, RuleKind, TargetId, Workspace};
use std::path::PathBuf;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The short help. Generated from `commands::COMMANDS`, so it cannot list a
/// command that does not exist or omit one that does.
pub fn usage() -> String {
    crate::commands::usage()
}


pub struct Args {
    pub command: String,
    pub targets: Vec<String>,
    pub flags: Flags,
    /// Arguments after `--`, which go to the program `buri run` executes.
    pub passthrough: Vec<String>,
}

#[derive(Default)]
pub struct Flags {
    pub release: bool,
    pub debug: bool,
    pub check: bool,
    /// Apply the findings that have one mechanical answer. `buri lint` only.
    pub fix: bool,
    pub force: bool,
    pub accept: bool,
    pub outputs_only: bool,
    pub output: Option<String>,
    pub filter: Option<String>,
    pub color: Option<bool>,
    pub error_format: ErrorFormat,
    /// How `buri docs` prints a page. Distinct from `--error-format`, which is
    /// about diagnostics and applies to every command.
    pub format: Option<Format>,
    /// Print headings and examples only, dropping most prose. For agents.
    pub dense: bool,
    pub self_check: bool,
    pub verbose: bool,
    /// Report each action, its key, and whether the cache served it. The build
    /// system's claims are about *which actions run*, and a claim nothing can
    /// observe from outside is not one anybody can hold the toolchain to.
    pub explain: bool,
}

/// How `buri docs` prints a page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    /// The markdown source, unrendered.
    Markdown,
    /// One JSON object, for a tool or a coding agent.
    Json,
}

/// How diagnostics reach the terminal.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum ErrorFormat {
    /// Source snippet, carets, and the `= ...` block.
    #[default]
    Human,
    /// One JSON object per diagnostic, one per line: stable field names, no
    /// escapes, nothing to parse out of a pretty-printed snippet.
    Json,
}

pub fn parse(argv: &[String]) -> Result<Args, String> {
    let mut it = argv.iter().peekable();
    let Some(command) = it.next().cloned() else {
        return Err(String::new());
    };
    let mut targets = Vec::new();
    let mut passthrough = Vec::new();
    let mut flags = Flags::default();
    let mut after_dashdash = false;
    // `help` is not in the table — it prints the table — so flag checking is
    // skipped for it rather than special-cased inside the loop.
    let known_command = crate::commands::find(&command).is_some();

    for arg in it {
        if after_dashdash {
            passthrough.push(arg.clone());
            continue;
        }
        if arg == "--" {
            after_dashdash = true;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--") {
            let (name, value) = match rest.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (rest, None),
            };
            if name == "help" {
                return Err(String::new());
            }
            let Some(flag) = crate::commands::flag(name) else {
                return Err(unknown_flag(name));
            };
            // A flag that exists but belongs to another command is a different
            // mistake from one that does not exist, and saying which commands
            // do take it is the fix.
            if known_command && !crate::commands::accepts(&command, name) {
                let takers = crate::commands::commands_taking(name);
                return Err(format!(
                    "`buri {command}` does not take `--{name}`; {} does",
                    match takers.len() {
                        0 => "no command".to_string(),
                        1 => format!("`buri {}`", takers[0]),
                        _ => format!(
                            "`buri {}`",
                            takers.join("`, `buri ")
                        ),
                    }
                ));
            }
            if matches!(flag.value, crate::commands::Value::Required(_)) && value.is_none() {
                return Err(format!("`--{name}` needs a value, as in `--{name}=...`"));
            }
            if matches!(flag.value, crate::commands::Value::None) && value.is_some() {
                return Err(format!("`--{name}` takes no value"));
            }
            (flag.set)(&mut flags, value.as_deref())?;
            continue;
        }
        targets.push(arg.clone());
    }

    if flags.release && flags.debug {
        return Err("`--release` and `--debug` are exclusive".into());
    }
    Ok(Args { command, targets, flags, passthrough })
}

/// An unknown flag, with the nearest real one when there is a plausible
/// candidate — the same treatment an unknown identifier gets.
fn unknown_flag(name: &str) -> String {
    let known: Vec<&str> = crate::commands::FLAGS.iter().map(|f| f.name).collect();
    match crate::buildfile::nearest(name, &known) {
        Some(near) => format!("unknown flag `--{name}`; did you mean `--{near}`?"),
        None => format!("unknown flag `--{name}`"),
    }
}

/// Writes to standard output, treating a closed pipe as success.
///
/// `buri docs lang/types | head` is the first thing anybody does, and the
/// `print!` macro panics when the reader goes away. Nothing has gone wrong in
/// that case — the caller got what it asked for — so exit quietly.
pub fn out(text: &str) {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    match lock.write_all(text.as_bytes()).and_then(|()| lock.flush()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => std::process::exit(0),
        Err(e) => {
            eprintln!("error: cannot write to stdout: {e}");
            std::process::exit(2);
        }
    }
}

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
    pub fn emit(&self, d: &crate::diag::Diagnostic) {
        match self.error_format {
            ErrorFormat::Human => eprint!("{}", self.map.render(d, self.color)),
            ErrorFormat::Json => eprintln!("{}", self.map.to_json(d)),
        }
    }
}
