//! `--merge` and `--raise` as CI runs them: shards' lcov from three platforms
//! in, a verdict, a baseline and each platform's lcov out.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const X86: &str = "x86_64-unknown-linux-gnu";
const ARM: &str = "aarch64-unknown-linux-gnu";
const MAC: &str = "aarch64-apple-darwin";
const LEXER: &str = "crates/syntax/src/lexer.rs";
const LINK: &str = "cli/src/build/link.rs";

/// A workspace with a `coverage/` directory and the shards CI downloaded.
struct Repo {
    root: PathBuf,
}

impl Repo {
    fn new(name: &str) -> Repo {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("merge").join(name);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("coverage")).unwrap();
        std::fs::create_dir_all(root.join("shards")).unwrap();
        std::fs::write(root.join("Cargo.lock"), "").unwrap();
        Repo { root }
    }

    fn baseline(&self, text: &str) {
        std::fs::write(self.root.join("coverage/compiler-branches.txt"), text).unwrap();
    }

    /// One shard's artifact, as `--shard` writes it.
    fn shard(&self, host: &str, which: &str, lcov: &str) {
        let dir = self.root.join("shards").join(format!("coverage-shard-{host}-{}", which.replace('/', "-")));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shard"), format!("{host} {which}\n")).unwrap();
        std::fs::write(dir.join("coverage.lcov"), lcov).unwrap();
    }

    /// Every platform's one shard.
    fn everywhere(&self, x86: &str, arm: &str, mac: &str) {
        self.shard(X86, "1/1", x86);
        self.shard(ARM, "1/1", arm);
        self.shard(MAC, "1/1", mac);
    }

    fn run(&self, arg: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_buri-compiler-coverage"))
            .arg(arg)
            .current_dir(&self.root)
            .output()
            .unwrap()
    }

    fn merge(&self) -> Output {
        self.run("--merge=shards")
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
    }
}

