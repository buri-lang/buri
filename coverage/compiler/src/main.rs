//! The coverage gate: the compiler's branch coverage, counted from end-to-end
//! tests only, and held to `compiler-branches.txt`.
//!
//! ```text
//! nix develop .#coverage -c cargo run -p buri-compiler-coverage                  # run the end-to-end tests, check
//! nix develop .#coverage -c cargo run -p buri-compiler-coverage -- --bless       # and rewrite the baseline
//! nix develop .#coverage -c cargo run -p buri-compiler-coverage -- --build       # CI: build once, for the shards
//! nix develop .#coverage -c cargo run -p buri-compiler-coverage -- --shard=2/4   # CI: one shard's profile
//! nix develop .#coverage -c cargo run -p buri-compiler-coverage -- --merge=<dir> # CI: check the shards' profiles
//! ```
//!
//! Only `buri` processes count. `buri` is built with coverage on and the test
//! binaries without, so a test that calls the compiler in-process adds nothing,
//! and every `buri` a test starts adds its profile.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a gate like `cli/benches/instructions.rs`: its printing is its output, \
              and a gate that can't run its tools should stop rather than report a number."
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Branch coverage needs a nightly; the `coverage` shell pins it. `buri_coverage`
/// turns on `#[coverage(off)]`, which leaves out the few branches a race
/// decides, each with its reason beside it.
const RUSTFLAGS: &str = "-Cinstrument-coverage -Zcoverage-options=branch --cfg buri_coverage";

/// Profiles merged online into this many files per binary, so disk use doesn't
/// grow with the number of processes.
const POOL: &str = "%16m";

/// How many files `compiler.txt` lists as the least covered.
const WORST: usize = 30;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bless = args.iter().any(|a| a == "--bless");
    let gate = Gate::new();

    if args.iter().any(|a| a == "--raise") {
        gate.raise();
        return;
    }
    if args.iter().any(|a| a == "--build") {
        gate.build();
        return;
    }
    if let Some(shard) = args.iter().find_map(|a| a.strip_prefix("--shard=")) {
        gate.shard(shard);
        return;
    }
    let profiles = match args.iter().find_map(|a| a.strip_prefix("--merge=")) {
        Some(dir) => collected(Path::new(dir)),
        None => {
            gate.build();
            vec![gate.test(None)]
        }
    };
    let binary = gate.built().join("buri");
    let measured = gate.measure(&binary, &profiles);
    let unmapped = unmapped(&measured);
    if !unmapped.is_empty() {
        eprintln!(
            "these files are in no coverage area. Add them to `AREAS` in {}:\n  {}",
            file!(),
            unmapped.join("\n  ")
        );
        std::process::exit(1);
    }

    let baseline_path = gate.root.join("coverage").join("compiler-branches.txt");
    let summary_path = gate.root.join("coverage").join("compiler.txt");
    let baseline = Baseline::parse(&std::fs::read_to_string(&baseline_path).unwrap_or_default());
    let now = Baseline::of(&gate.host, &measured);

    // What this run measured, beside the build, for CI to keep as an artifact.
    std::fs::write(gate.target.join("compiler-branches.txt"), now.render()).unwrap();
    std::fs::write(gate.target.join("compiler.txt"), summary(&measured)).unwrap();
    print!("{}", summary(&measured));

    if bless {
        if !baseline.host.is_empty() && baseline.host != gate.host {
            eprintln!(
                "the baseline was measured on {}, and this is {}. Branch counts differ by \
                 platform, so bless on {} or commit the CI run's artifact.",
                baseline.host, gate.host, baseline.host
            );
            std::process::exit(2);
        }
        std::fs::write(&baseline_path, now.render()).unwrap();
        std::fs::write(&summary_path, summary(&measured)).unwrap();
        println!("\nwrote {} and {}", baseline_path.display(), summary_path.display());
        return;
    }
    if baseline.host != gate.host {
        println!(
            "\nThe baseline is for {}, not {}, so nothing was compared.",
            if baseline.host.is_empty() { "no host" } else { &baseline.host },
            gate.host
        );
        return;
    }
    let verdict = compare(&baseline, &now);
    println!("\n{}", verdict.report.trim_end());
    if verdict.failed {
        std::process::exit(1);
    }
}

struct Gate {
    /// The workspace root.
    root: PathBuf,
    /// `target/coverage`: the instrumented build and everything it writes.
    target: PathBuf,
    /// rustc's host triple.
    host: String,
    /// Where the nightly's `llvm-profdata` and `llvm-cov` are.
    tools: PathBuf,
}

impl Gate {
    fn new() -> Gate {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().to_path_buf();
        let target = root.join("target").join("coverage");
        std::fs::create_dir_all(&target).unwrap();
        let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
        let version = output(Command::new(&rustc).arg("-vV"));
        let host = version
            .lines()
            .find_map(|l| l.strip_prefix("host: "))
            .expect("rustc -vV names a host")
            .to_string();
        let sysroot = output(Command::new(&rustc).args(["--print", "sysroot"]));
        let tools = Path::new(sysroot.trim()).join("lib/rustlib").join(&host).join("bin");
        if !tools.join("llvm-cov").exists() {
            eprintln!(
                "{} has no llvm-cov. Run this inside `nix develop .#coverage`, whose nightly has \
                 llvm-tools and `-Zcoverage-options=branch`.",
                tools.display()
            );
            std::process::exit(2);
        }
        Gate { root, target, host, tools }
    }

