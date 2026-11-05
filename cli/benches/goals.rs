//! The throughput goals, measured on what a user's build does
//! (`design/PERFORMANCE.md` §6.82).
//!
//! ```text
//! cargo bench -p buri --bench compiler -- --goals=time              # lines a CPU second, here
//! cargo bench -p buri --bench compiler -- --goals=count --size=ci   # callgrind, against the baseline
//! cargo bench -p buri --bench compiler -- --goals=count --size=ci --bless
//! ```
//!
//! Both run a cold dev `buri build //...` of the workspace `workspace.rs`
//! writes, through the real CLI, on one thread. `time` reads one profiled
//! build's CPU time per phase, and its instructions where the machine counts
//! them. `count` is Linux only: it runs the build under callgrind's cache
//! simulation once per phase group and once whole, and holds each goal
//! phase's instructions and cache cost per line to `goals/baseline.txt`.

mod report;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use report::{Baseline, Events, Measured, Profile};

use crate::workspace::{self, Size};
use crate::{fail, TARGET_LOWER, TARGET_PARSE, TARGET_SEMA};

const GOALS: [f64; 3] = [TARGET_PARSE, TARGET_SEMA, TARGET_LOWER];

/// Profiled builds `time` takes the fastest of, per phase.
const RUNS: usize = 3;

pub fn run(mode: &str, size: Option<&str>, bless: bool) {
    let default = if mode == "count" { "ci" } else { "full" };
    let name = size.unwrap_or(default);
    let size = workspace::SIZES
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| fail(&format!("no workspace size `{name}`: full, ci or tiny")));
    match mode {
        "time" => timed(size),
        "count" => counted(size, bless),
        _ => fail("--goals wants `time` or `count`"),
    }
}

/// Where workspaces are written: `BURI_GOALS_DIR`, or `target/goals`.
fn root() -> PathBuf {
    std::env::var_os("BURI_GOALS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("target/goals")
        })
}

/// The workspace at `size` in `dir`, written again only when its bytes moved,
/// with a home of its own that one unmeasured build has warmed. The warm-up
/// runs every time: a home a different `buri` warmed holds nothing this one
/// reuses, and the first measured build would pay for it. Returns the home.
fn prepared(size: &Size, dir: &Path) -> PathBuf {
    let files = workspace::files(size);
    let digest = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for f in &files {
            f.path.hash(&mut h);
            f.text.hash(&mut h);
        }
        format!("{:016x}\n", h.finish())
    };
    let stamp = dir.join(".digest");
    let home = dir.join(".home");
    if std::fs::read_to_string(&stamp).ok().as_deref() != Some(digest.as_str()) {
        let _ = std::fs::remove_dir_all(dir);
        for f in &files {
            let path = dir.join(&f.path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &f.text).unwrap();
        }
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(&stamp, digest).unwrap();
    }
    build(dir, &home, &[]);
    clean(dir, &home);
    home
}

fn buri(dir: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
    cmd.current_dir(dir)
        .env("HOME", home)
        .env("BURI_THREADS", "1")
        .env_remove("BURI_PROFILE")
        .env_remove("BURI_PROFILE_COLLECT");
    cmd
}

fn clean(dir: &Path, home: &Path) {
    let ok = buri(dir, home)
        .arg("clean")
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        fail(&format!("`buri clean` failed in {}", dir.display()));
    }
}

/// One cold profiled build; its stderr.
fn build(dir: &Path, home: &Path, env: &[(&str, &str)]) -> String {
    clean(dir, home);
    let out = buri(dir, home)
        .args(["build", "//..."])
        .env("BURI_PROFILE", "1")
        .envs(env.iter().copied())
        .output()
        .unwrap_or_else(|e| fail(&e.to_string()));
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() {
        fail(&format!(
            "`buri build //...` failed in {}:\n{stderr}",
            dir.display()
        ));
    }
    stderr
}

fn lines_of(profile: &Profile) -> u64 {
    match profile.lines {
        Some(n) if n > 0 => n,
        _ => fail("the build's profile counted no lines of code"),
    }
}

fn timed(size: &Size) {
    let dir = root().join(size.name);
    let home = prepared(size, &dir);
    let mut lines = 0;
    let mut best: Option<[(f64, Option<f64>); 4]> = None;
    for _ in 0..RUNS {
        let profile = Profile::read(&build(&dir, &home, &[]));
        lines = lines_of(&profile);
        let now = profile.phases();
        best = Some(match best {
            None => now,
            Some(b) => std::array::from_fn(|i| {
                let min = |x: Option<f64>, y: Option<f64>| match (x, y) {
                    (Some(x), Some(y)) => Some(x.min(y)),
                    _ => x.or(y),
                };
                (b[i].0.min(now[i].0), min(b[i].1, now[i].1))
            }),
        });
    }
    clean(&dir, &home);
    let figures = best.unwrap().map(|(cpu, n)| (Some(cpu), n));
    print!(
        "{}",
        Measured::new(size.name, lines, GOALS, figures, None).render()
    );
}

