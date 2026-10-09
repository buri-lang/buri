//! The goals gate: the three throughput goals, checked two ways
//! (`design/PERFORMANCE.md` §6.82).
//!
//! ```text
//! cargo bench -p buri --bench compiler -- --goals=count   # instructions a line, against a budget
//! cargo bench -p buri --bench compiler -- --goals=wall    # lines a second, against the goal
//! ```
//!
//! `count` reads the kernel's instruction counter on macOS and cachegrind's on
//! Linux. Counts don't move with the machine's load, so it never flakes. `wall`
//! times this machine and fails only below a goal itself.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use buri::diagnostics::{Diagnostics, FileId, SourceMap};
use buri::parsing::parser;

use crate::generate::Program;
use crate::{bench, check, corpus, fail, load, Config, TARGET_LOWER, TARGET_PARSE, TARGET_SEMA};

/// One goal, and the corpora and budget it's checked with.
struct Goal {
    phase: &'static str,
    /// Lines a second.
    goal: f64,
    /// Instructions a second one core of the reference machine, an M3 Pro,
    /// retires in this phase's code. A phase over `rate / goal` instructions a
    /// line can't meet its goal there.
    rate: f64,
    /// Where it's counted and where it's timed. Dev compile counts 100k
    /// because cachegrind runs a 1M build for minutes.
    counted_on: &'static str,
    timed_on: &'static str,
    /// Whether `wall` fails below the goal. Only where this machine has at
    /// least 3x headroom, so noise can't fail it.
    blocking: bool,
}

const GOALS: [Goal; 3] = [
    Goal {
        phase: "parse",
        goal: TARGET_PARSE,
        rate: 18e9,
        counted_on: "mixed-1M",
        timed_on: "mixed-1M",
        blocking: false,
    },
    Goal {
        phase: "check",
        goal: TARGET_SEMA,
        rate: 12e9,
        counted_on: "mixed-1M",
        timed_on: "mixed-1M",
        blocking: true,
    },
    Goal {
        phase: "dev compile",
        goal: TARGET_LOWER,
        rate: 8.5e9,
        counted_on: "mixed-100k",
        timed_on: "mixed-1M",
        blocking: true,
    },
];

impl Goal {
    /// Instructions a line.
    fn budget(&self) -> f64 {
        self.rate / self.goal
    }
}

pub fn run(mode: &str) {
    let failures = match mode {
        "count" => counted(),
        "wall" => timed(),
        _ => fail("--goals wants `count` or `wall`"),
    };
    if !failures.is_empty() {
        eprintln!();
        for f in &failures {
            eprintln!("error: {f}");
        }
        eprintln!("design/PERFORMANCE.md §6.82 says what each number means and how a budget was set.");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// The corpora, written out once
// ---------------------------------------------------------------------------

/// Where the corpora are written: `BURI_GOALS_DIR`, or `target/goals`.
fn root() -> PathBuf {
    std::env::var_os("BURI_GOALS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/goals"))
}

/// A pinned corpus as a repository on disk, and its program.
///
/// A copy left by an earlier run is kept when its bytes still match the pinned
/// digest, so a cached directory can never be stale.
fn repository(name: &str) -> (PathBuf, Program) {
    let manifest_path = corpus::pinned_root().join(format!("{name}.txt"));
    let manifest = corpus::pinned_manifest(&manifest_path).unwrap_or_else(|e| fail(&e));
    let dir = root().join(name);
    if let Ok(program) = corpus::load_repository(&dir) {
        if corpus::digest(&program) == manifest.digest {
            return (dir, program);
        }
    }
    let (_, program) = corpus::load_pinned(&manifest_path).unwrap_or_else(|e| fail(&e));
    corpus::write_repository(&dir, &program).unwrap_or_else(|e| fail(&e));
    (dir, program)
}

/// `7.78M lines/s`.
fn lines_per_second(rate: f64) -> String {
    format!("{} lines/s", crate::rate(rate).trim().trim_end_matches("/s").trim())
}

fn buri(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
    cmd.args(args).current_dir(dir).env_remove("BURI_PROFILE");
    cmd
}

fn clean(dir: &Path) {
    let ok = buri(dir, &["clean"]).stdout(Stdio::null()).status().is_ok_and(|s| s.success());
    if !ok {
        fail(&format!("`buri clean` failed in {}", dir.display()));
    }
}

// ---------------------------------------------------------------------------
// count: instructions a line
// ---------------------------------------------------------------------------

/// On macOS a child counts its own thread around one phase and prints the
/// count, which leaves out reading the corpus and the kernel's work for it.
/// Elsewhere cachegrind counts the whole child, and `read` and `load` are the
/// floors the parent subtracts.
const COUNTS_ITSELF: bool = cfg!(target_os = "macos");

/// One phase, once, in a process of its own.
pub fn child(phase: &str, dir: &str) {
    let program = corpus::load_repository(Path::new(dir)).unwrap_or_else(|e| fail(&e));
    let mut map = SourceMap::new();
    let mut cache = parser::Cache::new();
    let mut diagnostics = Diagnostics::new();
    let loaded = matches!(phase, "load" | "check").then(|| load(&program, &mut map, &mut cache, &mut diagnostics));
    let before = buri::profile::thread_instructions();
    match (phase, &loaded) {
        ("read" | "load", _) => {}
        ("parse", _) => {
            for (i, m) in program.modules.iter().enumerate() {
                std::hint::black_box(parser::parse(&m.text, FileId(i as u32)));
            }
        }
        ("check", Some(loaded)) => {
            std::hint::black_box(check(loaded, &mut Diagnostics::new()));
        }
        _ => fail(&format!("no goals phase `{phase}`")),
    }
    println!("{}", buri::profile::thread_instructions().saturating_sub(before));
}

/// What a child that counts itself printed.
fn own_count(cmd: &mut Command) -> Result<u64, String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    match String::from_utf8_lossy(&out.stdout).trim().parse::<u64>() {
        Ok(n) if n > 0 => Ok(n),
        _ => Err("this machine counts no instructions, as in a virtual machine; \
                  count on a Mac, or on Linux with valgrind"
            .to_string()),
    }
}

