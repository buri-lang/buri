//! The one table that describes the CLI.
//!
//! `main` dispatches through it, `usage()` below is printed from it, and
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
//! the manifest, and search all follow. A command that is a namespace —
//! `buri add` — carries a second table of the same shape (`Subcommand`), read
//! by the same three readers, so `buri add skills` is one entry too.

pub mod add;
pub mod arguments;
pub mod build;
pub mod clean;
pub mod format;
pub mod generate;
pub mod init;
pub mod lint;
/// What the last lint pass found for a target, kept in `.buri/cache` so that a
/// second run re-analyses only the targets whose closure moved.
pub mod lint_cache;
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

/// One `buri <command> <subcommand>`, for a command that is a namespace.
///
/// It has no `flags` of its own: a namespace's flags belong to the command
/// that owns it, so `accepts` still answers from one place, and a subcommand
/// that needs a flag the parent does not take is the sign the family is really
/// two commands.
pub struct Subcommand {
    pub name: &'static str,
    /// How the synopsis line shows the argument.
    pub args: &'static str,
    pub blurb: &'static str,
    pub run: fn(&Args) -> i32,
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
    pub run: fn(&Args) -> i32,
    /// The commands under this one, when it is a namespace. Empty for every
    /// command that does its own work; `run` then dispatches through this
    /// table (`dispatch`) and does nothing else.
    pub subcommands: &'static [Subcommand],
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
        blurb: "headings and examples only — fewer tokens, every example kept; on a build, \
                diagnostics without the explanation under them",
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
        name: "init",
        args: "[directory]",
        blurb: "write a new repository: a library, a binary, a test, and the skills",
        doc: include_str!("../docs/cli/init.md"),
        flags: &[],
        run: init::command_init,
        subcommands: &[],
        hidden: false,
    },
    Command {
        name: "build",
        args: "[targets]",
        blurb: "compile",
        doc: include_str!("../docs/cli/build.md"),
        flags: &["release", "debug", "output", "force", "explain", "check-reproducible", "dense"],
        run: build::command_build,
        subcommands: &[],
        hidden: false,
    },
    Command {
        name: "test",
        args: "[targets]",
        blurb: "compile and run test suites",
        doc: include_str!("../docs/cli/test.md"),
        flags: &["release", "debug", "output", "filter", "force", "accept", "explain", "watch", "dense"],
        run: test::command_test,
        subcommands: &[],
        hidden: false,
    },
    Command {
        name: "run",
        args: "<target> [-- args]",
        blurb: "build one binary and execute it",
        doc: include_str!("../docs/cli/run.md"),
        flags: &["release", "debug", "output", "force", "explain", "dense"],
        run: run::command_run,
        subcommands: &[],
        hidden: false,
    },
    Command {
        name: "format",
        args: "[paths]",
        blurb: "format .buri sources and BUILD.buri files",
        doc: include_str!("../docs/cli/format.md"),
        flags: &["check"],
        run: format::command_format,
        subcommands: &[],
        hidden: false,
    },
    Command {
        name: "lint",
        args: "[targets]",
        blurb: "static checks beyond type checking",
        doc: include_str!("../docs/cli/lint.md"),
        flags: &["fix", "explain", "dense"],
        run: lint::command_lint,
        subcommands: &[],
        hidden: false,
    },
    Command {
        name: "gen",
        args: "[targets]",
        blurb: "regenerate sources/deps in existing BUILD.buri files",
        doc: include_str!("../docs/cli/gen.md"),
        flags: &["check"],
        run: generate::command_generate,
        subcommands: &[],
        hidden: false,
    },
    Command {
        name: "query",
        args: "<expr>",
        blurb: "ask about the build graph",
        doc: include_str!("../docs/cli/query.md"),
        flags: &[],
        run: query::command_query,
        subcommands: &[],
        hidden: false,
    },
    Command {
        name: "docs",
        args: "[topic]",
        blurb: "the language, the build system, and this CLI",
        doc: include_str!("../docs/cli/docs.md"),
        flags: &["format", "dense", "check"],
        run: crate::documentation::command_docs,
        subcommands: &[],
        hidden: false,
    },
    Command {
        name: "add",
        args: "<subcommand>",
        blurb: "write what this toolchain ships into an existing repository",
        doc: include_str!("../docs/cli/add.md"),
        flags: &[],
        run: add::command_add,
        subcommands: add::SUBCOMMANDS,
        hidden: false,
    },
    Command {
        name: "lsp",
        args: "",
        blurb: "language server, over stdio",
        doc: include_str!("../docs/cli/lsp.md"),
        flags: &[],
        run: crate::language_server::command_language_server,
        subcommands: &[],
        hidden: false,
    },
    Command {
        name: "clean",
        args: "",
        blurb: "drop the local cache",
        doc: include_str!("../docs/cli/clean.md"),
        flags: &["outputs"],
        run: clean::command_clean,
        subcommands: &[],
        hidden: false,
    },
    Command {
        name: "version",
        args: "",
        blurb: "toolchain version, and --verbose its executable's hash",
        doc: include_str!("../docs/cli/version.md"),
        flags: &["self-check"],
        run: version::command_version,
        subcommands: &[],
        hidden: false,
    },
];

