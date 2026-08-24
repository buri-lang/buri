//! The one table that describes the CLI.
//!
//! `main` dispatches through it, `arguments::usage()` is printed from it, and
//! `buri docs cli <command>` is rendered from it. There is no second list to
//! keep in step, so the help text, the reference, and what the binary actually
//! does cannot disagree — which they did: `--fix` and `--shuffle` parsed and
//! did nothing, and `--check-reproducible` and `query --output=proto` were
//! documented and rejected.
//!
//! Two rules follow from the table that were not enforceable before:
//!
//!   * **A flag belongs to the commands that read it.** `buri build
//!     --filter=x` used to parse silently; now it is an unknown flag for
//!     `build`, with a message naming the commands that do take it.
//!   * **A flag that nothing reads cannot be listed.** Deleting a flag from
//!     `FLAGS` is the only way to stop accepting it, and the reference is
//!     generated from the same list, so a flag cannot be documented into
//!     existence.
//!
//! Adding a command is one entry here. Dispatch, `--help`, the reference page,
//! the manifest, and search all follow.

pub mod arguments;
pub mod build;
pub mod clean;
pub mod format;
pub mod generate;
pub mod lint;
pub mod query;
pub mod run;
pub mod test;
pub mod version;
pub mod watch;

use crate::commands::arguments::{Args, Flags};
use std::fmt::Write as _;

/// What a flag does with its `=value`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Value {
    /// `--force`
    None,
    /// `--output=<selector>`
    Required(&'static str),
    /// `--shuffle` or `--shuffle=off`
    Optional(&'static str),
}

pub struct Flag {
    pub name: &'static str,
    pub value: Value,
    /// The accepted values, when there is a closed set. Printed in the
    /// reference, so an agent does not have to guess.
    pub choices: &'static [&'static str],
    pub blurb: &'static str,
    /// Accepted by every command. Kept small on purpose.
    pub global: bool,
    pub set: fn(&mut Flags, Option<&str>) -> Result<(), String>,
}

/// What a command does with its non-flag arguments.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Targets {
    /// Takes none.
    None,
    /// Labels and patterns; with none, the package containing the cwd.
    Any,
    /// Exactly one binary target.
    OneBinary,
    /// A query expression, as in `deps(//cmd/server)`.
    Expression,
    /// Paths on disk rather than labels.
    Paths,
    /// A documentation id, a subcommand, or nothing.
    DocId,
}

pub struct Command {
    pub name: &'static str,
    /// How the synopsis line shows the argument.
    pub args: &'static str,
    /// The one-line description in `buri --help`.
    pub blurb: &'static str,
    /// The prose page. It contains no flag list — the flag table is generated
    /// from `flags` below, so the two cannot drift apart.
    pub doc: &'static str,
    /// Which entries in `FLAGS` this command accepts, beyond the global ones.
    pub flags: &'static [&'static str],
    pub targets: Targets,
    /// `docs` and `version` answer without a repository; everything else needs
    /// one, and says so rather than failing obscurely.
    pub needs_repo: bool,
    pub run: fn(&Args) -> i32,
    /// Listed in the reference but not in the short help.
    pub hidden: bool,
}

// ---------------------------------------------------------------------------
// Flags
// ---------------------------------------------------------------------------