/// A file's lcov record: lines as `(line, hits)`, branches as `(line, block, branch, taken)`.
fn file(path: &str, lines: &[(u32, u64)], branches: &[(u32, u32, u32, u64)]) -> String {
    let mut out = format!("SF:{path}\n");
    for (line, hits) in lines {
        out.push_str(&format!("DA:{line},{hits}\n"));
    }
    for (line, block, branch, taken) in branches {
        out.push_str(&format!("BRDA:{line},{block},{branch},{taken}\n"));
    }
    out.push_str("end_of_record\n");
    out
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn passed(out: &Output) -> String {
    assert!(out.status.success(), "{}\n{}", stdout(out), stderr(out));
    stdout(out)
}

fn failed(out: &Output) -> String {
    assert_eq!(out.status.code(), Some(1), "{}\n{}", stdout(out), stderr(out));
    format!("{}{}", stdout(out), stderr(out))
}

const THREE: &str = "hosts aarch64-apple-darwin aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu\n";

#[test]
fn a_branch_counts_as_covered_if_any_platform_took_it() {
    let repo = Repo::new("any_platform");
    let one_side = |taken: (u64, u64)| file(LEXER, &[(3, 1)], &[(3, 0, 0, taken.0), (3, 0, 1, taken.1)]);
    repo.everywhere(&one_side((1, 0)), &one_side((1, 0)), &one_side((0, 1)));
    let out = passed(&repo.merge());
    assert!(out.contains("merged from aarch64-apple-darwin, aarch64-unknown-linux-gnu, x86_64-unknown-linux-gnu"), "{out}");

    let measured = repo.read("target/coverage/compiler-branches.txt");
    assert!(measured.starts_with(&format!("# ")), "{measured}");
    assert!(measured.contains(THREE), "{measured}");
    assert!(measured.contains("\ntotal 2 2 100.00%\n"), "{measured}");
    assert!(measured.contains("\narea crates/syntax 2 2 100.00%\n"), "{measured}");
    assert!(measured.contains("\n2 2 crates/syntax/src/lexer.rs\n"), "{measured}");
}

#[test]
fn a_branch_one_platform_compiles_counts_once() {
    let repo = Repo::new("one_platform");
    let shared = file(LINK, &[(1, 1)], &[(1, 0, 0, 1), (1, 0, 1, 1)]);
    // The Mach-O writer's `if`, which only macOS compiles, took one side.
    let mac = file(LINK, &[(1, 1), (9, 1)], &[(1, 0, 0, 1), (1, 0, 1, 1), (9, 0, 0, 1), (9, 0, 1, 0)]);
    repo.everywhere(&shared, &shared, &mac);
    passed(&repo.merge());
    let measured = repo.read("target/coverage/compiler-branches.txt");
    assert!(measured.contains("\narea cli/build 3 4 75.00%\n"), "{measured}");
}

#[test]
fn a_line_whose_branches_differ_by_platform_counts_each_platforms_apart() {
    let repo = Repo::new("differ");
    let two = file(LEXER, &[(5, 1)], &[(5, 0, 0, 1), (5, 0, 1, 0)]);
    let four = file(LEXER, &[(5, 1)], &[(5, 0, 0, 0), (5, 0, 1, 1), (5, 1, 2, 0), (5, 1, 3, 0)]);
    repo.everywhere(&two, &two, &four);
    passed(&repo.merge());
    let measured = repo.read("target/coverage/compiler-branches.txt");
    assert!(measured.contains("\n2 6 crates/syntax/src/lexer.rs\n"), "{measured}");
}

#[test]
fn lines_count_as_covered_if_any_platform_ran_them() {
    let repo = Repo::new("lines");
    let ran = |hits: u64| file(LEXER, &[(1, 1), (2, hits)], &[]);
    repo.everywhere(&ran(0), &ran(0), &ran(4));
    let out = passed(&repo.merge());
    let row = out.lines().find(|l| l.starts_with("crates/syntax ")).unwrap_or_else(|| panic!("{out}"));
    assert!(row.contains("2/2"), "{row}");
}

#[test]
fn every_shard_of_every_platform_must_be_there() {
    let repo = Repo::new("missing_shard");
    let lcov = file(LEXER, &[(1, 1)], &[]);
    repo.shard(X86, "1/2", &lcov);
    repo.shard(ARM, "1/1", &lcov);
    repo.shard(MAC, "1/1", &lcov);
    let out = repo.merge();
    assert!(!out.status.success());
    assert!(stderr(&out).contains("x86_64-unknown-linux-gnu is missing shard [2] of 2"), "{}", stderr(&out));
}

#[test]
fn a_platform_the_baseline_has_must_be_there() {
    let repo = Repo::new("missing_platform");
    repo.baseline(&format!("{THREE}total 1 1 100.00%\n"));
    let lcov = file(LEXER, &[(1, 1)], &[(1, 0, 0, 1)]);
    repo.shard(X86, "1/1", &lcov);
    repo.shard(ARM, "1/1", &lcov);
    let out = failed(&repo.merge());
    assert!(out.contains("The baseline merges aarch64-apple-darwin, and this run has no shard from it."), "{out}");
}

#[test]
fn a_fall_in_an_area_fails_and_names_it() {
    let repo = Repo::new("fall");
    repo.baseline(&format!(
        "{THREE}total 2 2 100.00%\narea crates/syntax 2 2 100.00%\n2 2 crates/syntax/src/lexer.rs\n"
    ));
    let half = file(LEXER, &[(3, 1)], &[(3, 0, 0, 1), (3, 0, 1, 0)]);
    repo.everywhere(&half, &half, &half);
    let out = failed(&repo.merge());
    assert!(out.contains("Branch coverage fell:\n  the total: 100.00% (2/2) -> 50.00% (1/2)"), "{out}");
    assert!(out.contains("crates/syntax: 100.00% (2/2) -> 50.00% (1/2)"), "{out}");
}

#[test]
fn a_rise_names_what_rose_and_the_ratchet_commits_it() {
    let repo = Repo::new("rise");
    repo.baseline(&format!(
        "{THREE}total 1 2 50.00%\narea crates/syntax 1 2 50.00%\n1 2 crates/syntax/src/lexer.rs\n"
    ));
    let both = file(LEXER, &[(3, 1)], &[(3, 0, 0, 1), (3, 0, 1, 1)]);
    let none = file(LEXER, &[(3, 1)], &[(3, 0, 0, 0), (3, 0, 1, 0)]);
    repo.everywhere(&none, &both, &none);
    let out = passed(&repo.merge());
    assert!(out.contains("Branch coverage rose: the total, crates/syntax."), "{out}");

    let out = passed(&repo.run("--raise"));
    assert!(out.contains("raised the baseline of the total, crates/syntax"), "{out}");
    let checked_in = repo.read("coverage/compiler-branches.txt");
    assert!(checked_in.contains("\narea crates/syntax 2 2 100.00%\n"), "{checked_in}");
    assert_eq!(repo.read("coverage/compiler.txt"), repo.read("target/coverage/compiler.txt"));
}

/// The baseline before this gate merged platforms: x86_64 Linux's alone.
const OLD: &str = "host x86_64-unknown-linux-gnu\ntotal 1 2 50.00%\narea crates/syntax 1 2 50.00%\n\
                   1 2 crates/syntax/src/lexer.rs\n";

#[test]
fn a_baseline_of_fewer_platforms_is_held_to_those_platforms_alone() {
    let repo = Repo::new("fewer");
    repo.baseline(OLD);
    let x86 = |taken: u64| file(LEXER, &[(3, 1)], &[(3, 0, 0, 1), (3, 0, 1, taken)]);
    let none = file(LEXER, &[(3, 1)], &[(3, 0, 0, 0), (3, 0, 1, 0)]);

    // Only macOS took the false side: merged it's 2/2, but x86_64 alone is still 1/2.
    repo.everywhere(&x86(0), &none, &x86(1));
    let out = passed(&repo.merge());
    assert!(
        out.contains("The baseline is x86_64-unknown-linux-gnu's alone, so this run's x86_64-unknown-linux-gnu alone was compared with it."),
        "{out}"
    );
    assert!(out.contains("branch coverage: 50.00% (1/2) in the baseline, 50.00% (1/2) now"), "{out}");

    // On main, the ratchet's first run writes the merged baseline outright.
    let out = passed(&repo.run("--raise"));
    assert!(out.contains("raised the baseline to merge aarch64-apple-darwin, aarch64-unknown-linux-gnu, x86_64-unknown-linux-gnu"), "{out}");
    let checked_in = repo.read("coverage/compiler-branches.txt");
    assert!(checked_in.contains(THREE), "{checked_in}");
    assert!(checked_in.contains("\ntotal 2 2 100.00%\n"), "{checked_in}");
    assert!(!checked_in.contains("\nhost "), "{checked_in}");
}

#[test]
fn a_baseline_of_fewer_platforms_still_fails_a_fall_on_them() {
    let repo = Repo::new("fewer_fall");
    repo.baseline(OLD);
    let none = file(LEXER, &[(3, 1)], &[(3, 0, 0, 0), (3, 0, 1, 0)]);
    let both = file(LEXER, &[(3, 1)], &[(3, 0, 0, 1), (3, 0, 1, 1)]);
    repo.everywhere(&none, &both, &both);
    let out = failed(&repo.merge());
    assert!(out.contains("crates/syntax: 50.00% (1/2) -> 0.00% (0/2)"), "{out}");
}

#[test]
fn the_ratchet_never_takes_a_run_of_fewer_platforms() {
    let repo = Repo::new("ratchet_fewer");
    let lcov = file(LEXER, &[(3, 1)], &[(3, 0, 0, 1), (3, 0, 1, 1)]);
    repo.shard(X86, "1/1", &lcov);
    passed(&repo.merge());
    repo.baseline(&format!("{THREE}total 1 2 50.00%\narea crates/syntax 1 2 50.00%\n"));
    let out = passed(&repo.run("--raise"));
    assert!(out.contains("this run has no shard from aarch64-apple-darwin, aarch64-unknown-linux-gnu; the baseline stays"), "{out}");
    assert!(repo.read("coverage/compiler-branches.txt").contains("total 1 2 50.00%"));
}

#[test]
fn each_platforms_lcov_merges_its_shards() {
    let repo = Repo::new("lcov");
    repo.shard(X86, "1/2", &file(LEXER, &[(1, 2), (2, 0)], &[(2, 0, 0, 1), (2, 0, 1, 0)]));
    repo.shard(X86, "2/2", &file(LEXER, &[(1, 0), (2, 3)], &[(2, 0, 0, 0), (2, 0, 1, 5)]));
    let other = file(LEXER, &[(1, 0), (2, 0)], &[(2, 0, 0, 0), (2, 0, 1, 0)]);
    repo.shard(ARM, "1/1", &other);
    repo.shard(MAC, "1/1", &other);
    passed(&repo.merge());
    assert_eq!(
        repo.read(&format!("target/coverage/lcov/{X86}.lcov")),
        format!("SF:{LEXER}\nDA:1,2\nDA:2,3\nLF:2\nLH:2\nBRDA:2,0,0,1\nBRDA:2,0,1,5\nBRF:2\nBRH:2\nend_of_record\n")
    );
    assert_eq!(
        repo.read(&format!("target/coverage/lcov/{MAC}.lcov")),
        format!("SF:{LEXER}\nDA:1,0\nDA:2,0\nLF:2\nLH:0\nBRDA:2,0,0,0\nBRDA:2,0,1,0\nBRF:2\nBRH:0\nend_of_record\n")
    );
}

#[test]
fn a_file_in_no_area_fails() {
    let repo = Repo::new("unmapped");
    let lcov = file("crates/new/src/lib.rs", &[(1, 1)], &[]);
    repo.everywhere(&lcov, &lcov, &lcov);
    let out = failed(&repo.merge());
    assert!(out.contains("these files are in no coverage area"), "{out}");
    assert!(out.contains("crates/new/src/lib.rs"), "{out}");
}