/// Instructions `cmd` retires: macOS's own counter, or cachegrind's on Linux,
/// there pinned to `core` so the toolchain starts no workers.
fn instructions(cmd: &Command, core: usize) -> Result<u64, String> {
    let program = cmd.get_program().to_owned();
    let args: Vec<_> = cmd.get_args().map(|a| a.to_owned()).collect();
    let mut counted = if cfg!(target_os = "macos") {
        let mut c = Command::new("/usr/bin/time");
        c.arg("-l");
        c
    } else {
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        let mut c = Command::new("taskset");
        c.args(["-c", &(core % cores).to_string(), "valgrind", "--tool=cachegrind", "--cache-sim=no"]);
        c.arg("--cachegrind-out-file=/dev/null");
        c
    };
    counted.arg(&program).args(&args).stdout(Stdio::null());
    if let Some(dir) = cmd.get_current_dir() {
        counted.current_dir(dir);
    }
    for (k, v) in cmd.get_envs() {
        match v {
            Some(v) => counted.env(k, v),
            None => counted.env_remove(k),
        };
    }
    let out = counted.output().map_err(|e| format!("{}: {e}", counted.get_program().to_string_lossy()))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        return Err(format!("{} failed:\n{stderr}", program.to_string_lossy()));
    }
    let count = stderr.lines().find_map(|line| {
        let line = line.trim();
        // BSD `time -l`: "  123456  instructions retired".
        // cachegrind: "==123== I refs:      1,234,567" (`I   refs:` before 3.21).
        let digits = line
            .strip_suffix("instructions retired")
            .or_else(|| line.split_once("I refs:").or_else(|| line.split_once("I   refs:")).map(|(_, n)| n))?;
        digits.trim().replace(',', "").parse::<u64>().ok()
    });
    match count {
        Some(n) if n > 0 => Ok(n),
        _ => Err(format!(
            "{} counted no instructions. macOS's counter reads nothing in a virtual machine; \
             count on a Mac, or on Linux with valgrind",
            counted.get_program().to_string_lossy()
        )),
    }
}