pub const FLAGS: &[Flag] = &[
    Flag {
        name: "release",
        value: Value::None,
        choices: &[],
        blurb: "optimize and minify; the default is a readable debug build",
        global: false,
        set: |f, _| {
            f.mode = arguments::BuildMode::Release;
            Ok(())
        },
    },
    Flag {
        name: "debug",
        value: Value::None,
        choices: &[],
        blurb: "the default build mode, stated explicitly",
        global: false,
        set: |f, _| {
            f.mode = arguments::BuildMode::Debug;
            Ok(())
        },
    },
    Flag {
        name: "check",
        value: Value::None,
        choices: &[],
        blurb: "report what would change and exit 1, writing nothing",
        global: false,
        set: |f, _| {
            f.check = true;
            Ok(())
        },
    },
    Flag {
        name: "fix",
        value: Value::None,
        choices: &[],
        blurb: "apply the findings that have one mechanical answer",
        global: false,
        set: |f, _| {
            f.fix = true;
            Ok(())
        },
    },
    Flag {
        name: "force",
        value: Value::None,
        choices: &[],
        blurb: "ignore the cache and run the action",
        global: false,
        set: |f, _| {
            f.force = true;
            Ok(())
        },
    },
    Flag {
        name: "check-reproducible",
        value: Value::None,
        choices: &[],
        blurb: "build twice in separate directories and compare the artifacts byte for byte",
        global: false,
        set: |f, _| {
            f.check_reproducible = true;
            Ok(())
        },
    },
    Flag {
        name: "accept",
        value: Value::None,
        choices: &[],
        blurb: "rewrite the golden files a suite declares in `test { data }` from what it produced",
        global: false,
        set: |f, _| {
            f.accept = true;
            Ok(())
        },
    },
    Flag {
        name: "outputs",
        value: Value::None,
        choices: &[],
        blurb: "remove build outputs but keep the action cache",
        global: false,
        set: |f, _| {
            f.outputs_only = true;
            Ok(())
        },
    },
    Flag {
        name: "output",
        value: Value::Required("<selector>"),
        choices: &[],
        blurb: "name one output: which to build, or which a suite runs on",
        global: false,
        set: |f, v| {
            f.output = v.map(String::from);
            Ok(())
        },
    },
    Flag {
        name: "filter",
        value: Value::Required("<substring>"),
        choices: &[],
        blurb: "run only the tests whose name contains this",
        global: false,
        set: |f, v| {
            f.filter = v.map(String::from);
            Ok(())
        },
    },
    Flag {
        name: "watch",
        value: Value::None,
        choices: &[],
        blurb: "re-run on every change to a declared input, until interrupted",
        global: false,
        set: |f, _| {
            f.watch = true;
            Ok(())
        },
    },
    Flag {
        name: "self-check",
        value: Value::None,
        choices: &[],
        blurb: "type-check the embedded standard library against itself",
        global: false,
        set: |f, _| {
            f.self_check = true;
            Ok(())
        },
    },
    Flag {
        name: "explain",
        value: Value::None,
        choices: &[],
        blurb: "one line per action: whether it ran or the cache served it, and the key",
        global: false,
        set: |f, _| {
            f.explain = true;
            Ok(())
        },
    },
    Flag {
        name: "format",
        value: Value::Required("<format>"),
        choices: &["human", "markdown", "json"],
        blurb: "how `buri docs` prints a page",
        global: false,
        set: |f, v| {
            f.format = match v {
                Some("json") => arguments::Format::Json,
                Some("markdown") | Some("md") => arguments::Format::Markdown,
                Some("human") | None => arguments::Format::Human,
                Some(other) => {
                    return Err(format!(
                        "unknown --format `{other}`; expected `human`, `markdown`, or `json`"
                    ))
                }
            };
            Ok(())
        },
    },
    Flag {
        name: "dense",
        value: Value::None,
        choices: &[],
        blurb: "headings and examples only — fewer tokens, every example kept",
        global: false,
        set: |f, _| {
            f.dense = true;
            Ok(())
        },
    },
    Flag {
        name: "verbose",
        value: Value::None,
        choices: &[],
        blurb: "say more about what was skipped and why",
        global: true,
        set: |f, _| {
            f.verbose = true;
            Ok(())
        },
    },
    Flag {
        name: "color",
        value: Value::Optional("<when>"),
        choices: &["always", "never"],
        blurb: "ANSI escapes; also honours NO_COLOR",
        global: true,
        set: |f, v| {
            f.color = Some(!matches!(v, Some("never" | "off" | "0")));
            Ok(())
        },
    },
    Flag {
        name: "error-format",
        value: Value::Required("<format>"),
        choices: &["human", "json"],
        blurb: "diagnostics as one JSON object per line, for tools and coding agents",
        global: true,
        set: |f, v| {
            f.error_format = match v {
                Some("json") => arguments::ErrorFormat::Json,
                Some("human") | None => arguments::ErrorFormat::Human,
                Some(other) => {
                    return Err(format!(
                        "unknown --error-format `{other}`; expected `human` or `json`"
                    ))
                }
            };
            Ok(())
        },
    },
];

pub fn flag(name: &str) -> Option<&'static Flag> {
    FLAGS.iter().find(|f| f.name == name)
}

/// The commands that accept a flag, for the "you wanted one of these" message.
pub fn commands_taking(name: &str) -> Vec<&'static str> {
    COMMANDS.iter().filter(|c| c.flags.contains(&name)).map(|c| c.name).collect()
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

