//! The instruction-count gate: fixed workloads, counted by cachegrind and held
//! to `instructions/baseline.txt` (design/PERFORMANCE.md §9).
//!
//! ```text
//! cargo bench -p buri --features backend-llvm --bench instructions               # check
//! cargo bench -p buri --features backend-llvm --bench instructions -- --bless    # rewrite the baseline
//! cargo bench -p buri --features backend-llvm --bench instructions -- --runs=3   # and show the spread
//! cargo bench -p buri --features backend-llvm --bench instructions -- run/       # only names containing `run/`
//! ```
//!
//! Linux only. Every measured process is pinned to one core, so the compiler
//! and the runtime start no workers and the count doesn't depend on scheduling.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "benchmark harness, like `compiler.rs`: its printing is its output, \
              and a harness that can't set up a workload should stop rather than \
              report a number."
)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// How much a count may grow before the gate fails.
const THRESHOLD: f64 = 0.02;

/// How far apart `--runs` may read before the gate calls a workload unsteady.
/// A twentieth of the threshold; measured spreads are under a thousandth of it.
const STEADY: f64 = 0.001;

/// Fixed rather than temporary, so every machine hands the compiler the same
/// paths. Two gates at once on one machine share it, so don't run two.
const ROOT: &str = "/tmp/buri-instructions";

/// The runtime workloads, each a package under `instructions/programs`.
const PROGRAMS: [&str; 7] = ["floats", "lists", "maps", "match", "sort", "strings", "tree"];

/// What the `buri test` workload adds to one test file before its measured run.
const EDIT: &str = "\ntest \"an edit the gate makes\" {\n    assert.equal(fromCents(7), fromCents(7));\n}\n";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).filter(|a| a != "--bench").collect();
    let bless = args.iter().any(|a| a == "--bless");
    let runs: usize = args
        .iter()
        .find_map(|a| a.strip_prefix("--runs="))
        .map_or(1, |n| n.parse().expect("--runs=<count>"));
    let filter = args.iter().find(|a| !a.starts_with("--")).cloned().unwrap_or_default();

    if !cfg!(target_os = "linux") {
        eprintln!("the instruction-count gate runs on Linux only: cachegrind does (design/PERFORMANCE.md §9)");
        std::process::exit(2);
    }
    let gate = Gate::new();
    let arch = std::env::consts::ARCH;
    let toolchain = toolchain();

    let mut measured = BTreeMap::new();
    let mut unsteady = Vec::new();
    for name in gate.workloads().into_iter().filter(|n| n.contains(&filter)) {
        let started = std::time::Instant::now();
        let counts: Vec<u64> = (0..runs).map(|_| gate.measure(&name)).collect();
        eprintln!("measured {name}: {counts:?} in {:.1?}", started.elapsed());
        let (lo, hi) = (*counts.iter().min().unwrap(), *counts.iter().max().unwrap());
        if percent(lo, hi) > STEADY * 100.0 {
            unsteady.push(format!("{name}: {lo}..{hi}, {:+.4}%", percent(lo, hi)));
        }
        measured.insert(name, lo);
    }

    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/instructions/baseline.txt");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let baseline = Baseline::parse(&text);
    let next = baseline.with(arch, &toolchain, &measured, filter.is_empty());
    let written = Path::new(ROOT).join("baseline.txt");
    std::fs::write(&written, next.render()).unwrap();

    if bless {
        std::fs::write(&path, next.render()).unwrap();
        println!("wrote {}", path.display());
        return;
    }
    let verdict = compare(&baseline, arch, &toolchain, &measured, filter.is_empty());
    println!("{}", verdict.report.trim_end());
    if verdict.failed {
        println!(
            "\nIf the change is deliberate, re-bless: run the gate with `-- --bless` (design/PERFORMANCE.md §9),\n\
             or commit {} from this run as cli/benches/instructions/baseline.txt.",
            written.display()
        );
    }
    if !unsteady.is_empty() {
        println!(
            "\nThese counts moved between runs, so they can't gate anything. Find what varies; \
             don't raise the threshold:\n  {}",
            unsteady.join("\n  ")
        );
    }
    if verdict.failed || !unsteady.is_empty() {
        std::process::exit(1);
    }
}

