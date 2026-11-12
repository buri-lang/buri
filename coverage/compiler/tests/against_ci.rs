//! `--against-ci` on a measured shard: the other platforms' shards come from a
//! `gh` that hands out the newest run's whose coverage passed, and the diff
//! from that run's commit moves their lines. Run 8 is the newest, and its
//! coverage failed unless a test says it passed; run 7's passed.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test code: a helper that unwraps fails on the line that broke"
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const X86: &str = "x86_64-unknown-linux-gnu";
const MAC: &str = "aarch64-apple-darwin";
const LINK: &str = "cli/src/build/link.rs";

/// A checkout whose one commit CI's runs measured.
struct Repo {
    root: PathBuf,
}

impl Repo {
    fn new(name: &str) -> Repo {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("against-ci-{}", std::process::id()))
            .join(name);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("cli/src/build")).unwrap();
        std::fs::create_dir_all(root.join("coverage")).unwrap();
        std::fs::write(root.join("Cargo.lock"), "").unwrap();
        let source: String = (1..=10).map(|n| format!("line {n}\n")).collect();
        std::fs::write(root.join(LINK), source).unwrap();
        // Line 3's `if` took its true side on Linux and its false side on
        // macOS; line 5's took both on Linux alone.
        std::fs::write(
            root.join("coverage/compiler-branches.txt"),
            format!("hosts {MAC} {X86}\ntotal 4 4\narea cli/build 4 4\n4 4 {LINK}\n"),
        )
        .unwrap();
        let repo = Repo { root };
        repo.git(&["init", "-q"]);
        repo.git(&["add", "."]);
        repo.git(&["commit", "-qm", "green"]);
        let commit = String::from_utf8(repo.git(&["rev-parse", "HEAD"]).stdout).unwrap();
        repo.shard("green", X86, &[(3, 1, 0), (5, 1, 1)], 1);
        repo.shard("green", MAC, &[(3, 0, 1), (5, 0, 0)], 1);
        let gh = repo.root.join("bin/gh");
        std::fs::create_dir_all(gh.parent().unwrap()).unwrap();
        let green = repo.root.join("green");
        std::fs::write(
            &gh,
            format!(
                "#!/bin/sh\ncase \"$2\" in\n  list) echo \"8 {c}\"; echo \"7 {c}\" ;;\n  view) if [ \"$3\" = 7 ] || [ -e passed-$3 ]; then echo success; else echo failure; fi ;;\n  download) cp -R {}/. \"$7\" ;;\nesac\n",
                green.display(),
                c = commit.trim(),
            ),
        )
        .unwrap();
        let mut mode = std::fs::metadata(&gh).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
        std::fs::set_permissions(&gh, mode).unwrap();
        repo
    }

    fn git(&self, args: &[&str]) -> Output {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(&self.root)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        out
    }

    /// `host`'s one shard in `dir`, of `link.rs`'s `if`s: `(line, true side
    /// taken, false side taken)`, in a function that starts on `starts`.
    fn shard(&self, dir: &str, host: &str, ifs: &[(u32, u64, u64)], starts: u32) {
        let dir = self.root.join(dir).join(format!("coverage-shard-{host}-1"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shard"), format!("{host} 1/1\n")).unwrap();
        let mut coverage = format!("file 0 {LINK}\n");
        for &(line, yes, no) in ifs {
            for (side, taken) in [(0, yes), (1, no)] {
                coverage.push_str(&format!(
                    "branch {starts} 1 {line} 4 {line} 8 0 {side} {taken}\n"
                ));
            }
        }
        std::fs::write(dir.join("coverage.txt"), coverage).unwrap();
    }

    /// Two lines written above the rest, as the change under test.
    fn insert_two_lines(&self) {
        let path = self.root.join(LINK);
        let source = std::fs::read_to_string(&path).unwrap();
        std::fs::write(path, format!("new 1\nnew 2\n{source}")).unwrap();
    }

    /// `--against-ci` with this host's shard, `ifs` as `shard` takes them.
    fn against_ci(&self, ifs: &[(u32, u64, u64)]) -> Output {
        let _ = std::fs::remove_dir_all(self.root.join("here"));
        self.shard("here", MAC, ifs, 3);
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap()
        );
        Command::new(env!("CARGO_BIN_EXE_buri-compiler-coverage"))
            .arg(format!(
                "--against-ci={}",
                self.root
                    .join("here/coverage-shard-aarch64-apple-darwin-1")
                    .display()
            ))
            .env("PATH", path)
            .current_dir(&self.root)
            .output()
            .unwrap()
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn the_other_platforms_counts_move_with_the_lines_and_still_count() {
    let repo = Repo::new("moved");
    repo.insert_two_lines();
    // Lines 3 and 5 are 5 and 7 now.
    let out = repo.against_ci(&[(5, 0, 1), (7, 0, 0)]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("against CI run 7, of "),
        "{}",
        text(&out)
    );
    assert!(
        text(&out).contains("branch coverage: 100.00% (4/4) in the baseline, 100.00% (4/4) now"),
        "{}",
        text(&out)
    );
}

#[test]
fn a_side_this_host_stops_taking_fails_and_is_named() {
    let repo = Repo::new("stopped");
    repo.insert_two_lines();
    let out = repo.against_ci(&[(5, 0, 0), (7, 0, 0)]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        text(&out).contains("No test takes these 1 branch side(s), which CI run 7 took."),
        "{}",
        text(&out)
    );
    assert!(
        text(&out).contains("can do that too):\n  cli/src/build/link.rs:5:4, the false side\n"),
        "{}",
        text(&out)
    );
}

#[test]
fn the_newest_run_whose_coverage_passed_is_the_one_taken() {
    let repo = Repo::new("newest");
    let out = repo.against_ci(&[(3, 0, 1), (5, 0, 0)]);
    assert!(
        text(&out).contains("against CI run 7, of "),
        "{}",
        text(&out)
    );
    // A run another job failed in still has its coverage.
    std::fs::write(repo.root.join("passed-8"), "").unwrap();
    let out = repo.against_ci(&[(3, 0, 1), (5, 0, 0)]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("against CI run 8, of "),
        "{}",
        text(&out)
    );
}