pub const COMMANDS: &[Command] = &[
    Command {
        name: "build",
        args: "[targets]",
        blurb: "compile",
        doc: include_str!("../docs/cli/build.md"),
        flags: &["release", "debug", "output", "force", "explain", "check-reproducible"],
        targets: Targets::Any,
        needs_repo: true,
        run: build::cmd_build,
        hidden: false,
    },
    Command {
        name: "test",
        args: "[targets]",
        blurb: "compile and run test suites",
        doc: include_str!("../docs/cli/test.md"),
        flags: &["release", "debug", "output", "filter", "force", "accept", "explain", "watch"],
        targets: Targets::Any,
        needs_repo: true,
        run: test::cmd_test,
        hidden: false,
    },
    Command {
        name: "run",
        args: "<target> [-- args]",
        blurb: "build one binary and execute it",
        doc: include_str!("../docs/cli/run.md"),
        flags: &["release", "debug", "output", "force", "explain"],
        targets: Targets::OneBinary,
        needs_repo: true,
        run: run::cmd_run,
        hidden: false,
    },
    Command {
        name: "format",
        args: "[paths]",
        blurb: "format .buri sources and BUILD.buri files",
        doc: include_str!("../docs/cli/format.md"),
        flags: &["check"],
        targets: Targets::Paths,
        needs_repo: true,
        run: format::cmd_format,
        hidden: false,
    },
    Command {
        name: "lint",
        args: "[targets]",
        blurb: "static checks beyond type checking",
        doc: include_str!("../docs/cli/lint.md"),
        flags: &["fix"],
        targets: Targets::Any,
        needs_repo: true,
        run: lint::cmd_lint,
        hidden: false,
    },
    Command {
        name: "gen",
        args: "[targets]",
        blurb: "regenerate sources/deps in existing BUILD.buri files",
        doc: include_str!("../docs/cli/gen.md"),
        flags: &["check"],
        targets: Targets::Any,
        needs_repo: true,
        run: generate::cmd_gen,
        hidden: false,
    },
    Command {
        name: "query",
        args: "<expr>",
        blurb: "ask about the build graph",
        doc: include_str!("../docs/cli/query.md"),
        flags: &[],
        targets: Targets::Expression,
        needs_repo: true,
        run: query::cmd_query,
        hidden: false,
    },
    Command {
        name: "docs",
        args: "[topic]",
        blurb: "the language, the build system, and this CLI",
        doc: include_str!("../docs/cli/docs.md"),
        flags: &["format", "dense", "check"],
        targets: Targets::DocId,
        needs_repo: false,
        run: crate::documentation::cmd_docs,
        hidden: false,
    },
    Command {
        name: "lsp",
        args: "",
        blurb: "language server, over stdio",
        doc: include_str!("../docs/cli/lsp.md"),
        flags: &[],
        targets: Targets::None,
        needs_repo: true,
        run: crate::language_server::cmd_lsp,
        hidden: false,
    },
    Command {
        name: "clean",
        args: "",
        blurb: "drop the local cache",
        doc: include_str!("../docs/cli/clean.md"),
        flags: &["outputs"],
        targets: Targets::None,
        needs_repo: true,
        run: clean::cmd_clean,
        hidden: false,
    },
    Command {
        name: "version",
        args: "",
        blurb: "toolchain version, and --verbose its executable's hash",
        doc: include_str!("../docs/cli/version.md"),
        flags: &["self-check"],
        targets: Targets::None,
        needs_repo: false,
        run: version::cmd_version,
        hidden: false,
    },
];

pub fn find(name: &str) -> Option<&'static Command> {
    // `doc` is the obvious mistype of the command an agent reaches for most.
    let name = if name == "doc" { "docs" } else { name };
    COMMANDS.iter().find(|c| c.name == name)
}