fn percent(base: u64, now: u64) -> f64 {
    (now as f64 - base as f64) / base as f64 * 100.0
}

/// The tools a count depends on besides this repository's code. A change to
/// any of them moves counts, so the baseline is only compared under the same.
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
    let llvm_config = std::env::var("LLVM_SYS_211_PREFIX")
        .map_or_else(|_| "llvm-config".to_string(), |p| format!("{p}/bin/llvm-config"));
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    [
        first_line("rustc", &["-V"]),
        format!("LLVM {}", first_line(&llvm_config, &["--version"])),
        first_line(&cc, &["--version"]),
        first_line("valgrind", &["--version"]),
        format!("glibc {}", glibc()),
    ]
    .join("; ")
}

/// The C library this binary, and so the `buri` built beside it, runs on.
#[cfg(target_os = "linux")]
fn glibc() -> String {
    unsafe extern "C" {
        fn gnu_get_libc_version() -> *const std::ffi::c_char;
    }
    // SAFETY: glibc returns a static, NUL-terminated string.
    unsafe { std::ffi::CStr::from_ptr(gnu_get_libc_version()) }.to_string_lossy().into_owned()
}

#[cfg(not(target_os = "linux"))]
fn glibc() -> String {
    String::from("?")
}

struct Gate {
    buri: PathBuf,
    taskset: PathBuf,
    valgrind: PathBuf,
    /// The release builds of [`PROGRAMS`], made once and run by each `run/` workload.
    built: std::cell::OnceCell<PathBuf>,
}

