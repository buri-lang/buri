//! `buri init`.
//!
//! The first thing somebody meeting Buri has to do is the one thing the
//! documentation cannot do for them: put a `REPO.buri`, a library, a binary and
//! a test suite on disk in the arrangement the rest of the toolchain assumes.
//! Copying that out of a page is where a first session goes wrong — a
//! two-space indent the formatter rewrites, a library with no
//! `visibility`, a `lib.buri` listed in its own `sources`.
//!
//! So the scaffold is compiled into the binary, like the skills and the
//! documentation, and it is the *formatter's own output*: `buri build`,
//! `buri test`, `buri lint`, `buri gen --check` and `buri format --check` are
//! all clean on a freshly generated repository, and
//! `a_generated_repository_builds_and_tests` in `cli/tests/build/init.rs`
//! holds them to it.
//!
//! The one rule here is that **nothing a user wrote is ever written over**. A
//! `REPO.buri` at the target means the directory is already a repository and
//! the command refuses; any other collision refuses too, before the first byte
//! is written, so a refusal never leaves half a scaffold behind. The single
//! exception is an existing `.gitignore`, because it is the one scaffold file
//! whose name git owns — a directory that is already a git repository has one,
//! and refusing over it would make `buri init` useless exactly where people
//! start. Entries are line-keyed and order-independent, so the build's entries
//! are appended below whatever the user wrote, and nothing of theirs moves.
//!
//! A `REPO.buri` *above* the target is refused for a different reason, and it
//! is the one that bites hardest. Nesting one is not a collision — no file is
//! written over — but `find_root` stops at the outermost marker it walks up
//! to, so the inner one is not a root at all: it is a stray build file inside
//! somebody else's repository, and it breaks their `buri build //...` on the
//! next run. A generator that leaves a working directory worse than it found
//! it has to refuse.
#![allow(
    clippy::print_stdout,
    reason = "what was written is this command's output; diagnostics still leave \
              through `Session::emit`"
)]

use crate::commands::add::skills::{self, Outcome};
use crate::commands::arguments::Args;
use std::path::{Path, PathBuf};

/// One generated file: where it lands under the target directory, and the
/// bytes that land there.
pub struct ScaffoldFile {
    pub path: &'static str,
    pub text: &'static str,
}

/// The file whose presence makes a directory a repository root, and so the one
/// this command checks for before it does anything at all.
const REPOSITORY_FILE: &str = "REPO.buri";

/// The one scaffold file an existing copy of does not refuse the run: git owns
/// this name, so a directory that is already a git repository has one, and the
/// scaffold's entries are merged into it instead.
const IGNORE_FILE: &str = ".gitignore";

/// The generated repository, in the order the lines are printed.
///
/// The sources live under `docs/init/` as real files rather than as string
/// literals so that they are edited as Buri — the formatter's checked-in
/// build-file test walks them, and a reader diffs a `.buri` rather than an
/// escaped constant. The ignore file is stored without its leading dot because
/// a `.gitignore` inside the toolchain's own tree would be read by git as one.
pub const SCAFFOLD: &[ScaffoldFile] = &[
    ScaffoldFile { path: "REPO.buri", text: include_str!("../docs/init/REPO.buri") },
    ScaffoldFile { path: ".gitignore", text: include_str!("../docs/init/gitignore") },
    ScaffoldFile {
        path: "libs/greeting/BUILD.buri",
        text: include_str!("../docs/init/libs/greeting/BUILD.buri"),
    },
    ScaffoldFile {
        path: "libs/greeting/lib.buri",
        text: include_str!("../docs/init/libs/greeting/lib.buri"),
    },
    ScaffoldFile {
        path: "libs/greeting/greeting.buri",
        text: include_str!("../docs/init/libs/greeting/greeting.buri"),
    },
    ScaffoldFile {
        path: "libs/greeting/test/greeting.buri",
        text: include_str!("../docs/init/libs/greeting/test/greeting.buri"),
    },
    ScaffoldFile {
        path: "apps/hello/BUILD.buri",
        text: include_str!("../docs/init/apps/hello/BUILD.buri"),
    },
    ScaffoldFile {
        path: "apps/hello/main.buri",
        text: include_str!("../docs/init/apps/hello/main.buri"),
    },
];

