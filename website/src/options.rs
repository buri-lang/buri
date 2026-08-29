//! What the command was asked to do.
//!
//! Four flags and one subcommand. The argument parser is here rather than
//! borrowed from `commands::arguments` because that one is the `buri` CLI's
//! surface, and this is not the `buri` CLI.

use std::path::PathBuf;

/// Where the site lands when nothing says otherwise. Under `target/`, which
/// the repository already ignores, because a generated site that can be
/// committed eventually is.
pub const DEFAULT_OUTPUT: &str = "target/docs-site";

pub const DEFAULT_PORT: u16 = 8080;

#[derive(PartialEq, Eq, Debug)]
pub enum Command {
    /// Write the site.
    Build,
    /// Build into a temporary directory and check every link it wrote.
    Check,
    /// Build, then serve the result on localhost.
    Serve,
    Help,
}

#[derive(Debug)]
pub struct Options {
    pub command: Command,
    pub out: Option<PathBuf>,
    /// The checkout to read from. Found by walking up from the working
    /// directory when it is not given.
    pub root: Option<PathBuf>,
    pub port: u16,
    /// Poll the source files and rebuild when one of them changes.
    pub watch: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            command: Command::Build,
            out: None,
            root: None,
            port: DEFAULT_PORT,
            watch: false,
        }
    }
}

pub const USAGE: &str = "\
buri documentation site

    cargo run -p website                 build the site into target/docs-site
    cargo run -p website -- serve        build it and serve it on localhost
    cargo run -p website -- --check      build into a temporary directory and
                                         check every link, exiting non-zero on
                                         the first broken one

    --out <dir>     write the site here instead
    --root <dir>    read the checkout here instead of the one above the
                    working directory
    --port <n>      the port `serve` listens on (default 8080)
    --watch         with `serve`, poll the source files and rebuild on a change
    --help          this
";

pub fn parse(arguments: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut at = 0usize;
    while let Some(argument) = arguments.get(at) {
        at = at.saturating_add(1);
        // The separated form of a flag that takes a value; `None` for the rest.
        let mut following = || -> Result<String, String> {
            let value =
                arguments.get(at).cloned().ok_or_else(|| format!("`{argument}` takes a value"))?;
            at = at.saturating_add(1);
            Ok(value)
        };
        match argument.as_str() {
            "serve" => options.command = Command::Serve,
            "--check" => options.command = Command::Check,
            "--help" | "-h" => options.command = Command::Help,
            "--watch" => options.watch = true,
            "--out" => options.out = Some(PathBuf::from(following()?)),
            "--root" => options.root = Some(PathBuf::from(following()?)),
            "--port" => options.port = port(&following()?)?,
            other => {
                if let Some(path) = other.strip_prefix("--out=") {
                    options.out = Some(PathBuf::from(path));
                } else if let Some(path) = other.strip_prefix("--root=") {
                    options.root = Some(PathBuf::from(path));
                } else if let Some(raw) = other.strip_prefix("--port=") {
                    options.port = port(raw)?;
                } else {
                    return Err(format!("unknown argument `{other}`\n\n{USAGE}"));
                }
            }
        }
    }
    Ok(options)
}

fn port(raw: &str) -> Result<u16, String> {
    raw.parse().map_err(|_| format!("`--port` takes a number, not `{raw}`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(arguments: &[&str]) -> Options {
        let owned: Vec<String> = arguments.iter().map(|a| (*a).to_string()).collect();
        match parse(&owned) {
            Ok(options) => options,
            Err(why) => panic!("{why}"),
        }
    }

    #[test]
    fn no_arguments_builds_into_the_default_directory() {
        let options = parsed(&[]);
        assert_eq!(options.command, Command::Build);
        assert!(options.out.is_none());
    }

    #[test]
    fn a_value_may_be_written_either_way_round() {
        assert_eq!(parsed(&["--out", "/tmp/site"]).out, Some(PathBuf::from("/tmp/site")));
        assert_eq!(parsed(&["--out=/tmp/site"]).out, Some(PathBuf::from("/tmp/site")));
        assert_eq!(parsed(&["serve", "--port=9000", "--watch"]).port, 9000);
        assert!(parsed(&["serve", "--watch"]).watch);
    }

    #[test]
    fn an_unknown_argument_is_refused_rather_than_ignored() {
        assert!(parse(&["--colour".to_string()]).is_err());
        assert!(parse(&["--port".to_string()]).is_err());
        assert!(parse(&["--port=eight".to_string()]).is_err());
    }
}
