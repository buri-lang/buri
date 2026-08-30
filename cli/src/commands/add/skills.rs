//! `buri add skills`.
//!
//! The toolchain ships the skills a coding agent needs in order to work in a
//! Buri repository, for the same reason it ships its own documentation
//! (`documentation::topics`): a skill kept anywhere else is a second copy to
//! hold in step with the compiler, and it goes stale without anybody noticing.
//! These are compiled into the binary, so `buri add skills` works in a
//! directory that is not a repository and on a machine with no checkout.
//!
//! Re-running is the upgrade path, which is what decides the one rule here:
//! **a skill directory named `buri-...` is this toolchain's**, rewritten from
//! the binary every time, and removed when a release stops shipping it. A
//! directory named anything else belongs to whoever wrote it and is never read,
//! written, or removed.
#![allow(
    clippy::print_stdout,
    reason = "what was written is this command's output; diagnostics still leave \
              through `Session::emit`"
)]

use crate::commands::arguments::Args;
use std::path::{Path, PathBuf};

/// One skill, as it is written to disk.
pub struct Skill {
    /// The directory it lands in, and the `name` its own frontmatter declares.
    /// `every_skill_agrees_with_its_own_frontmatter` holds the two together.
    pub name: &'static str,
    pub text: &'static str,
}

/// The marker that makes a skill directory this toolchain's.
///
/// It is a naming convention rather than a manifest because the directory is
/// the only thing both sides can see: a file listing what we installed would
/// have to be found, parsed, and trusted before a stale skill could be
/// removed, and it would be wrong the moment somebody moved a directory.
const OFFICIAL: &str = "buri-";

/// Where a skill lands under the directory the command was pointed at.
const SKILLS_DIRECTORY: &str = ".claude/skills";

pub const SKILLS: &[Skill] = &[
    Skill { name: "buri-language", text: include_str!("../../docs/skills/buri-language.md") },
    Skill { name: "buri-types", text: include_str!("../../docs/skills/buri-types.md") },
    Skill { name: "buri-build", text: include_str!("../../docs/skills/buri-build.md") },
    Skill { name: "buri-testing", text: include_str!("../../docs/skills/buri-testing.md") },
    Skill { name: "buri-cli", text: include_str!("../../docs/skills/buri-cli.md") },
];

/// What happened to one path, in the word the command prints for it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Wrote,
    Overwrote,
    /// A `buri-` directory this toolchain no longer ships.
    Removed,
}

impl Outcome {
    pub fn verb(self) -> &'static str {
        match self {
            Outcome::Wrote => "wrote",
            Outcome::Overwrote => "overwrote",
            Outcome::Removed => "removed",
        }
    }
}