    /// The ratchet: raises the checked-in baseline to the last measurement
    /// wherever the total or an area rose, and takes its per-file counts and
    /// summary with it.
    fn raise(&self) {
        let checked_in = self.root.join("coverage").join("compiler-branches.txt");
        let measured = self.target.join("compiler-branches.txt");
        let was = Baseline::parse(&std::fs::read_to_string(&checked_in).unwrap_or_default());
        let now = Baseline::parse(&std::fs::read_to_string(&measured).unwrap_or_else(|e| {
            panic!("{} can't be read ({e}); measure first", measured.display())
        }));
        let Some((raised, rose)) = ratchet(&was, &now) else {
            println!("neither the total nor an area rose; the baseline stays");
            return;
        };
        std::fs::write(&checked_in, raised.render()).unwrap();
        std::fs::copy(self.target.join("compiler.txt"), self.root.join("coverage").join("compiler.txt")).unwrap();
        println!("raised the baseline of {}", rose.join(", "));
    }

    /// The instrumented `buri` the tests run.
    fn buri(&self) -> PathBuf {
        self.target.join("debug").join("buri")
    }

    /// What `build` writes: the instrumented `buri` and the archived tests.
    fn built(&self) -> PathBuf {
        self.target.join("build")
    }

    /// Builds an instrumented `buri` and uninstrumented tests. Once, so every
    /// shard runs the same `buri` and its profiles merge.
    fn build(&self) {
        let started = std::time::Instant::now();
        let built = self.built();
        let _ = std::fs::remove_dir_all(&built);
        std::fs::create_dir_all(&built).unwrap();
        let instrumented = self.root.join("target").join("coverage-buri");
        run(self
            .cargo()
            .args(["build", "-p", "buri", "--bin", "buri", "--features", "backend-llvm"])
            .env("RUSTFLAGS", RUSTFLAGS)
            .env("CARGO_TARGET_DIR", &instrumented)
            // Its build scripts are instrumented too; their profiles go nowhere that counts.
            .env("LLVM_PROFILE_FILE", instrumented.join("build-scripts").join("%m.profraw")));
        std::fs::copy(instrumented.join("debug").join("buri"), built.join("buri")).unwrap();
        // Archived, so nothing rebuilds the `buri` that `test` swaps in.
        run(self
            .cargo()
            .args(["nextest", "archive", "-p", "buri", "--features", "backend-llvm", "--archive-file"])
            .arg(built.join("tests.tar.zst"))
            .env_remove("RUSTFLAGS")
            .env("CARGO_TARGET_DIR", &self.target));
        eprintln!("built in {:.0?}", started.elapsed());
    }

    /// Runs the end-to-end tests (one shard of them, if asked) against the
    /// instrumented `buri`, and merges its profiles.
    fn test(&self, shard: Option<(u32, u32)>) -> PathBuf {
        // Where every test looks for it: `env!("CARGO_BIN_EXE_buri")`.
        let _ = std::fs::remove_file(self.buri());
        std::fs::create_dir_all(self.buri().parent().unwrap()).unwrap();
        std::fs::copy(self.built().join("buri"), self.buri()).unwrap();
        let mut mode = std::fs::metadata(self.buri()).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
        std::fs::set_permissions(self.buri(), mode).unwrap();
        let archive = self.built().join("tests.tar.zst");

        let profiles = self.target.join("profiles");
        let _ = std::fs::remove_dir_all(&profiles);
        std::fs::create_dir_all(&profiles).unwrap();
        let extracted = self.target.join("extracted");
        std::fs::create_dir_all(&extracted).unwrap();
        let mut nextest = self.cargo();
        nextest
            .args(["nextest", "run", "--no-fail-fast", "--archive-file"])
            .arg(&archive)
            .arg("--workspace-remap")
            .arg(&self.root)
            .arg("--extract-to")
            .arg(&extracted)
            .arg("--extract-overwrite")
            .env("LLVM_PROFILE_FILE", profiles.join(format!("{POOL}.profraw")));
        if let Some((index, count)) = shard {
            nextest.arg(format!("--partition=count:{index}/{count}"));
        }
        let started = std::time::Instant::now();
        let status = nextest.status().expect("cargo nextest runs");
        eprintln!("the tests took {:.0?}", started.elapsed());
        if !status.success() {
            eprintln!("some end-to-end tests failed, so their coverage can't count");
            std::process::exit(1);
        }
        self.merge(&profiles, shard.map_or(0, |(index, _)| index))
    }

    /// A `cargo` in the workspace without `cargo run`'s own state.
    fn cargo(&self) -> Command {
        let mut cargo = Command::new("cargo");
        for (name, _) in std::env::vars() {
            if name.starts_with("CARGO_") && name != "CARGO_HOME" {
                cargo.env_remove(&name);
            }
        }
        cargo.current_dir(&self.root);
        cargo
    }

