//! Argument parsing.
//!
//! Exit codes (CLI.md):
//!   0  success; for `test`, every test passed
//!   1  build, lint, or test failure — the thing you asked about is wrong
//!   2  malformed invocation, unparseable build file — the thing you asked
//!      *with* is wrong

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Args {
    pub command: String,
    pub targets: Vec<String>,
    pub flags: Flags,
    /// Arguments after `--`, which go to the program `buri run` executes.
    pub passthrough: Vec<String>,
}

/// Which of the two builds is being asked for.
///
/// `--release` and `--debug` are one choice, so they are one field. Before,
/// they were two booleans whose fourth state — both set — `parse` rejected and
/// the three other constructors of `Flags` did not; and `debug` was read by
/// nothing, so `--debug` parsed and meant nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BuildMode {
    #[default]
    Debug,
    Release,
}

impl BuildMode {
    pub fn is_release(self) -> bool {
        self == BuildMode::Release
    }

    /// The spelling that enters a cache key.
    pub fn name(self) -> &'static str {
        match self {
            BuildMode::Debug => "debug",
            BuildMode::Release => "release",
        }
    }
}

#[derive(Default)]
pub struct Flags {
    pub mode: BuildMode,
    pub check: bool,
    /// Apply the findings that have one mechanical answer. `buri lint` only.
    pub fix: bool,
    pub force: bool,
    /// Build twice — a fresh session each time, the cache bypassed, into two
    /// separate output directories — and compare the artifacts byte for byte.
    /// `buri build` only.
    pub check_reproducible: bool,
    pub accept: bool,
    pub outputs_only: bool,
    pub output: Option<String>,
    pub filter: Option<String>,
    pub color: Option<bool>,
    pub error_format: ErrorFormat,
    /// How `buri docs` prints a page. Distinct from `--error-format`, which is
    /// about diagnostics and applies to every command.
    pub format: Format,
    /// Print headings and examples only, dropping most prose. For agents.
    pub dense: bool,
    pub self_check: bool,
    pub verbose: bool,
    /// Report each action, its key, and whether the cache served it. The build
    /// system's claims are about *which actions run*, and a claim nothing can
    /// observe from outside is not one anybody can hold the toolchain to.
    pub explain: bool,
    /// Re-run the invocation every time one of its declared inputs moves.
    /// `buri test` only, and refused in the three combinations `parse` names
    /// below.
    pub watch: bool,
}

/// How `buri docs` prints a page.
///
/// `Human` is a variant rather than the `None` of an `Option<Format>`: with
/// the option, `--format=human` and no `--format` at all were the same value,
/// so "the default happens to be human" and "the user asked for human" could
/// not be told apart — and this sat next to `ErrorFormat`, which already
/// spelled the same choice as a `#[default]` variant.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Format {
    /// Rendered for a terminal.
    #[default]
    Human,
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
    // Which of `--release` / `--debug` was written, so that naming both is
    // still refused. It lives here rather than on `Flags` because "both" is a
    // property of the argument list, not a state a build can be in.
    let mut mode_named: Option<&str> = None;
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
                    "`buri {command}` does not take `--{name}`; {}",
                    match takers.as_slice() {
                        [] => "no command does".to_string(),
                        [only] => format!("`buri {only}` does"),
                        _ => format!("`buri {}` do", takers.join("`, `buri ")),
                    }
                ));
            }
            if matches!(flag.value, crate::commands::Value::Required(_)) && value.is_none() {
                return Err(format!("`--{name}` needs a value, as in `--{name}=...`"));
            }
            if matches!(flag.value, crate::commands::Value::None) && value.is_some() {
                return Err(format!("`--{name}` takes no value"));
            }
            if name == "release" || name == "debug" {
                if mode_named.is_some_and(|prev| prev != name) {
                    return Err("`--release` and `--debug` are exclusive".into());
                }
                mode_named = Some(if name == "release" { "release" } else { "debug" });
            }
            (flag.set)(&mut flags, value.as_deref())?;
            continue;
        }
        targets.push(arg.clone());
    }

    if flags.watch {
        refuse_watch(&flags)?;
    }
    Ok(Args { command, targets, flags, passthrough })
}

/// The three things `--watch` will not be combined with.
///
/// All three at parsing rather than in `cmd_test`, because none of them is a
/// question about a repository: each is a way of asking for a loop that would
/// not be the mode it is named after (BUILD-AND-WATCH.md §4.3).
///
/// The order is deliberate. A flag combination is a mistake in the command
/// line, and it is the one a person can fix by reading; the terminal check is
/// about where the command is running and comes last, so that
/// `buri test --watch --force` in a pipe says which flag is wrong rather than
/// which pipe it is in.
fn refuse_watch(flags: &Flags) -> Result<(), String> {
    use std::io::IsTerminal as _;
    if flags.force {
        return Err(
            "`--watch` and `--force` are exclusive: `--force` turns every cache hit into a run, \
             so every save would re-run every suite in the selection — and the cache is the whole \
             of what makes a watch loop cheap"
                .into(),
        );
    }
    if flags.accept {
        return Err(
            "`--watch` and `--accept` are exclusive: `--accept` is the one mode that writes to \
             the source tree, and rewriting golden files on a timer accepts a regression while \
             you are still reading the failure"
                .into(),
        );
    }
    if !std::io::stdout().is_terminal() {
        return Err(
            "`--watch` needs a terminal: a watch loop with nothing watching it is a hung job, \
             which in CI is a build that never finishes — run `buri test` instead, which is the \
             same selection run once"
                .into(),
        );
    }
    Ok(())
}

/// An unknown flag, with the nearest real one when there is a plausible
/// candidate — the same treatment an unknown identifier gets.
fn unknown_flag(name: &str) -> String {
    let known: Vec<&str> = crate::commands::FLAGS.iter().map(|f| f.name).collect();
    match crate::build::buildfile::nearest(name, &known) {
        Some(near) => format!("unknown flag `--{name}`; did you mean `--{near}`?"),
        None => format!("unknown flag `--{name}`"),
    }
}

/// Writes to standard output, treating a closed pipe as success.
///
/// `buri docs lang/types | head` is the first thing anybody does, and the
/// `print!` macro panics when the reader goes away. Nothing has gone wrong in
/// that case — the caller got what it asked for — so exit quietly.
#[expect(
    clippy::print_stderr,
    reason = "a failed write to stdout is the one thing that cannot be reported on stdout"
)]
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