impl Gate {
    fn new() -> Gate {
        let root = Path::new(ROOT);
        let _ = std::fs::remove_dir_all(root);
        for dir in ["bin", "home", "out", "work"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        // The measured processes get a PATH of these links alone, so a probe
        // for a linker walks the same directories on every machine.
        let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
        let mut linked = vec![("cc", which(&cc))];
        for tool in ["clang", "mold", "ld.mold", "ld.lld", "lld"] {
            linked.push((tool, which(tool)));
        }
        for (name, found) in linked {
            if let Some(found) = found {
                std::os::unix::fs::symlink(found, root.join("bin").join(name)).unwrap();
            }
        }
        let need = |tool: &str| which(tool).unwrap_or_else(|| panic!("`{tool}` is not on PATH"));
        let gate = Gate {
            buri: PathBuf::from(env!("CARGO_BIN_EXE_buri")),
            taskset: need("taskset"),
            valgrind: need("valgrind"),
            built: std::cell::OnceCell::new(),
        };
        // The first native build on a machine records the linker's identity
        // and its command under `~/.buri`. A user pays that once, so no
        // measured run does.
        let prime = root.join("work/prime");
        copy(&Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/instructions/programs"), &prime);
        gate.unmeasured(&gate.buri, &["build", "//..."], &prime);
        gate
    }

    fn workloads(&self) -> Vec<String> {
        let mut names: Vec<String> =
            ["build/mixed-10k", "lint/mixed-10k", "build/programs", "test/one-edit"].map(String::from).to_vec();
        names.extend(PROGRAMS.iter().map(|p| format!("run/{p}")));
        names
    }

    fn measure(&self, name: &str) -> u64 {
        let buri = &self.buri;
        let fresh = |from: &Path| -> PathBuf {
            let to = Path::new(ROOT).join("work").join(name.replace('/', "-"));
            let _ = std::fs::remove_dir_all(&to);
            copy(from, &to);
            to
        };
        let benches = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches");
        match name {
            "build/mixed-10k" => self.counted(name, buri, &["build", "//..."], &fresh(&mixed_10k())),
            "lint/mixed-10k" => self.counted(name, buri, &["lint", "//..."], &fresh(&mixed_10k())),
            "build/programs" => {
                self.counted(name, buri, &["build", "//..."], &fresh(&benches.join("instructions/programs")))
            }
            "test/one-edit" => {
                let repo = fresh(&benches.join("instructions/tested"));
                self.unmeasured(buri, &["test", "//..."], &repo);
                let file = repo.join("lib/money/test/cents.buri");
                let text = std::fs::read_to_string(&file).unwrap();
                std::fs::write(&file, text + EDIT).unwrap();
                self.counted(name, buri, &["test", "//..."], &repo)
            }
            _ => {
                let program = name.strip_prefix("run/").unwrap_or_else(|| panic!("no workload {name}"));
                let built = self.built.get_or_init(|| {
                    let repo = Path::new(ROOT).join("work/programs-release");
                    copy(&benches.join("instructions/programs"), &repo);
                    self.unmeasured(buri, &["build", "--release", "//..."], &repo);
                    repo.join(".buri/out/native").join(host_variant())
                });
                self.counted(name, &built.join(program).join(program), &[], Path::new(ROOT))
            }
        }
    }

    /// Runs `program` under cachegrind, on one core, and returns the
    /// instructions it executed. Its children aren't counted.
    fn counted(&self, name: &str, program: &Path, args: &[&str], cwd: &Path) -> u64 {
        let out = Path::new(ROOT).join("out").join(format!("{}.cachegrind", name.replace('/', "-")));
        let mut argv: Vec<OsString> =
            vec!["--cpu-list".into(), "0".into(), self.valgrind.clone().into(), "--tool=cachegrind".into()];
        argv.push("--cache-sim=no".into());
        argv.push(format!("--cachegrind-out-file={}", out.display()).into());
        argv.push(program.into());
        argv.extend(args.iter().map(OsString::from));
        let ran = run(&self.taskset, &argv, cwd);
        // `buri lint` exits 1 when it has findings, and the generated corpus has some.
        if !(name.starts_with("lint/") && ran.status.code() == Some(1)) {
            checked(ran, name);
        }
        let text = std::fs::read_to_string(&out).unwrap();
        text.lines()
            .find_map(|l| l.strip_prefix("summary:"))
            .and_then(|s| s.split_whitespace().next())
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("no summary in {}", out.display()))
    }

    /// Setup: the same pinning and environment, without the count.
    fn unmeasured(&self, program: &Path, args: &[&str], cwd: &Path) {
        let mut argv: Vec<OsString> = vec!["--cpu-list".into(), "0".into(), program.into()];
        argv.extend(args.iter().map(OsString::from));
        checked(run(&self.taskset, &argv, cwd), &format!("{} {}", program.display(), args.join(" ")));
    }
}

/// One environment for every process the gate starts, whatever the shell's.
fn run(program: &Path, args: &[OsString], cwd: &Path) -> Output {
    let root = Path::new(ROOT);
    Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", root.join("bin"))
        .env("HOME", root.join("home"))
        .env("CC", root.join("bin/cc"))
        .output()
        .unwrap_or_else(|e| panic!("cannot start {}: {e}", program.display()))
}

fn checked(out: Output, what: &str) {
    if !out.status.success() {
        panic!(
            "{what} failed with {}\n{}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn which(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        return Some(PathBuf::from(program));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(program)).find(|p| p.is_file())
}

fn host_variant() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "linux-arm64",
        _ => "linux-x86_64",
    }
}

/// `cli/benches/corpora/mixed-10k` as a repository: one package, one binary.
fn mixed_10k() -> PathBuf {
    let repo = Path::new(ROOT).join("work/mixed-10k-source");
    if !repo.exists() {
        let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/corpora/mixed-10k/src");
        copy(&corpus, &repo.join("bench"));
        let mut sources: Vec<String> = std::fs::read_dir(&corpus)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != "main.buri")
            .map(|name| format!("{name:?}"))
            .collect();
        sources.sort();
        let build = format!(
            "binary {{\n    sources: [{}]\n    outputs: [{{ platform: \"node\" }}]\n}}\n",
            sources.join(", ")
        );
        std::fs::write(repo.join("REPO.buri"), "# The gate's copy of mixed-10k.\n").unwrap();
        std::fs::write(repo.join("bench/BUILD.buri"), build).unwrap();
    }
    repo
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    let mut entries: Vec<_> = std::fs::read_dir(from).unwrap().map(|e| e.unwrap().path()).collect();
    entries.sort();
    for entry in entries {
        let target = to.join(entry.file_name().unwrap());
        if entry.is_dir() {
            if entry.file_name().is_some_and(|n| n == ".buri") {
                continue;
            }
            copy(&entry, &target);
        } else {
            std::fs::copy(&entry, &target).unwrap();
        }
    }
}