    /// The `buri` binary's profiles in `dir`, merged into one `.profdata`.
    fn merge(&self, dir: &Path, index: u32) -> PathBuf {
        let signature = self.signature();
        let mut ours = Vec::new();
        let mut others = 0;
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&format!("{signature}_")) {
                ours.push(entry.path());
            } else {
                others += 1;
            }
        }
        assert!(
            !ours.is_empty(),
            "no profile in {} came from {}: the tests ran no `buri`",
            dir.display(),
            self.buri().display()
        );
        ours.sort();
        eprintln!("merging {} profile(s) from buri, leaving {others} from other binaries", ours.len());
        let out = self.target.join(format!("buri-{index}.profdata"));
        run(Command::new(self.tools.join("llvm-profdata"))
            .args(["merge", "-sparse", "-o"])
            .arg(&out)
            .args(&ours));
        out
    }

    /// The signature compiler-rt names `buri`'s profiles by (`%m`).
    fn signature(&self) -> String {
        let probe = self.target.join("probe");
        let _ = std::fs::remove_dir_all(&probe);
        std::fs::create_dir_all(&probe).unwrap();
        run(Command::new(self.buri())
            .arg("version")
            .stdout(Stdio::null())
            .env("LLVM_PROFILE_FILE", probe.join("%m.profraw")));
        let names: Vec<String> = std::fs::read_dir(&probe)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        let [name] = names.as_slice() else {
            panic!("`buri version` wrote {names:?}, not one profile");
        };
        name.split('_').next().unwrap().to_string()
    }

    /// One CI shard's profile, from the `buri` that `--build` left in
    /// `target/coverage/build`.
    fn shard(&self, which: &str) {
        let (index, count) = which
            .split_once('/')
            .and_then(|(i, n)| Some((i.parse::<u32>().ok()?, n.parse::<u32>().ok()?)))
            .filter(|&(i, n)| i >= 1 && i <= n)
            .unwrap_or_else(|| panic!("--shard=<index>/<count>, from 1, not {which}"));
        let profile = self.test(Some((index, count)));
        let out = self.target.join("shard");
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).unwrap();
        std::fs::rename(&profile, out.join("profile.profdata")).unwrap();
        std::fs::write(out.join("shard"), format!("{index}/{count}\n")).unwrap();
        println!("wrote {}", out.display());
    }

    /// Per-file coverage of the compiler's own sources.
    fn measure(&self, binary: &Path, profiles: &[PathBuf]) -> BTreeMap<String, Counts> {
        let merged = if let [one] = profiles {
            one.clone()
        } else {
            let out = self.target.join("merged.profdata");
            run(Command::new(self.tools.join("llvm-profdata"))
                .args(["merge", "-sparse", "-o"])
                .arg(&out)
                .args(profiles));
            out
        };
        let report = output(
            Command::new(self.tools.join("llvm-cov"))
                .args(["report", "--show-branch-summary", "--instr-profile"])
                .arg(&merged)
                .arg(binary),
        );
        let root = self.root.display().to_string();
        let mut files = BTreeMap::new();
        for line in report.lines() {
            let Some((path, counts)) = Counts::parse(line) else { continue };
            let path = relative(&path, &root);
            if counted(&path) {
                files.insert(path, counts);
            }
        }
        assert!(files.len() > 100, "llvm-cov reported {} of the compiler's files", files.len());
        files
    }
}

/// `a` against `b` as fractions, exactly.
fn cmp((a, at): (u64, u64), (b, bt): (u64, u64)) -> std::cmp::Ordering {
    (u128::from(a) * u128::from(bt)).cmp(&(u128::from(b) * u128::from(at)))
}

/// Whether `now` sets a new baseline over `was`: there is none, there were no
/// branches, or now a larger share is covered.
fn rises(was: Option<(u64, u64)>, now: (u64, u64)) -> bool {
    match was {
        None => true,
        Some((_, 0)) => now.1 > 0,
        Some(was) => now.1 > 0 && cmp(now, was).is_gt(),
    }
}

/// `was` raised by `now`: each of the total and the areas that rose takes its
/// new count, the rest keep theirs, and the per-file counts are `now`'s. `None`
/// when nothing rose. A baseline for another host is replaced outright. Also
/// says what rose.
fn ratchet(was: &Baseline, now: &Baseline) -> Option<(Baseline, Vec<String>)> {
    if was.host != now.host {
        return Some((now.clone(), vec![format!("host {}", now.host)]));
    }
    let mut rose = Vec::new();
    let total = if rises(Some(was.total), now.total) {
        rose.push("the total".to_string());
        now.total
    } else {
        was.total
    };
    let mut areas = BTreeMap::new();
    for (area, &counts) in &now.areas {
        let old = was.areas.get(area).copied();
        if rises(old, counts) {
            rose.push(area.clone());
            areas.insert(area.clone(), counts);
        } else {
            areas.insert(area.clone(), old.unwrap_or(counts));
        }
    }
    if rose.is_empty() {
        return None;
    }
    Some((Baseline { host: now.host.clone(), files: now.files.clone(), areas, total }, rose))
}