fn counted() -> Vec<String> {
    let exe = std::env::current_exe().unwrap_or_else(|e| fail(&e.to_string()));
    // A label, the command, and the repository to clean before each run.
    let mut jobs: Vec<(String, Command, Option<PathBuf>)> = Vec::new();
    let mut lines = std::collections::BTreeMap::new();
    for name in ["mixed-1M", "mixed-100k"] {
        let (dir, program) = repository(name);
        lines.insert(name, program.lines());
        if GOALS.iter().any(|g| g.counted_on == name && g.phase == "dev compile") {
            jobs.push((format!("{name}/build"), buri(&dir, &["build", "//bench"]), Some(dir.clone())));
        }
        if GOALS.iter().any(|g| g.counted_on == name && g.phase != "dev compile") {
            let phases: &[&str] =
                if COUNTS_ITSELF { &["parse", "check"] } else { &["read", "parse", "load", "check"] };
            for phase in phases {
                let mut cmd = Command::new(&exe);
                cmd.arg(format!("--goals-child={phase}")).arg(format!("--from={}", dir.display()));
                jobs.push((format!("{name}/{phase}"), cmd, None));
            }
        }
    }

    // Side by side, each on its own core under cachegrind. macOS's counter also
    // counts the kernel's page faults, which move with memory pressure, so
    // there the fewest of three runs counts (design/PERFORMANCE.md §8).
    let runs = if cfg!(target_os = "macos") { 3 } else { 1 };
    let results: Vec<(String, Result<u64, String>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .iter_mut()
            .enumerate()
            .map(|(i, (label, cmd, dirty))| {
                scope.spawn(move || {
                    let fewest = (0..runs).try_fold(u64::MAX, |fewest, _| {
                        if let Some(dir) = dirty {
                            clean(dir);
                        }
                        let n = if dirty.is_none() && COUNTS_ITSELF { own_count(cmd) } else { instructions(cmd, i) };
                        n.map(|n| fewest.min(n))
                    });
                    (label.clone(), fewest)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("a counting thread")).collect()
    });
    // A built program is hundreds of megabytes that a cached corpus shouldn't carry.
    for (_, _, dirty) in &jobs {
        if let Some(dir) = dirty {
            clean(dir);
        }
    }
    let mut count = std::collections::BTreeMap::new();
    for (label, result) in results {
        match result {
            Ok(n) => {
                count.insert(label, n);
            }
            Err(e) => fail(&format!("counting {label}: {e}")),
        }
    }
    let of = |label: String| count.get(&label).copied().unwrap_or(0);

    println!("goals, counted: instructions a line against each budget (design/PERFORMANCE.md §6.82)");
    println!();
    println!("  {:<12} {:<12} {:>12} {:>9} {:>9}", "phase", "corpus", "per line", "budget", "headroom");
    let mut failures = Vec::new();
    for g in &GOALS {
        let name = g.counted_on;
        let n = match g.phase {
            "parse" => of(format!("{name}/parse")).saturating_sub(of(format!("{name}/read"))),
            "check" => of(format!("{name}/check")).saturating_sub(of(format!("{name}/load"))),
            _ => of(format!("{name}/build")),
        };
        let per_line = n as f64 / lines[name] as f64;
        let headroom = g.budget() / per_line;
        println!(
            "  {:<12} {:<12} {:>12} {:>9} {:>8.2}x",
            g.phase,
            name,
            crate::commas(per_line),
            crate::commas(g.budget()),
            headroom
        );
        if headroom < 1.0 {
            failures.push(format!(
                "{} retires {} instructions a line on pinned:{name}, over its budget of {}. At the \
                 reference core's {:.0} G instructions a second that's {}, under the goal of {}.",
                g.phase,
                crate::commas(per_line),
                crate::commas(g.budget()),
                g.rate / 1e9,
                lines_per_second(g.rate / per_line),
                lines_per_second(g.goal),
            ));
        }
    }
    failures
}

// ---------------------------------------------------------------------------
// wall: lines a second
// ---------------------------------------------------------------------------

/// Cold `buri build`s for dev compile; the fastest counts.
const BUILDS: usize = 3;

fn timed() -> Vec<String> {
    let cfg = Config {
        min_reps: 5,
        min_time: Duration::from_secs(1),
        warmup: Duration::from_millis(300),
        warm_reps: 1,
        reduced: true,
    };
    println!("goals, timed: the fastest of several runs against each goal (design/PERFORMANCE.md §6.82)");
    println!();
    println!("  {:<12} {:<12} {:>10} {:>10} {:>9}  on a miss", "phase", "corpus", "lines/s", "goal", "headroom");
    let mut failures = Vec::new();
    for g in &GOALS {
        let (dir, program) = repository(g.timed_on);
        let fastest = match g.phase {
            "parse" => {
                let (_, _, fastest, _) = bench(&cfg, || {
                    for (i, m) in program.modules.iter().enumerate() {
                        std::hint::black_box(parser::parse(&m.text, FileId(i as u32)));
                    }
                });
                fastest
            }
            "check" => {
                let mut map = SourceMap::new();
                let mut cache = parser::Cache::new();
                let mut diagnostics = Diagnostics::new();
                let loaded = load(&program, &mut map, &mut cache, &mut diagnostics);
                let (_, _, fastest, _) = bench(&cfg, || {
                    std::hint::black_box(check(&loaded, &mut Diagnostics::new()));
                });
                fastest
            }
            _ => {
                let fastest = (0..BUILDS).map(|_| cold_build(&dir)).min().unwrap_or_default();
                clean(&dir);
                fastest
            }
        };
        let rate = program.lines() as f64 / fastest.as_secs_f64();
        let headroom = rate / g.goal;
        println!(
            "  {:<12} {:<12} {:>10} {:>10} {:>8.2}x  {}",
            g.phase,
            g.timed_on,
            crate::rate(rate),
            crate::rate(g.goal),
            headroom,
            if g.blocking { "fails" } else { "warns" }
        );
        if headroom < 1.0 {
            let what = format!(
                "{} ran at {} on pinned:{}, the fastest of its runs, under the goal of {}.",
                g.phase,
                lines_per_second(rate),
                g.timed_on,
                lines_per_second(g.goal)
            );
            if g.blocking {
                failures.push(what);
            } else {
                println!("warning: {what}");
            }
        }
    }
    failures
}

/// The wall time of one `buri build` from an empty cache: what a debug build
/// and a cold `buri test` both do, up to a linked binary.
fn cold_build(dir: &Path) -> Duration {
    clean(dir);
    let started = Instant::now();
    let out = buri(dir, &["build", "//bench"]).output().unwrap_or_else(|e| fail(&e.to_string()));
    let took = started.elapsed();
    if !out.status.success() {
        fail(&format!("`buri build //bench` failed in {}:\n{}", dir.display(), String::from_utf8_lossy(&out.stderr)));
    }
    took
}