pub fn find(name: &str) -> Option<&'static Command> {
    // `doc` is the obvious mistype of the command an agent reaches for most.
    let name = if name == "doc" { "docs" } else { name };
    COMMANDS.iter().find(|c| c.name == name)
}

/// The commands that used to be spelled differently, and what they are now.
///
/// A rename is not a typo, so `nearest` cannot answer it: `add-skills` is
/// seven edits from `add`, which is exactly the distance that stops a
/// suggestion from being noise. The old spelling still fails — an alias would
/// keep two names alive and nothing would ever retire the first — but it fails
/// naming its replacement, which is the whole of what somebody with the old
/// command in a script needs.
pub const RENAMED: &[(&str, &str)] = &[("add-skills", "add skills")];

/// What to type instead of `name`, when `name` is a command this toolchain
/// used to have.
pub fn renamed(name: &str) -> Option<&'static str> {
    RENAMED.iter().find(|(was, _)| *was == name).map(|(_, now)| *now)
}

/// `buri <command> <subcommand>`, dispatched through the command's own table.
///
/// The two refusals are the ones `main` makes for a command that is not there,
/// one level down and in the same order: what was wrong, the nearest real
/// spelling when there is one, then the list to choose from.
#[expect(
    clippy::print_stderr,
    reason = "a malformed invocation is reported by the CLI itself, before there is a session"
)]
pub fn dispatch(command: &'static str, subcommands: &'static [Subcommand], args: &Args) -> i32 {
    let Some((asked, rest)) = args.targets.split_first() else {
        eprintln!("error: `buri {command}` needs a subcommand");
        eprintln!();
        eprint!("{}", subcommand_usage(command));
        return 2;
    };
    let Some(sub) = subcommands.iter().find(|s| s.name == asked) else {
        eprintln!("error: there is no `buri {command}` subcommand `{asked}`");
        let names: Vec<&str> = subcommands.iter().map(|s| s.name).collect();
        if let Some(near) = crate::build::buildfile::nearest(asked, &names) {
            eprintln!("  = did you mean `buri {command} {near}`?");
        }
        eprintln!();
        eprint!("{}", subcommand_usage(command));
        return 2;
    };
    // The subcommand is handed the arguments *after* its own name, so a
    // subcommand's `run` reads `targets` exactly as a command's does and can
    // be moved between the two tables without being rewritten.
    (sub.run)(&Args {
        command: format!("{command} {}", sub.name),
        targets: rest.to_vec(),
        flags: args.flags.clone(),
        passthrough: args.passthrough.clone(),
    })
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

/// Every line of the command table: each command a user can type, and each
/// subcommand under it.
fn rows() -> Vec<(String, &'static str, &'static str)> {
    let mut rows = Vec::new();
    for c in COMMANDS.iter().filter(|c| !c.hidden) {
        rows.push((c.name.to_string(), c.args, c.blurb));
        for s in c.subcommands {
            rows.push((format!("{} {}", c.name, s.name), s.args, s.blurb));
        }
    }
    rows
}

/// The help for one namespace: what `buri add` alone prints, and what its two
/// refusals print under them. Generated from the same table that dispatches.
pub fn subcommand_usage(name: &str) -> String {
    let Some(c) = find(name) else { return String::new() };
    let mut out = format!("buri {} — {}\n\n", c.name, c.blurb);
    let name_width = c.subcommands.iter().map(|s| s.name.len()).max().unwrap_or(0);
    let width = c.subcommands.iter().map(|s| s.args.len()).max().unwrap_or(0);
    for s in c.subcommands {
        let _ = writeln!(
            out,
            "  buri {} {:<name_width$} {:<width$}  {}",
            c.name, s.name, s.args, s.blurb
        );
    }
    let _ = write!(out, "\n`buri docs cli {}` documents it in full.\n", c.name);
    out
}

/// `buri --help`, generated.
pub fn usage() -> String {
    let mut out = String::from("buri — the toolchain for the Buri language\n\n");
    // Both columns are measured rather than pinned: a name longer than the
    // widest one at the time the constant was chosen pushed every blurb on its
    // line out of the column and left the rest of the table where it was.
    //
    // A subcommand is a row like any other, under its own command and spelled
    // the way it is typed. Nesting it in a second, indented table would put
    // `buri add skills` nowhere a reader scanning the left column could find
    // it, and there is one line either way.
    let rows = rows();
    let name_width = rows.iter().map(|(n, _, _)| n.len()).max().unwrap_or(0);
    let width = rows.iter().map(|(_, a, _)| a.len()).max().unwrap_or(0);
    for (name, args, blurb) in &rows {
        let _ = writeln!(out, "  buri {name:<name_width$} {args:<width$}  {blurb}");
    }
    out.push_str(
        "\nTarget arguments accept labels and patterns: //lib/money, //lib/..., //...\n\
         With no argument, commands operate on the whole repository, wherever you\n\
         happen to be standing in it.\n\n",
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

    if !c.subcommands.is_empty() {
        out.push_str("## Subcommands\n\n| Subcommand | What it does |\n|---|---|\n");
        for s in c.subcommands {
            let _ = writeln!(out, "| `buri {} {} {}` | {} |", c.name, s.name, s.args, s.blurb);
        }
        out.push('\n');
    }

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

    /// The same two rules the commands are held to, one level down: a name is
    /// registered once, and it says what it does.
    #[test]
    fn every_subcommand_is_unique_and_described() {
        for c in COMMANDS {
            let mut seen = HashSet::new();
            for s in c.subcommands {
                assert!(seen.insert(s.name), "`buri {} {}` is registered twice", c.name, s.name);
                assert!(!s.blurb.trim().is_empty(), "`buri {} {}` has no blurb", c.name, s.name);
                assert!(
                    !s.name.contains(' '),
                    "`{}` is two words; a subcommand of a subcommand is another table",
                    s.name
                );
            }
        }
    }

    /// The generated help names every subcommand, spelled the way it is typed.
    #[test]
    fn usage_lists_every_subcommand() {
        let text = usage();
        for c in COMMANDS.iter().filter(|c| !c.hidden) {
            for s in c.subcommands {
                assert!(
                    text.contains(&format!("buri {} {}", c.name, s.name)),
                    "usage omits `buri {} {}`",
                    c.name,
                    s.name
                );
            }
        }
    }

    /// `buri add` alone, and both of its refusals, print this list.
    #[test]
    fn a_namespace_prints_what_it_can_be_asked_for() {
        let text = subcommand_usage("add");
        assert!(text.contains("buri add skills"), "the list omits the one subcommand there is");
        assert!(text.contains("buri docs cli add"), "the list does not say where the page is");
        assert!(subcommand_usage("nonesuch").is_empty(), "a command that is not there has no list");
    }

    /// A renamed command is *gone* — no alias, no hidden entry — and the
    /// spelling the hint offers is one the table can actually dispatch.
    #[test]
    fn a_renamed_command_is_gone_and_its_replacement_works() {
        for (was, now) in RENAMED {
            assert!(find(was).is_none(), "`{was}` was renamed but is still a command");
            assert_eq!(renamed(was), Some(*now));
            let mut words = now.split(' ');
            let c = find(words.next().unwrap_or_default())
                .unwrap_or_else(|| panic!("`{now}` names no command"));
            if let Some(sub) = words.next() {
                assert!(
                    c.subcommands.iter().any(|s| s.name == sub),
                    "`{now}` names no subcommand of `buri {}`",
                    c.name
                );
            }
            assert!(words.next().is_none(), "`{now}` is more words than a command and a subcommand");
        }
    }

    /// Both ways of asking for a subcommand that is not there: none named, and
    /// one named that does not exist. Each is the invocation being wrong, so
    /// each is exit 2.
    #[test]
    fn a_namespace_refuses_what_it_cannot_dispatch() {
        let args = |targets: &[&str]| Args {
            command: "add".to_string(),
            targets: targets.iter().map(|t| (*t).to_string()).collect(),
            flags: Flags::default(),
            passthrough: Vec::new(),
        };
        assert_eq!(dispatch("add", add::SUBCOMMANDS, &args(&[])), 2);
        assert_eq!(dispatch("add", add::SUBCOMMANDS, &args(&["nonesuch"])), 2);
    }

    #[test]
    fn the_reference_renders_for_every_command() {
        for c in COMMANDS {
            let text = reference(c);
            assert!(text.contains(&format!("buri {}", c.name)));
            assert!(text.contains("Exit codes"));
            for s in c.subcommands {
                assert!(
                    text.contains(&format!("buri {} {}", c.name, s.name)),
                    "`buri docs cli {}` omits its `{}` subcommand",
                    c.name,
                    s.name
                );
            }
        }
    }
}