/// `path` relative to `root`. `llvm-cov report` drops the prefix every file
/// shares, so `path` may start anywhere in `root`.
fn relative(path: &str, root: &str) -> String {
    let mut rest = root;
    loop {
        if let Some(inside) = path.strip_prefix(rest).and_then(|p| p.strip_prefix('/')) {
            return inside.to_string();
        }
        match rest.find('/') {
            Some(at) => rest = &rest[at + 1..],
            None => return path.to_string(),
        }
    }
}

/// Whether a source counts: the compiler's own, and not the runtime, a test,
/// a bench, a build script or generated code.
fn counted(path: &str) -> bool {
    if path.contains("/tests/") || path.ends_with("/tests.rs") {
        return false;
    }
    if path.starts_with("cli/src/") {
        return true;
    }
    let mut parts = path.splitn(4, '/');
    matches!((parts.next(), parts.next(), parts.next()), (Some("crates"), Some(_), Some("src")))
}

/// The areas the gate holds apart, each by the paths that start with its
/// prefix. The longest prefix wins, so every counted file is in exactly one.
const AREAS: &[(&str, &str)] = &[
    ("crates/backend/", "crates/backend"),
    ("crates/diagnostics/", "crates/diagnostics"),
    ("crates/docs/", "crates/docs"),
    ("crates/hash/", "crates/hash"),
    ("crates/js/", "crates/js"),
    ("crates/llvm/", "crates/llvm"),
    ("crates/middle/", "crates/middle"),
    ("crates/project/", "crates/project"),
    ("crates/semantics/", "crates/semantics"),
    ("crates/stdlib/", "crates/stdlib"),
    ("crates/stencil/", "crates/stencil"),
    ("crates/syntax/", "crates/syntax"),
    // `lint.rs` and `lint_cache.rs`.
    ("cli/src/commands/lint", "cli/lint"),
    ("cli/src/commands/format", "cli/format"),
    ("cli/src/commands/test", "cli/test"),
    ("cli/src/commands/run", "cli/run"),
    ("cli/src/commands/serve", "cli/serve+watch"),
    ("cli/src/commands/watch", "cli/serve+watch"),
    ("cli/src/language_server/", "cli/language_server"),
    ("cli/src/build/", "cli/build"),
    ("cli/src/documentation/", "cli/documentation"),
    ("cli/src/", "cli/other"),
];

/// The area `path` is in, if any.
fn area_of(path: &str) -> Option<&'static str> {
    AREAS
        .iter()
        .filter(|(prefix, _)| path.starts_with(prefix))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|&(_, area)| area)
}

/// The counted files in no area: a new crate, say.
fn unmapped(files: &BTreeMap<String, Counts>) -> Vec<String> {
    files.keys().filter(|path| area_of(path).is_none()).cloned().collect()
}

/// The profiles of the shards CI downloaded into `dir`, one directory each,
/// every shard present.
fn collected(dir: &Path) -> Vec<PathBuf> {
    let mut shards = BTreeMap::new();
    let mut count = None;
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let shard = entry.path();
        let Ok(which) = std::fs::read_to_string(shard.join("shard")) else { continue };
        let (index, n) = which.trim().split_once('/').unwrap();
        assert!(count.is_none_or(|c| c == n), "shards of different runs in {}", dir.display());
        count = Some(n.to_string());
        shards.insert(index.parse::<u32>().unwrap(), shard.join("profile.profdata"));
    }
    let count: u32 = count.unwrap_or_else(|| panic!("no shard in {}", dir.display())).parse().unwrap();
    let missing: Vec<u32> = (1..=count).filter(|i| !shards.contains_key(i)).collect();
    assert!(missing.is_empty(), "shards {missing:?} of {count} are missing from {}", dir.display());
    shards.into_values().collect()
}

/// One file's row of `llvm-cov report`.
#[derive(Clone, Copy, Default)]
struct Counts {
    regions: u64,
    regions_covered: u64,
    lines: u64,
    lines_covered: u64,
    branches: u64,
    branches_covered: u64,
}

impl Counts {
    /// `<file> regions missed % functions missed % lines missed % branches missed %`.
    fn parse(line: &str) -> Option<(String, Counts)> {
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.len() < 13 {
            return None;
        }
        let tail = &words[words.len() - 12..];
        let n = |i: usize| tail[i].parse::<u64>().ok();
        let (regions, regions_missed) = (n(0)?, n(1)?);
        let (lines, lines_missed) = (n(6)?, n(7)?);
        let (branches, branches_missed) = (n(9)?, n(10)?);
        let path = words[..words.len() - 12].join(" ");
        Some((
            path,
            Counts {
                regions,
                regions_covered: regions - regions_missed,
                lines,
                lines_covered: lines - lines_missed,
                branches,
                branches_covered: branches - branches_missed,
            },
        ))
    }

    fn add(&mut self, other: &Counts) {
        self.regions += other.regions;
        self.regions_covered += other.regions_covered;
        self.lines += other.lines;
        self.lines_covered += other.lines_covered;
        self.branches += other.branches;
        self.branches_covered += other.branches_covered;
    }
}