/// The tools a count depends on besides this repository's code.
fn toolchain() -> String {
    let first_line = |program: &str, args: &[&str]| -> String {
        Command::new(program)
            .args(args)
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.lines().next().map(str::to_string))
            .unwrap_or_else(|| format!("{program} ?"))
    };
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    [
        first_line("rustc", &["-V"]),
        first_line(&cc, &["--version"]),
        first_line("valgrind", &["--version"]),
    ]
    .join("; ")
}

/// The four callgrind runs: one per phase group, then the whole build and its
/// children. Each is a workspace and home of its own, so they run side by side.
const GROUPS: [&str; 4] = ["parse", "check", "other", "whole"];

fn counted(size: &Size, bless: bool) {
    if !cfg!(target_os = "linux") {
        fail("--goals=count runs on Linux only: callgrind counts there (design/PERFORMANCE.md §6.82)");
    }
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let base = root().join(format!("{}-count", size.name));
    let results: Vec<(Events, String)> = std::thread::scope(|scope| {
        let handles: Vec<_> = GROUPS
            .iter()
            .enumerate()
            .map(|(i, group)| {
                let dir = base.join(group);
                scope.spawn(move || {
                    let home = prepared(size, &dir);
                    let out = dir.join(".callgrind");
                    let _ = std::fs::remove_dir_all(&out);
                    std::fs::create_dir_all(&out).unwrap();
                    clean(&dir, &home);
                    let mut cmd = Command::new("taskset");
                    cmd.args([
                        "-c",
                        &(i % cores).to_string(),
                        "valgrind",
                        "--tool=callgrind",
                        "--cache-sim=yes",
                    ])
                    .args(report::CACHES)
                    .arg(format!("--callgrind-out-file={}/out.%p", out.display()))
                    .arg(format!("--log-file={}/log.%p", out.display()));
                    if *group == "whole" {
                        cmd.arg("--trace-children=yes");
                    } else {
                        cmd.arg("--collect-atstart=no")
                            .env("BURI_PROFILE_COLLECT", group);
                    }
                    let run = cmd
                        .arg(env!("CARGO_BIN_EXE_buri"))
                        .args(["build", "//..."])
                        .current_dir(&dir)
                        .env("HOME", &home)
                        .env("BURI_THREADS", "1")
                        .env("BURI_PROFILE", "1")
                        .output()
                        .unwrap_or_else(|e| fail(&format!("valgrind: {e}")));
                    let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
                    if !run.status.success() {
                        fail(&format!(
                            "`buri build //...` under callgrind failed in {}:\n{stderr}",
                            dir.display()
                        ));
                    }
                    let mut events = Events::default();
                    for entry in std::fs::read_dir(&out).unwrap() {
                        let path = entry.unwrap().path();
                        if path
                            .file_name()
                            .is_some_and(|n| n.to_string_lossy().starts_with("out."))
                        {
                            let text = std::fs::read_to_string(&path).unwrap();
                            events = events.plus(
                                Events::read(&text)
                                    .unwrap_or_else(|e| fail(&format!("{}: {e}", path.display()))),
                            );
                        }
                    }
                    clean(&dir, &home);
                    (events, stderr)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("a callgrind run"))
            .collect()
    });
    let lines = lines_of(&Profile::read(&results[3].1));
    let [parse, check, other, whole] = [0, 1, 2, 3].map(|i| results[i].0);
    let codegen = whole.minus(parse).minus(check).minus(other);
    let events = [parse, check, other, codegen];
    let figures = events.map(|e| (None, Some(e.instructions)));
    let measured = Measured::new(
        size.name,
        lines,
        GOALS,
        figures,
        Some(events.map(|e| e.cost())),
    );
    print!("{}", measured.render());

    let arch = std::env::consts::ARCH;
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/goals/baseline.txt");
    let baseline = Baseline::read(&std::fs::read_to_string(&path).unwrap_or_default());
    let verdict = baseline.check(arch, &toolchain(), &measured.figures(arch));
    let written = base.join("baseline.txt");
    std::fs::write(&written, verdict.next.render()).unwrap();
    if bless {
        std::fs::write(&path, verdict.next.render()).unwrap();
        println!("\nwrote {}", path.display());
        return;
    }
    println!("\n{}", verdict.report.trim_end());
    if verdict.failed {
        println!(
            "\nIf the change is deliberate and lowers a figure, re-bless with `--bless`, or commit\n\
             {} from this run as cli/benches/goals/baseline.txt (design/PERFORMANCE.md §6.82).",
            written.display()
        );
        std::process::exit(1);
    }
}