pub fn command_init(args: &Args) -> i32 {
    let root = match args.targets.as_slice() {
        [] => PathBuf::from("."),
        [directory] => PathBuf::from(directory),
        _ => {
            report("`buri init` takes at most one directory");
            return 2;
        }
    };
    match generate(&root) {
        Ok(done) => {
            for (outcome, path) in done {
                let shown = path.strip_prefix(&root).unwrap_or(&path);
                println!("{} {}", outcome.verb(), shown.display());
            }
            0
        }
        Err(why) => {
            report(&why);
            2
        }
    }
}

/// Writes the scaffold and the agent skills under `root`, creating it if it is
/// not there.
///
/// Every collision is found before anything is written. A generator that
/// discovers the third file already exists has by then replaced the first two,
/// and there is nothing a user can do with that half-repository except undo it
/// by hand.
pub fn generate(root: &Path) -> Result<Vec<(Outcome, PathBuf)>, String> {
    if root.join(REPOSITORY_FILE).exists() {
        return Err(format!(
            "`{}` is already a Buri repository; `buri init` never writes over one",
            root.display()
        ));
    }
    if let Some(enclosing) = enclosing_repository(root) {
        return Err(format!(
            "`{}` is inside the Buri repository at `{}`; a repository cannot hold another",
            root.display(),
            enclosing.display()
        ));
    }
    for file in SCAFFOLD {
        let path = root.join(file.path);
        if path.exists() && file.path != IGNORE_FILE {
            return Err(format!(
                "`{}` already exists; `buri init` never writes over a file",
                path.display()
            ));
        }
    }

    std::fs::create_dir_all(root)
        .map_err(|e| format!("cannot create {}: {e}", root.display()))?;
    let mut done = Vec::new();
    for file in SCAFFOLD {
        let path = root.join(file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        if file.path == IGNORE_FILE && path.exists() {
            if merge_ignore(&path, file.text)? {
                done.push((Outcome::Updated, path));
            }
            continue;
        }
        std::fs::write(&path, file.text)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        done.push((Outcome::Wrote, path));
    }
    // The skills come from `buri add skills` rather than from a second copy
    // here: a repository generated by one release and a repository upgraded by
    // the next must end up with the same `.agent/skills`.
    done.extend(skills::install(root)?);
    Ok(done)
}

/// Appends whatever the scaffold's ignore file has that `path` does not,
/// leaving every line the user wrote where it stands. Says whether anything
/// was appended: a `.gitignore` that already ignores everything the build
/// writes is left byte-for-byte alone, so re-running is not an edit.
///
/// Lines are matched whole and trimmed — `out` is `out` however it is
/// indented, but it is not `out/`, and a near-miss is appended rather than
/// guessed at, since one redundant entry is harmless and one missing entry is
/// a build product committed.
fn merge_ignore(path: &Path, scaffold: &str) -> Result<bool, String> {
    let existing = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let have: std::collections::HashSet<String> =
        existing.lines().map(|line| line.trim().to_string()).collect();
    let is_entry = |line: &&str| {
        let line = line.trim();
        !line.is_empty() && !line.starts_with('#')
    };
    if scaffold.lines().filter(is_entry).all(|line| have.contains(line.trim())) {
        return Ok(false);
    }
    let mut merged = existing;
    if !merged.ends_with('\n') && !merged.is_empty() {
        merged.push('\n');
    }
    if !merged.is_empty() {
        merged.push('\n');
    }
    // The whole missing tail of the scaffold, comment included: an entry the
    // user already has is skipped, so what lands is only what was absent.
    for line in scaffold.lines().filter(|line| !have.contains(line.trim())) {
        merged.push_str(line);
        merged.push('\n');
    }
    std::fs::write(path, merged).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(true)
}

/// The root of the repository `root` would sit inside, if there is one.
///
/// Strictly above the target: a `REPO.buri` *at* it is the other refusal, and
/// it is checked first so that re-running in place still says "already a Buri
/// repository" rather than reporting itself as its own container.
fn enclosing_repository(root: &Path) -> Option<PathBuf> {
    let resolved = resolved(root);
    crate::build::workspace::find_root(resolved.parent()?)
}

/// `path` as an absolute path with `.` and `..` resolved.
///
/// Both halves of this are load-bearing. `find_root` walks up by popping
/// components, so `buri init ../elsewhere` would have it climb back into the
/// directory the `..` just left, and a relative `sub` would run out of
/// components before reaching the root that encloses it. And the target need
/// not exist yet, so the deepest ancestor that *does* is canonicalized and the
/// rest is appended — which is also what resolves a symlinked parent.
fn resolved(path: &Path) -> PathBuf {
    let mut head = match std::env::current_dir() {
        Ok(working) => working.join(path),
        Err(_) => path.to_path_buf(),
    };
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(real) = head.canonicalize() {
            let mut out = real;
            for name in tail.iter().rev() {
                out.push(name);
            }
            return out;
        }
        // A component that is not a name — `..` or the root itself — is where
        // the peeling stops: there is nothing sensible to put back on.
        let Some(name) = head.file_name().map(std::ffi::OsStr::to_os_string) else {
            return head;
        };
        tail.push(name);
        if !head.pop() {
            return head;
        }
    }
}