/// True when `command` accepts `flag`.
pub fn accepts(command: &str, flag_name: &str) -> bool {
    if flag(flag_name).is_some_and(|f| f.global) {
        return true;
    }
    find(command).is_some_and(|c| c.flags.contains(&flag_name))
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// `buri --help`, generated.
pub fn usage() -> String {
    let mut out = String::from("buri — the toolchain for the Buri language\n\n");
    let width = COMMANDS.iter().filter(|c| !c.hidden).map(|c| c.args.len()).max().unwrap_or(0);
    for c in COMMANDS.iter().filter(|c| !c.hidden) {
        let _ = writeln!(out, "  buri {:<8} {:<width$}  {}", c.name, c.args, c.blurb);
    }
    out.push_str(
        "\nTarget arguments accept labels and patterns: //lib/money, //lib/..., //...\n\
         With no argument, commands operate on the package containing the working\n\
         directory.\n\n",
    );
    for f in FLAGS.iter().filter(|f| f.global) {
        let _ = writeln!(out, "  {:<24} {}", spelling(f), f.blurb);
    }
    out.push_str(
        "\n`buri docs` is the documentation: it works outside a repository, every\n\
         example in it is compiled by the test suite, and `buri docs search <words>`\n\
         looks inside every page at once. `buri docs cli <command>` documents one\n\
         command and every flag it takes.\n",
    );
    out
}

fn spelling(f: &Flag) -> String {
    match f.value {
        Value::None => format!("--{}", f.name),
        Value::Required(v) => format!("--{}={v}", f.name),
        Value::Optional(v) => format!("--{}[={v}]", f.name),
    }
}

/// The reference page for one command: a generated synopsis, a generated flag
/// table, then the prose. The prose never lists a flag, so it cannot be wrong
/// about one.
pub fn reference(c: &Command) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# buri {}\n", c.name);
    let _ = writeln!(out, "{}\n", c.blurb);
    let _ = writeln!(out, "```text\nburi {} {}\n```\n", c.name, c.args);

    let mut rows: Vec<&Flag> = c.flags.iter().filter_map(|n| flag(n)).collect();
    rows.extend(FLAGS.iter().filter(|f| f.global));
    if !rows.is_empty() {
        out.push_str("## Flags\n\n| Flag | Meaning |\n|---|---|\n");
        for f in rows {
            let choices = if f.choices.is_empty() {
                String::new()
            } else {
                format!(" (`{}`)", f.choices.join("`, `"))
            };
            let _ = writeln!(out, "| `{}` | {}{} |", spelling(f), f.blurb, choices);
        }
        out.push('\n');
    }

    let _ = writeln!(
        out,
        "Exit codes: `0` success · `1` the thing you asked about is wrong · \
         `2` the thing you asked *with* is wrong.\n"
    );
    out.push_str(c.doc);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_command_is_unique_and_documented() {
        let mut seen = HashSet::new();
        for c in COMMANDS {
            assert!(seen.insert(c.name), "`{}` is registered twice", c.name);
            assert!(!c.doc.trim().is_empty(), "`buri {}` has no prose page", c.name);
        }
    }

    /// Every flag a command claims exists, and every flag that exists is
    /// reachable from some command. A flag nothing accepts is dead weight the
    /// parser would still take.
    #[test]
    fn flags_and_commands_agree() {
        for c in COMMANDS {
            for name in c.flags {
                assert!(flag(name).is_some(), "`buri {}` lists unknown flag `--{name}`", c.name);
                assert!(
                    !flag(name).unwrap().global,
                    "`--{name}` is global; `buri {}` should not list it",
                    c.name
                );
            }
        }
        for f in FLAGS {
            if f.global {
                continue;
            }
            assert!(
                !commands_taking(f.name).is_empty(),
                "`--{}` is accepted by no command; delete it or wire it up",
                f.name
            );
        }
    }

    /// The generated help names every command a user can type.
    #[test]
    fn usage_lists_every_command() {
        let text = usage();
        for c in COMMANDS.iter().filter(|c| !c.hidden) {
            assert!(text.contains(c.name), "usage omits `{}`", c.name);
        }
    }

    /// A command's prose page must not contain a flag table — the generated
    /// one is authoritative, and a second copy is the drift this table exists
    /// to end.
    #[test]
    fn no_prose_page_lists_flags() {
        for c in COMMANDS {
            for line in c.doc.lines() {
                let t = line.trim_start();
                assert!(
                    !(t.starts_with("| `--") || t.starts_with("- `--")),
                    "`buri {}`'s page tabulates `{}`; flags come from the table",
                    c.name,
                    t.split_whitespace().nth(1).unwrap_or(t)
                );
            }
        }
    }

    #[test]
    fn the_reference_renders_for_every_command() {
        for c in COMMANDS {
            let text = reference(c);
            assert!(text.contains(&format!("buri {}", c.name)));
            assert!(text.contains("Exit codes"));
        }
    }
}
