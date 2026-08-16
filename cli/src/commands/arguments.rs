//! Argument parsing.
//!
//! Exit codes (CLI.md):
//!   0  success; for `test`, every test passed
//!   1  build, lint, or test failure — the thing you asked about is wrong
//!   2  malformed invocation, unparseable build file — the thing you asked
//!      *with* is wrong

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