fn percent(covered: u64, total: u64) -> String {
    if total == 0 {
        return "-".to_string();
    }
    format!("{:.2}%", covered as f64 * 100.0 / total as f64)
}

/// Where in `cli/src` a source sits: a command (`commands/lint`), a
/// directory (`language_server`), or a file at the top (`json`).
fn module_of(path: &str) -> Option<String> {
    let rest = path.strip_prefix("cli/src/")?;
    let mut parts = rest.split('/');
    let first = parts.next()?;
    let name = match (first, parts.next()) {
        ("commands", Some(command)) => format!("commands/{}", command.trim_end_matches(".rs")),
        (dir, Some(_)) => dir.to_string(),
        (file, None) => file.trim_end_matches(".rs").to_string(),
    };
    Some(name)
}

/// `compiler.txt`: per crate, then the files with the most uncovered branches.
fn summary(files: &BTreeMap<String, Counts>) -> String {
    let mut areas: BTreeMap<&str, Counts> = AREAS.iter().map(|&(_, area)| (area, Counts::default())).collect();
    let mut total = Counts::default();
    for (path, counts) in files {
        if let Some(area) = area_of(path) {
            areas.entry(area).or_default().add(counts);
        }
        total.add(counts);
    }
    let mut out = String::from(
        "# The compiler's coverage from end-to-end tests only (coverage/compiler/src/main.rs).\n\
         # Branches gate, in total and per area; lines and regions are context.\n\n",
    );
    out.push_str(&format!(
        "{:<24} {:>17} {:>8} {:>17} {:>8} {:>17} {:>8}\n",
        "area", "branches", "", "lines", "", "regions", ""
    ));
    let row = |name: &str, c: &Counts| {
        format!(
            "{name:<24} {:>17} {:>8} {:>17} {:>8} {:>17} {:>8}\n",
            format!("{}/{}", c.branches_covered, c.branches),
            percent(c.branches_covered, c.branches),
            format!("{}/{}", c.lines_covered, c.lines),
            percent(c.lines_covered, c.lines),
            format!("{}/{}", c.regions_covered, c.regions),
            percent(c.regions_covered, c.regions),
        )
    };
    for (name, counts) in &areas {
        out.push_str(&row(name, counts));
    }
    out.push_str(&row("total", &total));

    let mut modules: BTreeMap<String, Counts> = BTreeMap::new();
    for (path, counts) in files {
        if let Some(module) = module_of(path) {
            modules.entry(module).or_default().add(counts);
        }
    }
    out.push_str(&format!(
        "\n{:<24} {:>17} {:>8} {:>17} {:>8} {:>17} {:>8}\n",
        "cli/src", "branches", "", "lines", "", "regions", ""
    ));
    for (name, counts) in &modules {
        out.push_str(&row(name, counts));
    }

    let mut worst: Vec<(&String, &Counts)> = files.iter().collect();
    worst.sort_by_key(|(path, c)| (std::cmp::Reverse(c.branches - c.branches_covered), path.as_str()));
    out.push_str(&format!("\nThe {WORST} files with the most uncovered branches:\n\n"));
    out.push_str(&format!("{:>9} {:>17} {:>8}  file\n", "uncovered", "branches", ""));
    for (path, c) in worst.into_iter().take(WORST) {
        out.push_str(&format!(
            "{:>9} {:>17} {:>8}  {path}\n",
            c.branches - c.branches_covered,
            format!("{}/{}", c.branches_covered, c.branches),
            percent(c.branches_covered, c.branches),
        ));
    }
    out
}

/// `compiler-branches.txt`: the host, the total's and each area's covered and
/// total branches, then each file's.
#[derive(Clone)]
struct Baseline {
    host: String,
    total: (u64, u64),
    areas: BTreeMap<String, (u64, u64)>,
    files: BTreeMap<String, (u64, u64)>,
}

impl Baseline {
    /// A measurement's baseline. Every area is in it, with no branches if no file is.
    fn of(host: &str, measured: &BTreeMap<String, Counts>) -> Baseline {
        let files: BTreeMap<String, (u64, u64)> =
            measured.iter().map(|(p, c)| (p.clone(), (c.branches_covered, c.branches))).collect();
        let mut areas: BTreeMap<String, (u64, u64)> =
            AREAS.iter().map(|&(_, area)| (area.to_string(), (0, 0))).collect();
        for (path, &(covered, total)) in &files {
            if let Some(counts) = area_of(path).and_then(|area| areas.get_mut(area)) {
                counts.0 += covered;
                counts.1 += total;
            }
        }
        Baseline { host: host.to_string(), total: sum(files.values()), areas, files }
    }