/// `baseline.txt`: per architecture, the toolchain line and one line per workload.
#[derive(Default)]
struct Baseline {
    toolchains: BTreeMap<String, String>,
    counts: BTreeMap<(String, String), u64>,
}

const HEADER: &str = "\
# Instructions each workload executes under cachegrind, per architecture.
# The gate fails when one grows by more than 2%. design/PERFORMANCE.md §9 says
# how to run it and re-bless; never edit a count by hand.
";

impl Baseline {
    fn parse(text: &str) -> Baseline {
        let mut b = Baseline::default();
        for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
            if let Some(rest) = line.strip_prefix("toolchain ") {
                let (arch, tools) = rest.split_once(' ').unwrap_or((rest, ""));
                b.toolchains.insert(arch.to_string(), tools.to_string());
                continue;
            }
            let fields: Vec<&str> = line.split_whitespace().collect();
            if let [arch, name, count] = fields[..] {
                if let Ok(count) = count.parse() {
                    b.counts.insert((arch.to_string(), name.to_string()), count);
                }
            }
        }
        b
    }

    /// This baseline with `arch`'s lines replaced: all of them when `whole`,
    /// otherwise only the measured ones.
    fn with(&self, arch: &str, toolchain: &str, measured: &BTreeMap<String, u64>, whole: bool) -> Baseline {
        let mut next = Baseline { toolchains: self.toolchains.clone(), counts: self.counts.clone() };
        if whole {
            next.counts.retain(|(a, _), _| a != arch);
        }
        next.toolchains.insert(arch.to_string(), toolchain.to_string());
        for (name, count) in measured {
            next.counts.insert((arch.to_string(), name.clone()), *count);
        }
        next
    }

    fn render(&self) -> String {
        let mut out = String::from(HEADER);
        for (arch, tools) in &self.toolchains {
            out.push_str(&format!("\ntoolchain {arch} {tools}\n"));
            let width = self.counts.keys().filter(|(a, _)| a == arch).map(|(_, n)| n.len()).max().unwrap_or(0);
            for ((a, name), count) in &self.counts {
                if a == arch {
                    out.push_str(&format!("{arch} {name:<width$} {count:>14}\n"));
                }
            }
        }
        out
    }
}

struct Verdict {
    report: String,
    failed: bool,
}

fn compare(
    baseline: &Baseline,
    arch: &str,
    toolchain: &str,
    measured: &BTreeMap<String, u64>,
    whole: bool,
) -> Verdict {
    let mut report = String::new();
    let mut failed = false;
    let mut improved = false;
    report.push_str(&format!("toolchain {arch} {toolchain}\n\n"));
    report.push_str(&format!("{:<18} {:>15} {:>15} {:>9}\n", "workload", "baseline", "now", "change"));
    for (name, &now) in measured {
        match baseline.counts.get(&(arch.to_string(), name.clone())) {
            None => {
                failed = true;
                report.push_str(&format!("{name:<18} {:>15} {now:>15}   no baseline\n", "-"));
            }
            Some(&base) => {
                let change = percent(base, now);
                let mark = if change > THRESHOLD * 100.0 {
                    failed = true;
                    "  REGRESSED"
                } else if change < -THRESHOLD * 100.0 {
                    improved = true;
                    "  improved"
                } else {
                    ""
                };
                report.push_str(&format!("{name:<18} {base:>15} {now:>15} {change:>+8.3}%{mark}\n"));
            }
        }
    }
    if whole {
        for (a, name) in baseline.counts.keys() {
            if a == arch && !measured.contains_key(name) {
                failed = true;
                report.push_str(&format!("{name:<18} is in the baseline but is no longer a workload\n"));
            }
        }
    }
    match baseline.toolchains.get(arch) {
        Some(was) if was == toolchain => {}
        Some(was) => {
            failed = true;
            report.push_str(&format!(
                "\nThe toolchain changed, and counts move with it, so re-bless:\n  was {was}\n  now {toolchain}\n"
            ));
        }
        None => {
            failed = true;
            report.push_str(&format!("\nNo baseline for {arch} yet.\n"));
        }
    }
    if improved && !failed {
        report.push_str("\nSome counts fell by more than 2%. Re-bless to hold the gain.\n");
    }
    Verdict { report, failed }
}
