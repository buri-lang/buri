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

pub const USAGE: &str = "\
buri — the toolchain for the Buri language

  buri build   [targets]   compile
  buri test    [targets]   compile and run test suites
  buri run     <target>    build one binary and execute it
  buri format  [paths]     format .buri sources and BUILD.buri files
  buri lint    [targets]   static checks beyond type checking
  buri gen     [targets]   regenerate sources/deps in existing BUILD.buri files
  buri query   <expr>      ask about the build graph
  buri clean               drop the local cache
  buri lsp                 language server, over stdio
  buri version             toolchain version and the REPO.buri pin

Target arguments accept labels and patterns: //lib/money, //lib/..., //...
With no argument, commands operate on the package containing the working
directory.
";

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
    pub fix: bool,
    pub force: bool,
    pub accept: bool,
    pub outputs_only: bool,
    pub output: Option<String>,
    pub filter: Option<String>,
    pub shuffle: Option<String>,
    pub color: Option<bool>,
    pub self_check: bool,
    pub verbose: bool,
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
            match name {
                "release" => flags.release = true,
                "debug" => flags.debug = true,
                "check" => flags.check = true,
                "fix" => flags.fix = true,
                "force" => flags.force = true,
                "accept" => flags.accept = true,
                "outputs" => flags.outputs_only = true,
                "self-check" => flags.self_check = true,
                "verbose" => flags.verbose = true,
                "output" => flags.output = value,
                "filter" => flags.filter = value,
                "shuffle" => flags.shuffle = Some(value.unwrap_or_else(|| "on".into())),
                "color" => {
                    flags.color = Some(!matches!(value.as_deref(), Some("never" | "off" | "0")))
                }
                "help" => return Err(String::new()),
                other => return Err(format!("unknown flag `--{other}`")),
            }
            continue;
        }
        targets.push(arg.clone());
    }

    if flags.release && flags.debug {
        return Err("`--release` and `--debug` are exclusive".into());
    }
    Ok(Args { command, targets, flags, passthrough })
}

pub struct Session {
    pub root: PathBuf,
    pub map: SourceMap,
    pub diags: Diagnostics,
    pub ws: Workspace,
    pub color: bool,
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
    let color = flags.color.unwrap_or_else(|| std::env::var("NO_COLOR").is_err());
    Ok(Session { root, map, diags, ws, color })
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
            eprint!("{}", self.map.render(d, self.color));
            had_error |= d.is_error();
        }
        self.diags.items.clear();
        had_error
    }

    pub fn print(&self, diags: &Diagnostics) -> bool {
        let mut had_error = false;
        for d in &diags.items {
            eprint!("{}", self.map.render(d, self.color));
            had_error |= d.is_error();
        }
        had_error
    }
}