#[expect(
    clippy::print_stderr,
    reason = "a directory that cannot be written is this command's own failure, and there is no \
              repository session to report it through"
)]
fn report(message: &str) {
    eprintln!("error: {message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory nobody else can see, emptied first so a previous
    /// run's leftovers cannot make a test pass.
    fn scratch(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("buri-init-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        directory
    }

    #[test]
    fn a_run_writes_the_whole_repository() {
        let root = scratch("fresh");
        let done = generate(&root).unwrap();

        // The directory did not exist beforehand, so `generate` made it.
        assert!(root.join(REPOSITORY_FILE).is_file());
        for file in SCAFFOLD {
            let path = root.join(file.path);
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                file.text,
                "{} is not what the binary ships",
                file.path
            );
            assert!(
                done.iter().any(|(outcome, at)| *outcome == Outcome::Wrote && *at == path),
                "{} was written but not reported",
                file.path
            );
        }
        // The ignore file is the one whose name changes on the way out.
        assert!(root.join(".gitignore").is_file(), "the ignore file keeps its leading dot");

        for skill in skills::SKILLS {
            let path = root.join(".agent/skills").join(skill.name).join("SKILL.md");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), skill.text);
        }
        assert_eq!(
            done.len(),
            SCAFFOLD.len() + skills::SKILLS.len(),
            "one line per file and nothing else"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The whole safety contract: a second run is refused, and refused without
    /// having touched anything.
    #[test]
    fn a_second_run_is_refused_and_changes_nothing() {
        let root = scratch("rerun");
        generate(&root).unwrap();
        let mine = "// mine, and edited\nexport fn greeting(): Str {\n    \"goodbye\"\n}\n";
        std::fs::write(root.join("libs/greeting/greeting.buri"), mine).unwrap();

        let refused = generate(&root).unwrap_err();
        assert!(
            refused.contains("already a Buri repository"),
            "a repository must be named as the reason: {refused}"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("libs/greeting/greeting.buri")).unwrap(),
            mine,
            "a refused run leaves every file exactly as it was"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A directory that is not yet a repository but already holds one of the
    /// paths the scaffold wants. Nothing is written, not even the files that
    /// come before the collision in the table.
    #[test]
    fn a_colliding_file_stops_the_run_before_any_write() {
        let root = scratch("collision");
        std::fs::create_dir_all(root.join("apps/hello")).unwrap();
        std::fs::write(root.join("apps/hello/main.buri"), "// somebody's own\n").unwrap();

        let refused = generate(&root).unwrap_err();
        assert!(refused.contains("apps/hello/main.buri"), "the collision must be named: {refused}");
        assert!(!root.join(REPOSITORY_FILE).exists(), "nothing is written before the check");
        assert!(!root.join("libs").exists(), "nothing is written before the check");
        assert_eq!(
            std::fs::read_to_string(root.join("apps/hello/main.buri")).unwrap(),
            "// somebody's own\n"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The one collision that is not a refusal: a directory that is already a
    /// git repository has a `.gitignore`, and `buri init` there must merge the
    /// build's entries in below the user's lines rather than stop.
    #[test]
    fn an_existing_gitignore_is_merged_not_written_over() {
        let root = scratch("gitignore");
        std::fs::create_dir_all(&root).unwrap();
        let theirs = "# mine\nnode_modules/\n";
        std::fs::write(root.join(IGNORE_FILE), theirs).unwrap();

        let done = generate(&root).unwrap();
        let merged = std::fs::read_to_string(root.join(IGNORE_FILE)).unwrap();
        assert!(merged.starts_with(theirs), "the user's lines stay first and untouched");
        assert!(merged.contains("\n.buri/\n"), "the build's entries are appended: {merged}");
        assert!(merged.contains("\nout\n"), "the build's entries are appended: {merged}");
        assert!(
            done.iter().any(|(outcome, at)| *outcome == Outcome::Updated
                && *at == root.join(IGNORE_FILE)),
            "a merge is reported as an update, not a write"
        );
        // The rest of the scaffold landed as it does in an empty directory.
        assert!(root.join(REPOSITORY_FILE).is_file());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A `.gitignore` that already ignores everything the build writes is left
    /// byte-for-byte alone — no duplicate entries, no report line, and the
    /// lines it already has count however they are indented.
    #[test]
    fn a_gitignore_already_covering_the_build_is_untouched() {
        let root = scratch("gitignore-covered");
        std::fs::create_dir_all(&root).unwrap();
        let theirs = "node_modules/\n  .buri/\nout";
        std::fs::write(root.join(IGNORE_FILE), theirs).unwrap();

        let done = generate(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join(IGNORE_FILE)).unwrap(),
            theirs,
            "nothing to add means nothing is touched, trailing newline included"
        );
        assert!(
            !done.iter().any(|(_, at)| *at == root.join(IGNORE_FILE)),
            "an untouched file is not reported"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A merge appends only what is missing: an entry the user already has is
    /// not repeated, and a file with no final newline gains one before the
    /// tail rather than gluing onto the last line.
    #[test]
    fn a_merge_appends_only_the_missing_entries() {
        let root = scratch("gitignore-partial");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(IGNORE_FILE), "out").unwrap();

        generate(&root).unwrap();
        let merged = std::fs::read_to_string(root.join(IGNORE_FILE)).unwrap();
        assert_eq!(merged.matches("out").count(), 1, "an entry already there is not repeated");
        assert!(merged.starts_with("out\n"), "the unterminated last line is closed, not glued to");
        assert!(merged.contains(".buri/\n"), "the missing entry is what lands: {merged}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A repository cannot hold another. The nested `REPO.buri` would not be a
    /// root — `find_root` stops at the outer one — so it would be a stray build
    /// file breaking somebody else's `buri build //...`.
    #[test]
    fn a_directory_inside_a_repository_is_refused() {
        let outer = scratch("nested");
        std::fs::create_dir_all(&outer).unwrap();
        std::fs::write(outer.join(REPOSITORY_FILE), "# the enclosing repository\n").unwrap();

        // Two directories down, and not yet on disk: the walk has to reach the
        // root from a target that does not exist.
        let inner = outer.join("packages/greeting");
        let refused = generate(&inner).unwrap_err();
        assert!(
            refused.contains("is inside the Buri repository at"),
            "the enclosing root must be named as the reason: {refused}"
        );
        assert!(!inner.exists(), "a refused run does not even create the directory");
        assert!(!outer.join("packages").exists(), "nor any part of the path to it");

        // The same target spelled with a `..` climb: a lexical walk would have
        // gone up out of the repository and missed it.
        let sideways = outer.join("packages/../greeting");
        assert!(generate(&sideways).unwrap_err().contains("is inside the Buri repository at"));

        std::fs::remove_dir_all(&outer).unwrap();
    }

    /// The scaffold is a repository, not a pile of files: it declares a root,
    /// a library, a binary, and a suite. A table that lost one of those would
    /// still generate and would still be refused a second time.
    #[test]
    fn the_scaffold_is_a_whole_repository() {
        let paths: Vec<&str> = SCAFFOLD.iter().map(|file| file.path).collect();
        for wanted in [
            REPOSITORY_FILE,
            ".gitignore",
            "libs/greeting/BUILD.buri",
            "libs/greeting/lib.buri",
            "libs/greeting/test/greeting.buri",
            "apps/hello/BUILD.buri",
            "apps/hello/main.buri",
        ] {
            assert!(paths.contains(&wanted), "the scaffold no longer ships {wanted}");
        }
        for file in SCAFFOLD {
            assert!(!file.text.is_empty(), "{} is empty", file.path);
            assert!(file.text.ends_with('\n'), "{} does not end in a newline", file.path);
            assert!(!file.path.starts_with('/'), "{} is not relative", file.path);
        }
    }
}