    fn parse(text: &str) -> Baseline {
        let mut host = String::new();
        let mut total = None;
        let mut areas = BTreeMap::new();
        let mut files = BTreeMap::new();
        let counts = |covered: &str, total: &str| Some((covered.parse().ok()?, total.parse().ok()?));
        for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
            let words: Vec<&str> = line.split(' ').collect();
            match words.as_slice() {
                ["host", h] => host = h.trim().to_string(),
                ["total", c, t, ..] => total = counts(c, t),
                ["area", area, c, t, ..] => {
                    if let Some(counts) = counts(c, t) {
                        areas.insert(area.to_string(), counts);
                    }
                }
                _ => {
                    let mut words = line.splitn(3, ' ');
                    let (Some(c), Some(t), Some(path)) = (words.next(), words.next(), words.next()) else {
                        continue;
                    };
                    if let Some(counts) = counts(c, t) {
                        files.insert(path.to_string(), counts);
                    }
                }
            }
        }
        // A baseline from before areas has no total line.
        let total = total.unwrap_or_else(|| sum(files.values()));
        Baseline { host, total, areas, files }
    }

    fn render(&self) -> String {
        let mut out = format!(
            "# Branches covered by end-to-end tests: covered, total, then what. The\n\
             # gate holds the total and each area to their share; files only warn.\n\
             # CI's coverage job raises it on main; never edit a count by hand.\n\
             # coverage/compiler/src/main.rs has the rest.\n\
             host {}\n\
             total {} {} {}\n",
            self.host,
            self.total.0,
            self.total.1,
            percent(self.total.0, self.total.1)
        );
        for (area, &(covered, total)) in &self.areas {
            out.push_str(&format!("area {area} {covered} {total} {}\n", percent(covered, total)));
        }
        for (path, (covered, total)) in &self.files {
            out.push_str(&format!("{covered} {total} {path}\n"));
        }
        out
    }
}

fn sum<'a>(counts: impl Iterator<Item = &'a (u64, u64)>) -> (u64, u64) {
    counts.fold((0, 0), |(c, t), &(fc, ft)| (c + fc, t + ft))
}

/// A share as its percentage and fraction: `50.00% (1/2)`.
fn share((covered, total): (u64, u64)) -> String {
    format!("{} ({covered}/{total})", percent(covered, total))
}

struct Verdict {
    report: String,
    failed: bool,
}

/// The total and every area block: a smaller covered share fails, compared as
/// exact fractions. An area with no branches, then or now, has nothing to fall
/// from. Per-file drops only warn: code that moves between files, or covered
/// code that is deleted, lowers a file's count without lowering coverage.
fn compare(baseline: &Baseline, now: &Baseline) -> Verdict {
    let mut report = format!("branch coverage: {} in the baseline, {} now\n", share(baseline.total), share(now.total));
    let falls = |was: (u64, u64), now: (u64, u64)| was.1 > 0 && now.1 > 0 && cmp(now, was).is_lt();

    let mut lost: Vec<(&str, String)> = Vec::new();
    for (path, &(covered, _)) in &now.files {
        let was = baseline.files.get(path).map_or(0, |&(c, _)| c);
        if covered < was {
            lost.push((path, format!("{path}: {was} -> {covered}")));
        }
    }
    for (path, &(was, _)) in &baseline.files {
        if was > 0 && !now.files.contains_key(path) {
            lost.push((path, format!("{path}: {was} -> gone")));
        }
    }

    let mut fell = Vec::new();
    if falls(baseline.total, now.total) {
        fell.push(format!("the total: {} -> {}", share(baseline.total), share(now.total)));
    }
    for (area, &counts) in &now.areas {
        let Some(&was) = baseline.areas.get(area) else { continue };
        if falls(was, counts) {
            fell.push(format!("{area}: {} -> {}", share(was), share(counts)));
            for (_, line) in lost.iter().filter(|(path, _)| area_of(path) == Some(area.as_str())) {
                fell.push(format!("  {line}"));
            }
        }
    }
    let failed = !fell.is_empty();

    if !lost.is_empty() {
        report.push_str(&format!(
            "\nwarning: {} file(s) cover fewer branches than the baseline. Fine if the code moved \
             or was deleted; otherwise a test stopped reaching it:\n  {}\n",
            lost.len(),
            lost.iter().map(|(_, line)| line.as_str()).collect::<Vec<_>>().join("\n  ")
        ));
    }
    if failed {
        report.push_str(&format!(
            "\nBranch coverage fell:\n  {}\n\nAdd end-to-end tests that reach the new or untested \
             branches. Never lower the baseline to pass.\n",
            fell.join("\n  ")
        ));
    } else if let Some((_, rose)) = ratchet(baseline, now) {
        report.push_str(&format!(
            "\nBranch coverage rose: {}. On main, CI raises the baseline to this run's numbers itself.\n",
            rose.join(", ")
        ));
    }
    Verdict { report, failed }
}

fn run(command: &mut Command) {
    let status = command.status().unwrap_or_else(|e| panic!("{command:?} didn't start: {e}"));
    assert!(status.success(), "{command:?} failed: {status}");
}