pub fn command_add_skills(args: &Args) -> i32 {
    let root = match args.targets.as_slice() {
        [] => PathBuf::from("."),
        [directory] => PathBuf::from(directory),
        _ => {
            report("`buri add skills` takes at most one directory");
            return 2;
        }
    };
    if !root.is_dir() {
        report(&format!("`{}` is not a directory", root.display()));
        return 2;
    }
    match install(&root) {
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

/// Writes every skill under `root`, refreshing what a previous run left.
///
/// The removals come first and are reported first: a renamed skill should read
/// as one line gone and one line arrived, in that order, rather than as an
/// unexplained deletion after the writes.
pub fn install(root: &Path) -> Result<Vec<(Outcome, PathBuf)>, String> {
    let skills = root.join(SKILLS_DIRECTORY);
    let mut done = Vec::new();
    for stale in retired(&skills) {
        std::fs::remove_dir_all(&stale)
            .map_err(|e| format!("cannot remove {}: {e}", stale.display()))?;
        done.push((Outcome::Removed, stale));
    }
    for skill in SKILLS {
        let directory = skills.join(skill.name);
        let path = directory.join("SKILL.md");
        let existed = path.exists();
        std::fs::create_dir_all(&directory)
            .map_err(|e| format!("cannot create {}: {e}", directory.display()))?;
        std::fs::write(&path, skill.text)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        done.push((if existed { Outcome::Overwrote } else { Outcome::Wrote }, path));
    }
    Ok(done)
}

/// The `buri-` directories under `skills` that this toolchain no longer ships.
///
/// Sorted, so that two runs on one tree print the same lines in the same order
/// whatever the filesystem hands back.
fn retired(skills: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(skills) else {
        return Vec::new();
    };
    let mut stale: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with(OFFICIAL) && !SKILLS.iter().any(|s| s.name == name)
        })
        .map(|entry| entry.path())
        .collect();
    stale.sort();
    stale
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
    use std::collections::HashSet;

    /// A scratch directory nobody else can see, emptied first so a previous
    /// run's leftovers cannot make a test pass.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("buri-skills-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn read(root: &Path, name: &str) -> String {
        std::fs::read_to_string(root.join(SKILLS_DIRECTORY).join(name).join("SKILL.md")).unwrap()
    }

    /// The `name` and `description` a skill's own frontmatter declares.
    fn frontmatter(text: &str) -> (String, String) {
        let rest = text.strip_prefix("---\n").expect("a skill opens with `---`");
        let (block, _) = rest.split_once("\n---\n").expect("the frontmatter is closed by `---`");
        let field = |key: &str| {
            block
                .lines()
                .find_map(|line| line.strip_prefix(key))
                .map(|value| value.trim().to_string())
                .unwrap_or_default()
        };
        (field("name:"), field("description:"))
    }

    #[test]
    fn a_run_writes_every_skill() {
        let root = scratch("fresh");
        let done = install(&root).unwrap();
        assert_eq!(done.len(), SKILLS.len(), "one line per skill and nothing else");
        for skill in SKILLS {
            assert!(done.iter().any(|(o, p)| *o == Outcome::Wrote && p.ends_with("SKILL.md")
                && p.parent().is_some_and(|d| d.ends_with(skill.name))));
            assert_eq!(read(&root, skill.name), skill.text);
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The upgrade path: a `buri-` skill somebody edited is replaced by the
    /// one this binary ships, and says so.
    #[test]
    fn a_rerun_overwrites_a_modified_skill() {
        let root = scratch("rerun");
        install(&root).unwrap();
        let edited = root.join(SKILLS_DIRECTORY).join("buri-cli").join("SKILL.md");
        std::fs::write(&edited, "stale\n").unwrap();

        let done = install(&root).unwrap();
        assert!(
            done.iter().all(|(outcome, _)| *outcome == Outcome::Overwrote),
            "a second run overwrites every skill the first one wrote"
        );
        assert_eq!(std::fs::read_to_string(&edited).unwrap(), SKILLS[4].text);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The whole of what the `buri-` prefix is for: somebody else's skill is
    /// not ours to rewrite, and a `buri-` one we no longer ship is.
    #[test]
    fn only_our_own_skills_are_touched() {
        let root = scratch("neighbours");
        let skills = root.join(SKILLS_DIRECTORY);
        std::fs::create_dir_all(skills.join("deploy-checklist")).unwrap();
        std::fs::write(skills.join("deploy-checklist/SKILL.md"), "mine\n").unwrap();
        std::fs::create_dir_all(skills.join("buri-retired")).unwrap();
        std::fs::write(skills.join("buri-retired/SKILL.md"), "an older release\n").unwrap();

        let done = install(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(skills.join("deploy-checklist/SKILL.md")).unwrap(),
            "mine\n",
            "a skill that is not ours is left exactly as it was"
        );
        assert!(!skills.join("buri-retired").exists(), "a `buri-` skill we no longer ship goes");
        assert_eq!(done.first().map(|(outcome, _)| *outcome), Some(Outcome::Removed));
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A skill is read in one sitting by an agent that has other work to do.
    /// Three hundred lines is the ceiling, and it is checked here rather than
    /// left to whoever edits the markdown.
    #[test]
    fn every_skill_fits_in_a_page() {
        for skill in SKILLS {
            let lines = skill.text.lines().count();
            assert!(lines <= 300, "{} is {lines} lines; the ceiling is 300", skill.name);
        }
    }

    /// The table above and the markdown must agree about what a skill is
    /// called, since the directory comes from one and the loader reads the
    /// other.
    #[test]
    fn every_skill_agrees_with_its_own_frontmatter() {
        let mut seen = HashSet::new();
        for skill in SKILLS {
            assert!(seen.insert(skill.name), "`{}` is registered twice", skill.name);
            assert!(skill.name.starts_with(OFFICIAL), "`{}` is not marked ours", skill.name);
            let (name, description) = frontmatter(skill.text);
            assert_eq!(name, skill.name, "`{}` declares a different name", skill.name);
            assert!(!description.is_empty(), "`{}` has no description", skill.name);
            assert!(
                !description.contains('\n'),
                "`{}`'s description is more than one line",
                skill.name
            );
        }
    }
}