fn output(command: &mut Command) -> String {
    let out = command.output().unwrap_or_else(|e| panic!("{command:?} didn't start: {e}"));
    assert!(
        out.status.success(),
        "{command:?} failed: {}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYNTAX: &str = "crates/syntax/src/lexer.rs";
    const LINT: &str = "cli/src/commands/lint.rs";

    /// A run that measured `files`: covered and total branches each.
    fn baseline(files: &[(&str, u64, u64)]) -> Baseline {
        let measured = files
            .iter()
            .map(|&(path, covered, total)| {
                (path.to_string(), Counts { branches: total, branches_covered: covered, ..Counts::default() })
            })
            .collect();
        Baseline::of("h", &measured)
    }

    #[test]
    fn a_lower_total_share_fails_however_small() {
        // 66.67% either way, to two places, but 666_666/1_000_000 is under 2/3.
        let was = baseline(&[(SYNTAX, 2, 3)]);
        let verdict = compare(&was, &baseline(&[(SYNTAX, 666_666, 1_000_000)]));
        assert!(verdict.failed, "{}", verdict.report);
        assert!(verdict.report.contains("the total: 66.67% (2/3) -> 66.67%"), "{}", verdict.report);
    }

    #[test]
    fn an_area_falling_fails_though_the_total_rises() {
        let was = baseline(&[(SYNTAX, 5, 10), ("crates/syntax/src/parser.rs", 5, 10), (LINT, 0, 10)]);
        let now = baseline(&[(SYNTAX, 4, 10), ("crates/syntax/src/parser.rs", 5, 10), (LINT, 9, 10)]);
        let verdict = compare(&was, &now);
        assert!(verdict.failed, "{}", verdict.report);
        assert!(
            verdict.report.contains(
                "Branch coverage fell:\n  crates/syntax: 50.00% (10/20) -> 45.00% (9/20)\n    \
                 crates/syntax/src/lexer.rs: 5 -> 4\n\n"
            ),
            "{}",
            verdict.report
        );
        assert!(!verdict.report.contains("the total:"), "{}", verdict.report);
    }

    #[test]
    fn an_equal_share_of_more_branches_passes() {
        let verdict = compare(&baseline(&[(SYNTAX, 1, 2)]), &baseline(&[(SYNTAX, 5, 10)]));
        assert!(!verdict.failed, "{}", verdict.report);
    }

    #[test]
    fn an_area_with_no_branches_never_fails() {
        let was = baseline(&[(SYNTAX, 5, 10), (LINT, 1, 2)]);
        // Lint's code deleted, then new lint code that no test reaches yet.
        assert!(!compare(&was, &baseline(&[(SYNTAX, 5, 10)])).failed);
        let empty = baseline(&[(SYNTAX, 5, 10), (LINT, 0, 0)]);
        assert!(!compare(&empty, &baseline(&[(SYNTAX, 10, 10), (LINT, 0, 5)])).failed);
    }

    #[test]
    fn code_moving_between_files_warns_and_passes() {
        let was = baseline(&[(SYNTAX, 6, 10), ("crates/syntax/src/b.rs", 0, 0)]);
        let verdict = compare(&was, &baseline(&[(SYNTAX, 3, 5), ("crates/syntax/src/b.rs", 3, 5)]));
        assert!(!verdict.failed, "{}", verdict.report);
        assert!(verdict.report.contains("warning: 1 file(s)"), "{}", verdict.report);
        assert!(verdict.report.contains("lexer.rs: 6 -> 3"), "{}", verdict.report);
    }

    #[test]
    fn a_gain_passes_and_names_what_rose() {
        let was = baseline(&[(SYNTAX, 5, 10), (LINT, 5, 10)]);
        let verdict = compare(&was, &baseline(&[(SYNTAX, 6, 10), (LINT, 5, 10)]));
        assert!(!verdict.failed);
        assert!(verdict.report.contains("rose: the total, crates/syntax. On main"), "{}", verdict.report);
    }

    #[test]
    fn the_ratchet_raises_what_rose_and_keeps_the_rest() {
        let was = baseline(&[(SYNTAX, 5, 10), (LINT, 5, 10)]);
        // Only syntax rises. Lint and the total fall, which fails the gate, but
        // the ratchet keeps their baselines anyway.
        let now = baseline(&[(SYNTAX, 6, 10), (LINT, 1, 10)]);
        let (raised, rose) = ratchet(&was, &now).unwrap();
        assert_eq!(rose, ["crates/syntax"]);
        assert_eq!(raised.total, (10, 20));
        assert_eq!(raised.areas["crates/syntax"], (6, 10));
        assert_eq!(raised.areas["cli/lint"], (5, 10));
        assert_eq!(raised.areas["cli/format"], (0, 0));
        assert_eq!(raised.files, now.files);

        let (raised, rose) = ratchet(&was, &baseline(&[(SYNTAX, 5, 10), (LINT, 9, 10)])).unwrap();
        assert_eq!(rose, ["the total", "cli/lint"]);
        assert_eq!((raised.total, raised.areas["cli/lint"]), ((14, 20), (9, 10)));
    }

    #[test]
    fn nothing_rising_leaves_the_baseline() {
        let was = baseline(&[(SYNTAX, 5, 10), (LINT, 5, 10)]);
        assert!(ratchet(&was, &baseline(&[(SYNTAX, 5, 10), (LINT, 10, 20)])).is_none());
        assert!(ratchet(&was, &baseline(&[(SYNTAX, 5, 10), (LINT, 0, 0)])).is_none());
    }

    #[test]
    fn a_baseline_without_areas_takes_every_area_and_keeps_its_total() {
        let old = "host h\n5 10 crates/syntax/src/lexer.rs\n";
        let was = Baseline::parse(old);
        assert_eq!(was.total, (5, 10));
        assert!(compare(&was, &baseline(&[(SYNTAX, 4, 10)])).failed);
        let (raised, rose) = ratchet(&was, &baseline(&[(SYNTAX, 5, 10)])).unwrap();
        assert_eq!(rose.len(), raised.areas.len());
        assert!(ratchet(&Baseline::parse(""), &was).is_some());
    }

    #[test]
    fn the_baseline_round_trips() {
        let mut was = baseline(&[("cli/src/a b.rs", 1, 2), (SYNTAX, 0, 0)]);
        was.total = (7, 9);
        let again = Baseline::parse(&was.render());
        assert_eq!(again.host, "h");
        assert_eq!(again.total, (7, 9));
        assert_eq!(again.areas, was.areas);
        assert_eq!(again.files, was.files);
        assert!(was.render().contains("\narea cli/other 1 2 50.00%\n"), "{}", was.render());
    }

    #[test]
    fn every_file_is_in_exactly_one_area() {
        let cases = [
            ("crates/syntax/src/lexer.rs", "crates/syntax"),
            ("cli/src/commands/lint.rs", "cli/lint"),
            ("cli/src/commands/lint_cache.rs", "cli/lint"),
            ("cli/src/commands/test.rs", "cli/test"),
            ("cli/src/commands/serve.rs", "cli/serve+watch"),
            ("cli/src/commands/watch.rs", "cli/serve+watch"),
            ("cli/src/commands/build.rs", "cli/other"),
            ("cli/src/build/link.rs", "cli/build"),
            ("cli/src/language_server/mod.rs", "cli/language_server"),
            ("cli/src/main.rs", "cli/other"),
        ];
        for (path, area) in cases {
            assert_eq!(area_of(path), Some(area), "{path}");
        }
        let prefixes: std::collections::BTreeSet<&str> = AREAS.iter().map(|&(prefix, _)| prefix).collect();
        assert_eq!(prefixes.len(), AREAS.len(), "two areas share a prefix");
    }

    #[test]
    fn a_file_in_no_area_is_named() {
        let files: BTreeMap<String, Counts> = [("crates/new/src/lib.rs", 1, 2), (SYNTAX, 1, 2)]
            .into_iter()
            .map(|(p, c, t)| (p.to_string(), Counts { branches: t, branches_covered: c, ..Counts::default() }))
            .collect();
        assert_eq!(unmapped(&files), ["crates/new/src/lib.rs"]);
    }

    #[test]
    fn every_file_in_the_baseline_is_in_an_area() {
        let checked_in = Baseline::parse(include_str!("../../compiler-branches.txt"));
        let missing: Vec<&String> = checked_in.files.keys().filter(|path| area_of(path).is_none()).collect();
        assert!(missing.is_empty(), "{missing:?}");
    }

    #[test]
    fn a_report_row_parses() {
        let row = "/r/cli/src/main.rs  13  2  84.62%  2  0  100.00%  10  1  90.00%  6  3  50.00%";
        let (path, c) = Counts::parse(row).unwrap();
        assert_eq!(path, "/r/cli/src/main.rs");
        assert_eq!((c.regions_covered, c.lines_covered, c.branches_covered, c.branches), (11, 9, 3, 6));
        let none = "/r/x.rs  1  0  100.00%  1  0  100.00%  1  0  100.00%  0  0  -";
        assert_eq!(Counts::parse(none).unwrap().1.branches, 0);
    }

    #[test]
    fn a_path_is_made_relative_however_much_llvm_cov_dropped() {
        let root = "/home/r/work/buri";
        for path in ["/home/r/work/buri/cli/src/a.rs", "home/r/work/buri/cli/src/a.rs", "work/buri/cli/src/a.rs"] {
            assert_eq!(relative(path, root), "cli/src/a.rs");
        }
        assert_eq!(relative(".cargo/registry/x.rs", root), ".cargo/registry/x.rs");
    }

    #[test]
    fn the_cli_is_broken_down_by_command_and_module() {
        assert_eq!(module_of("cli/src/commands/lint.rs").as_deref(), Some("commands/lint"));
        assert_eq!(module_of("cli/src/commands/test/runner.rs").as_deref(), Some("commands/test"));
        assert_eq!(module_of("cli/src/language_server/mod.rs").as_deref(), Some("language_server"));
        assert_eq!(module_of("cli/src/json.rs").as_deref(), Some("json"));
        assert_eq!(module_of("crates/syntax/src/lexer.rs"), None);
    }

    #[test]
    fn only_the_compilers_own_sources_count() {
        assert!(counted("cli/src/main.rs"));
        assert!(counted("crates/syntax/src/lexer.rs"));
        assert!(!counted("cli/runtime/lib.rs"));
        assert!(!counted("cli/src/compiler/tests/mod.rs"));
        assert!(!counted("crates/stencil/build.rs"));
        assert!(!counted("target/debug/build/x/out/gen.rs"));
    }
}
