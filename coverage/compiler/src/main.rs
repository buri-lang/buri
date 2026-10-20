//! The coverage gate: the compiler's branch coverage, counted from end-to-end
//! tests only, merged across platforms, and held to `compiler-branches.txt`.
//!
//! ```text
//! nix develop .#coverage -c cargo run -p buri-compiler-coverage                  # this host's tests, measured
//! nix develop .#coverage -c cargo run -p buri-compiler-coverage -- --build       # CI: build once, for the shards
//! nix develop .#coverage -c cargo run -p buri-compiler-coverage -- --shard=2/4   # CI: one shard's lcov
//! nix develop .#coverage -c cargo run -p buri-compiler-coverage -- --release     # any of the three, of a release `buri`
//! cargo run -p buri-compiler-coverage -- --merge=<dir>                           # CI: merge every platform's shards, gate
//! cargo run -p buri-compiler-coverage -- --raise                                 # CI: the ratchet
//! ```
//!
//! Only `buri` processes count. `buri` is built with coverage on and the test
//! binaries without, so a test that calls the compiler in-process adds nothing,
//! and every `buri` a test starts adds its profile. `design/COVERAGE.md` says
//! how platforms merge.
//!
//! `--release` measures a `buri` built in the release profile, without debug
//! assertions, as one more platform: `<host>+release`. So a branch on
//! `cfg!(debug_assertions)` counts both ways.

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

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

mod json;

/// Branch coverage needs a nightly; the `coverage` shell pins it.
const RUSTFLAGS: &str = "-Cinstrument-coverage -Zcoverage-options=branch";

/// Profiles merged online into this many files per binary, so disk use doesn't
/// grow with the number of processes. Not continuous (`%c`): most counts are
/// worked out from others, so a killed process would cover what it never reached.
const POOL: &str = "%16m";

/// How many files `compiler.txt` lists as the least covered.
const WORST: usize = 30;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = root();
    let release = args.iter().any(|a| a == "--release");
    if args.iter().any(|a| a == "--raise") {
        raise(&root);
        return;
    }
    if args.iter().any(|a| a == "--build") {
        Gate::new(root, release).build();
        return;
    }
    if let Some(shard) = args.iter().find_map(|a| a.strip_prefix("--shard=")) {
        Gate::new(root, release).shard(shard);
        return;
    }
    match args.iter().find_map(|a| a.strip_prefix("--merge=")) {
        Some(dir) => check(&root, &shards(Path::new(dir)), true),
        None => {
            let gate = Gate::new(root.clone(), release);
            gate.build();
            gate.shard("1/1");
            check(&root, &shards(&gate.target.join("shard")), false);
        }
    }
}

/// The workspace: the nearest directory up from here with a `Cargo.lock`.
fn root() -> PathBuf {
    let here = std::env::current_dir().unwrap();
    here.ancestors()
        .find(|dir| dir.join("Cargo.lock").exists())
        .unwrap_or_else(|| panic!("no Cargo.lock in {} or above it", here.display()))
        .to_path_buf()
}

/// `target/coverage`, where everything this writes goes.
fn target(root: &Path) -> PathBuf {
    root.join("target").join("coverage")
}

/// Merges every platform's shards, writes what it measured beside the build,
/// and holds it to the baseline. `ci` is `--merge`, where a platform the
/// baseline has must be there; a local run measures one host and compares
/// nothing then.
fn check(root: &Path, shards: &BTreeMap<String, Measured>, ci: bool) {
    let target = target(root);
    let lcov = target.join("lcov");
    let _ = std::fs::remove_dir_all(&lcov);
    std::fs::create_dir_all(&lcov).unwrap();
    let mut everywhere = Measured::default();
    for (host, platform) in shards {
        std::fs::write(lcov.join(format!("{host}.lcov")), platform.lcov()).unwrap();
        everywhere.add(platform);
    }
    std::fs::write(lcov.join("merged.lcov"), everywhere.lcov()).unwrap();

    let hosts: BTreeSet<String> = shards.keys().cloned().collect();
    let measured = merge(shards);
    let unmapped = unmapped(&measured);
    if !unmapped.is_empty() {
        eprintln!(
            "these files are in no coverage area. Add them to `AREAS` in {}:\n  {}",
            file!(),
            unmapped.join("\n  ")
        );
        std::process::exit(1);
    }
    let now = Baseline::of(&hosts, &measured);
    std::fs::write(target.join("compiler-branches.txt"), now.render()).unwrap();
    std::fs::write(target.join("compiler.txt"), summary(&hosts, &measured)).unwrap();
    print!("{}", summary(&hosts, &measured));

    let baseline_path = root.join("coverage").join("compiler-branches.txt");
    let baseline = Baseline::parse(&std::fs::read_to_string(&baseline_path).unwrap_or_default());
    if baseline.hosts.is_empty() {
        println!("\nThere is no baseline, so nothing was compared.");
        return;
    }
    let missing: Vec<&String> = baseline.hosts.difference(&hosts).collect();
    if !missing.is_empty() {
        println!(
            "\nThe baseline merges {}, and this run has no shard from it.",
            list(missing.iter().copied())
        );
        if ci {
            std::process::exit(1);
        }
        return;
    }
    let compared = if baseline.hosts == hosts {
        now
    } else {
        let alone = list(&baseline.hosts);
        println!(
            "\nThe baseline is {alone}'s alone, so this run's {alone} alone was compared with it. \
             On main, CI then replaces it with this run's merged numbers."
        );
        let only = shards
            .iter()
            .filter(|(host, _)| baseline.hosts.contains(*host));
        Baseline::of(
            &baseline.hosts,
            &merge(&only.map(|(h, l)| (h.clone(), l.clone())).collect()),
        )
    };
    let verdict = compare(&baseline, &compared);
    println!("\n{}", verdict.report.trim_end());
    if verdict.failed {
        std::process::exit(1);
    }
}

/// The ratchet: raises the checked-in baseline to the last measurement
/// wherever the total or an area rose, and takes its per-file counts and
/// summary with it. A measurement that merges more platforms replaces it.
fn raise(root: &Path) {
    let checked_in = root.join("coverage").join("compiler-branches.txt");
    let measured = target(root).join("compiler-branches.txt");
    let was = Baseline::parse(&std::fs::read_to_string(&checked_in).unwrap_or_default());
    let now =
        Baseline::parse(&std::fs::read_to_string(&measured).unwrap_or_else(|e| {
            panic!("{} can't be read ({e}); measure first", measured.display())
        }));
    let raised = if was.hosts == now.hosts {
        let Some((raised, rose)) = ratchet(&was, &now) else {
            println!("neither the total nor an area rose; the baseline stays");
            return;
        };
        println!("raised the baseline of {}", rose.join(", "));
        raised
    } else {
        let missing: Vec<&String> = was.hosts.difference(&now.hosts).collect();
        if !missing.is_empty() {
            println!(
                "this run has no shard from {}; the baseline stays",
                list(missing.iter().copied())
            );
            return;
        }
        println!("raised the baseline to merge {}", list(&now.hosts));
        now
    };
    std::fs::write(&checked_in, raised.render()).unwrap();
    std::fs::copy(
        target(root).join("compiler.txt"),
        root.join("coverage").join("compiler.txt"),
    )
    .unwrap();
}

fn list<'a>(names: impl IntoIterator<Item = &'a String>) -> String {
    names
        .into_iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Builds and runs the instrumented `buri` on this host.
struct Gate {
    /// The workspace root.
    root: PathBuf,
    /// `target/coverage`: the instrumented build and everything it writes.
    target: PathBuf,
    /// rustc's host triple, and `+release` for a release `buri`.
    host: String,
    /// Whether `buri` is built in the release profile.
    release: bool,
    /// Where the nightly's `llvm-profdata` and `llvm-cov` are.
    tools: PathBuf,
}

impl Gate {
    fn new(root: PathBuf, release: bool) -> Gate {
        let target = target(&root);
        std::fs::create_dir_all(&target).unwrap();
        let (host, tools) = toolchain();
        if !tools.join("llvm-cov").exists() {
            eprintln!(
                "{} has no llvm-cov. Run this inside `nix develop .#coverage`, whose nightly has \
                 llvm-tools and `-Zcoverage-options=branch`.",
                tools.display()
            );
            std::process::exit(2);
        }
        Gate {
            root,
            target,
            host: if release {
                format!("{host}+release")
            } else {
                host
            },
            release,
            tools,
        }
    }

    /// The instrumented `buri` the tests run.
    fn buri(&self) -> PathBuf {
        self.target.join("debug").join("buri")
    }

    /// What `build` writes: the instrumented `buri`, the archived tests, and
    /// the root `buri` was built in, which its source paths start with.
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
        let mut cargo = self.cargo();
        cargo
            .args([
                "build",
                "-p",
                "buri",
                "--bin",
                "buri",
                "--features",
                "backend-llvm,test-hooks",
            ])
            .env("RUSTFLAGS", RUSTFLAGS)
            .env("CARGO_TARGET_DIR", &instrumented)
            // Its build scripts are instrumented too; their profiles go nowhere that counts.
            .env(
                "LLVM_PROFILE_FILE",
                instrumented.join("build-scripts").join("%m.profraw"),
            );
        if self.release {
            // The release profile without its link-time optimization, which
            // changes how long the build takes and not which branches it has.
            cargo
                .arg("--release")
                .env("CARGO_PROFILE_RELEASE_LTO", "false")
                .env("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "16");
        }
        run(&mut cargo);
        let profile = if self.release { "release" } else { "debug" };
        std::fs::copy(instrumented.join(profile).join("buri"), built.join("buri")).unwrap();
        std::fs::write(built.join("root"), self.root.display().to_string()).unwrap();
        // Archived, so nothing rebuilds the `buri` that `test` swaps in.
        run(self
            .cargo()
            .args([
                "nextest",
                "archive",
                "-p",
                "buri",
                "--features",
                "backend-llvm",
                "--archive-file",
            ])
            .arg(built.join("tests.tar.zst"))
            .env_remove("RUSTFLAGS")
            .env("CARGO_TARGET_DIR", &self.target));
        eprintln!("built in {:.0?}", started.elapsed());
    }

    /// Runs the end-to-end tests (one shard of them, if asked) against the
    /// instrumented `buri`, and merges its profiles.
    fn test(&self, (index, count): (u32, u32)) -> PathBuf {
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
            .env(
                "LLVM_PROFILE_FILE",
                profiles.join(format!("{POOL}.profraw")),
            );
        if count > 1 {
            nextest.arg(format!("--partition=count:{index}/{count}"));
        }
        let started = std::time::Instant::now();
        let status = nextest.status().expect("cargo nextest runs");
        eprintln!("the tests took {:.0?}", started.elapsed());
        if !status.success() {
            eprintln!("some end-to-end tests failed, so their coverage can't count");
            std::process::exit(1);
        }
        self.merge(&profiles)
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
    fn merge(&self, dir: &Path) -> PathBuf {
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
        eprintln!(
            "merging {} profile(s) from buri, leaving {others} from other binaries",
            ours.len()
        );
        let out = self.target.join("buri.profdata");
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

    /// One shard's lcov, from the `buri` that `--build` left in
    /// `target/coverage/build`, written to `target/coverage/shard`.
    fn shard(&self, which: &str) {
        let (index, count) = which
            .split_once('/')
            .and_then(|(i, n)| Some((i.parse::<u32>().ok()?, n.parse::<u32>().ok()?)))
            .filter(|&(i, n)| i >= 1 && i <= n)
            .unwrap_or_else(|| panic!("--shard=<index>/<count>, from 1, not {which}"));
        let profile = self.test((index, count));
        let measured = self.export(&profile);
        let out = self.target.join("shard");
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("coverage.txt"), measured.render()).unwrap();
        std::fs::write(
            out.join("shard"),
            format!("{} {index}/{count}\n", self.host),
        )
        .unwrap();
        println!("wrote {}", out.display());
    }

    /// What `profile` took in the compiler's own sources, each named from the
    /// root `buri` was built in.
    fn export(&self, profile: &Path) -> Measured {
        let text = output(
            Command::new(self.tools.join("llvm-cov"))
                .args(["export", "-format=text", "--instr-profile"])
                .arg(profile)
                .arg(self.built().join("buri")),
        );
        let export =
            json::parse(&text).unwrap_or_else(|e| panic!("llvm-cov export wrote bad JSON: {e}"));
        let built_in = std::fs::read_to_string(self.built().join("root")).unwrap();
        let prefix = format!("{}/", built_in.trim());
        let measured = Measured::of_export(&export, |path| {
            Some(lexical(path.strip_prefix(&prefix).unwrap_or(path))).filter(|p| counted(p))
        });
        assert!(
            measured.files.len() > 100,
            "llvm-cov exported {} of the compiler's files",
            measured.files.len()
        );
        measured
    }
}

/// `path` with each `..` taking off the directory before it, so a file a
/// `#[path = "../runtime/x.rs"]` names is under `cli/runtime`.
fn lexical(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            ".." if parts.last().is_some_and(|p| *p != "..") => {
                parts.pop();
            }
            "." => {}
            _ => parts.push(part),
        }
    }
    parts.join("/")
}

/// The shards in `dir` (or `dir` itself, if it's one), each platform's
/// merged. Every shard of a platform must be there.
fn shards(dir: &Path) -> BTreeMap<String, Measured> {
    let mut dirs = vec![dir.to_path_buf()];
    if !dir.join("shard").exists() {
        dirs = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
    }
    let mut found: BTreeMap<String, (u32, BTreeMap<u32, PathBuf>)> = BTreeMap::new();
    for shard in dirs {
        let Ok(which) = std::fs::read_to_string(shard.join("shard")) else {
            continue;
        };
        let parsed = which.split_once(' ').and_then(|(host, which)| {
            let (index, count) = which.trim().split_once('/')?;
            Some((
                host.to_string(),
                index.parse::<u32>().ok()?,
                count.parse::<u32>().ok()?,
            ))
        });
        let (host, index, count) =
            parsed.unwrap_or_else(|| panic!("{} isn't `<host> <index>/<count>`", which.trim()));
        let entry = found
            .entry(host.clone())
            .or_insert((count, BTreeMap::new()));
        assert_eq!(
            entry.0,
            count,
            "shards of different runs from {host} in {}",
            dir.display()
        );
        entry.1.insert(index, shard.join("coverage.txt"));
    }
    assert!(!found.is_empty(), "no shard in {}", dir.display());
    let mut platforms = BTreeMap::new();
    for (host, (count, files)) in found {
        let missing: Vec<u32> = (1..=count).filter(|i| !files.contains_key(i)).collect();
        assert!(
            missing.is_empty(),
            "{host} is missing shard {missing:?} of {count} in {}",
            dir.display()
        );
        let mut merged = Measured::default();
        for path in files.values() {
            merged.add(&Measured::parse(&std::fs::read_to_string(path).unwrap()));
        }
        platforms.insert(host, merged);
    }
    platforms
}

/// One side of one branch: where its function starts, its condition's span,
/// which of the function's conditions with that span it is (a guard on an
/// or-pattern has one per alternative), and the side, 0 true and 1 false.
/// Instantiations of a generic function share it, as `llvm-cov report`'s
/// instantiation groups do.
type Side = ([u32; 2], [u32; 4], u32, u32);

/// A `Side` without its side.
type Condition = ([u32; 2], [u32; 4], u32);

/// What one platform's tests took, per file.
#[derive(Clone, Default)]
struct Measured {
    files: BTreeMap<String, Record>,
}

#[derive(Clone, Default)]
struct Record {
    /// Sides whose count is a constant, which `llvm-cov report` leaves out. The
    /// export doesn't say which they are, only how many.
    folded: u64,
    /// Line to hits.
    lines: BTreeMap<u32, u64>,
    /// Side to times taken.
    branches: BTreeMap<Side, u64>,
}

impl Measured {
    /// `llvm-cov export`'s JSON, with only the files `name` keeps, renamed.
    fn of_export(export: &json::Value, name: impl Fn(&str) -> Option<String>) -> Measured {
        let data = &export.get("data").expect("an export has data").items()[0];
        let mut measured = Measured::default();
        for function in data.get("functions").map_or(&[][..], json::Value::items) {
            let filenames: Vec<Option<String>> = function
                .get("filenames")
                .map_or(&[][..], json::Value::items)
                .iter()
                .map(|f| name(f.str()))
                .collect();
            let regions = function.get("regions").map_or(&[][..], json::Value::items);
            let Some(first) = regions.first() else {
                continue;
            };
            let starts = [first.items()[0].u64() as u32, first.items()[1].u64() as u32];
            let mut seen: BTreeMap<(usize, [u32; 4]), u32> = BTreeMap::new();
            for branch in function.get("branches").map_or(&[][..], json::Value::items) {
                let b: Vec<u64> = branch.items().iter().map(json::Value::u64).collect();
                let file = b[6] as usize;
                let span = [b[0] as u32, b[1] as u32, b[2] as u32, b[3] as u32];
                let copy = seen.entry((file, span)).or_default();
                let nth = *copy;
                *copy += 1;
                let Some(Some(path)) = filenames.get(file) else {
                    continue;
                };
                let record = measured.files.entry(path.clone()).or_default();
                *record.branches.entry((starts, span, nth, 0)).or_default() += b[4];
                *record.branches.entry((starts, span, nth, 1)).or_default() += b[5];
            }
        }
        for file in data.get("files").map_or(&[][..], json::Value::items) {
            let Some(path) = name(file.get("filename").map_or("", json::Value::str)) else {
                continue;
            };
            let record = measured.files.entry(path).or_default();
            let counted = file
                .get("summary")
                .and_then(|s| s.get("branches"))
                .and_then(|b| b.get("count"));
            record.folded =
                (record.branches.len() as u64).saturating_sub(counted.map_or(0, json::Value::u64));
            record.lines = lines(file.get("segments").map_or(&[][..], json::Value::items));
        }
        measured
    }

    /// `coverage.txt`, as `render` writes it.
    fn parse(text: &str) -> Measured {
        let mut measured = Measured::default();
        let mut file = None;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("file ") {
                let (folded, path) = rest.split_once(' ').expect("file <folded> <path>");
                let record = measured.files.entry(path.to_string()).or_default();
                record.folded = folded.parse().expect("folded is a count");
                file = Some(record);
                continue;
            }
            let numbers: Vec<u64> = line
                .split(' ')
                .skip(1)
                .map(|n| n.parse().expect("a count"))
                .collect();
            let record = file
                .as_deref_mut()
                .expect("a line or branch comes after its file");
            match (line.split(' ').next(), numbers.as_slice()) {
                (Some("line"), &[at, hits]) => *record.lines.entry(at as u32).or_default() += hits,
                (Some("branch"), &[gl, gc, ls, cs, le, ce, nth, side, taken]) => {
                    let n = |x: u64| x as u32;
                    let key = (
                        [n(gl), n(gc)],
                        [n(ls), n(cs), n(le), n(ce)],
                        n(nth),
                        n(side),
                    );
                    *record.branches.entry(key).or_default() += taken;
                }
                _ => panic!("`{line}` is neither a line nor a branch"),
            }
        }
        measured
    }

    fn render(&self) -> String {
        let mut out = String::new();
        for (path, record) in &self.files {
            out.push_str(&format!("file {} {path}\n", record.folded));
            for (at, hits) in &record.lines {
                out.push_str(&format!("line {at} {hits}\n"));
            }
            for (([gl, gc], [ls, cs, le, ce], nth, side), taken) in &record.branches {
                out.push_str(&format!(
                    "branch {gl} {gc} {ls} {cs} {le} {ce} {nth} {side} {taken}\n"
                ));
            }
        }
        out
    }

    /// Adds another run of the same binary: counts sum.
    fn add(&mut self, other: &Measured) {
        for (path, theirs) in &other.files {
            let ours = self.files.entry(path.clone()).or_default();
            ours.folded = ours.folded.max(theirs.folded);
            for (&at, &hits) in &theirs.lines {
                *ours.lines.entry(at).or_default() += hits;
            }
            for (&key, &taken) in &theirs.branches {
                *ours.branches.entry(key).or_default() += taken;
            }
        }
    }

    /// As lcov, for anything that reads it: each condition is a block on the
    /// line it starts on, numbered in order there.
    fn lcov(&self) -> String {
        let mut out = String::new();
        for (path, record) in &self.files {
            out.push_str(&format!("SF:{path}\n"));
            for (at, hits) in &record.lines {
                out.push_str(&format!("DA:{at},{hits}\n"));
            }
            let hit = |counts: &mut dyn Iterator<Item = &u64>| counts.filter(|&&c| c > 0).count();
            out.push_str(&format!(
                "LF:{}\nLH:{}\n",
                record.lines.len(),
                hit(&mut record.lines.values())
            ));
            let mut blocks: BTreeMap<u32, Vec<Condition>> = BTreeMap::new();
            for &(starts, span, nth, _) in record.branches.keys() {
                let on_line = blocks.entry(span[0]).or_default();
                if on_line.last() != Some(&(starts, span, nth)) {
                    on_line.push((starts, span, nth));
                }
            }
            for (at, conditions) in &blocks {
                for (block, &(starts, span, nth)) in conditions.iter().enumerate() {
                    for side in 0..2 {
                        let taken = record
                            .branches
                            .get(&(starts, span, nth, side))
                            .copied()
                            .unwrap_or(0);
                        out.push_str(&format!(
                            "BRDA:{at},{block},{},{taken}\n",
                            block as u32 * 2 + side
                        ));
                    }
                }
            }
            out.push_str(&format!(
                "BRF:{}\nBRH:{}\n",
                record.branches.len(),
                hit(&mut record.branches.values())
            ));
            out.push_str("end_of_record\n");
        }
        out
    }
}

/// Each line's hits from a file's segments, as `llvm-cov` counts them: a line
/// is mapped if a region starts on it or one runs over it, and its count is
/// the largest of those.
fn lines(segments: &[json::Value]) -> BTreeMap<u32, u64> {
    // [line, column, count, has count, is region entry, is gap region]
    let segments: Vec<Vec<u64>> = segments
        .iter()
        .map(|s| s.items().iter().map(json::Value::u64).collect())
        .collect();
    let mut out = BTreeMap::new();
    let Some(first) = segments.first() else {
        return out;
    };
    let starts = |s: &Vec<u64>| s[5] == 0 && s[3] != 0 && s[4] != 0;
    let mut wrapped: Option<&Vec<u64>> = None;
    let mut next = 0;
    let mut line = first[0];
    while next < segments.len() {
        let on_line: Vec<&Vec<u64>> = segments[next..]
            .iter()
            .take_while(|s| s[0] == line)
            .collect();
        next += on_line.len();
        let entries: Vec<&&Vec<u64>> = on_line.iter().filter(|s| starts(s)).collect();
        let skipped = on_line.first().is_some_and(|s| s[3] == 0 && s[4] != 0);
        if !skipped && (wrapped.is_some_and(|w| w[3] != 0) || !entries.is_empty()) {
            let count = entries
                .iter()
                .map(|s| s[2])
                .chain(wrapped.map(|w| w[2]))
                .max()
                .unwrap_or(0);
            out.insert(line as u32, count);
        }
        if let Some(last) = on_line.last() {
            wrapped = Some(last);
        }
        line += 1;
    }
    out
}

/// Each file's coverage over every platform. A side is covered if any
/// platform's tests took it; a side only some platforms compile counts once.
/// Of each file's folded sides, the fewest any platform has are left out.
fn merge(platforms: &BTreeMap<String, Measured>) -> BTreeMap<String, Counts> {
    let paths: BTreeSet<&String> = platforms.values().flat_map(|p| p.files.keys()).collect();
    let mut files = BTreeMap::new();
    for path in paths {
        let records: Vec<&Record> = platforms
            .values()
            .filter_map(|p| p.files.get(path))
            .collect();
        let mut lines: BTreeMap<u32, bool> = BTreeMap::new();
        let mut branches: BTreeMap<Side, bool> = BTreeMap::new();
        for record in &records {
            for (&at, &hits) in &record.lines {
                *lines.entry(at).or_default() |= hits > 0;
            }
            for (&side, &taken) in &record.branches {
                *branches.entry(side).or_default() |= taken > 0;
            }
        }
        let folded = records.iter().map(|r| r.folded).min().unwrap_or(0);
        let counts = Counts {
            lines: lines.len() as u64,
            lines_covered: lines.values().filter(|&&hit| hit).count() as u64,
            branches: (branches.len() as u64).saturating_sub(folded),
            branches_covered: branches.values().filter(|&&taken| taken).count() as u64,
        };
        files.insert(path.clone(), counts);
    }
    files
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

/// `was` raised by `now`, measured on the same platforms: each of the total
/// and the areas that rose takes its new count, the rest keep theirs, and the
/// per-file counts are `now`'s. `None` when nothing rose. Also says what rose.
fn ratchet(was: &Baseline, now: &Baseline) -> Option<(Baseline, Vec<String>)> {
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
    Some((
        Baseline {
            hosts: now.hosts.clone(),
            files: now.files.clone(),
            areas,
            total,
        },
        rose,
    ))
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
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some("crates"), Some(_), Some("src"))
    )
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
    files
        .keys()
        .filter(|path| area_of(path).is_none())
        .cloned()
        .collect()
}

/// One file's lines and branches, and how many of each the tests took.
#[derive(Clone, Copy, Default)]
struct Counts {
    lines: u64,
    lines_covered: u64,
    branches: u64,
    branches_covered: u64,
}

impl Counts {
    fn add(&mut self, other: &Counts) {
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
fn summary(hosts: &BTreeSet<String>, files: &BTreeMap<String, Counts>) -> String {
    let mut areas: BTreeMap<&str, Counts> = AREAS
        .iter()
        .map(|&(_, area)| (area, Counts::default()))
        .collect();
    let mut total = Counts::default();
    for (path, counts) in files {
        if let Some(area) = area_of(path) {
            areas.entry(area).or_default().add(counts);
        }
        total.add(counts);
    }
    let mut out = format!(
        "# The compiler's coverage from end-to-end tests only (coverage/compiler/src/main.rs),\n\
         # merged from {}.\n\
         # Branches gate, in total and per area; lines are context.\n\n",
        list(hosts)
    );
    out.push_str(&format!(
        "{:<24} {:>17} {:>8} {:>17} {:>8}\n",
        "area", "branches", "", "lines", ""
    ));
    let row = |name: &str, c: &Counts| {
        format!(
            "{name:<24} {:>17} {:>8} {:>17} {:>8}\n",
            format!("{}/{}", c.branches_covered, c.branches),
            percent(c.branches_covered, c.branches),
            format!("{}/{}", c.lines_covered, c.lines),
            percent(c.lines_covered, c.lines),
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
        "\n{:<24} {:>17} {:>8} {:>17} {:>8}\n",
        "cli/src", "branches", "", "lines", ""
    ));
    for (name, counts) in &modules {
        out.push_str(&row(name, counts));
    }

    let mut worst: Vec<(&String, &Counts)> = files.iter().collect();
    worst.sort_by_key(|(path, c)| {
        (
            std::cmp::Reverse(c.branches - c.branches_covered),
            path.as_str(),
        )
    });
    out.push_str(&format!(
        "\nThe {WORST} files with the most uncovered branches:\n\n"
    ));
    out.push_str(&format!(
        "{:>9} {:>17} {:>8}  file\n",
        "uncovered", "branches", ""
    ));
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

/// `compiler-branches.txt`: the platforms merged, the total's and each area's covered and
/// total branches, then each file's.
#[derive(Clone)]
struct Baseline {
    /// Host triples. A baseline from before platforms merged names one, as `host`.
    hosts: BTreeSet<String>,
    total: (u64, u64),
    areas: BTreeMap<String, (u64, u64)>,
    files: BTreeMap<String, (u64, u64)>,
}

impl Baseline {
    /// A measurement's baseline. Every area is in it, with no branches if no file is.
    fn of(hosts: &BTreeSet<String>, measured: &BTreeMap<String, Counts>) -> Baseline {
        let files: BTreeMap<String, (u64, u64)> = measured
            .iter()
            .map(|(p, c)| (p.clone(), (c.branches_covered, c.branches)))
            .collect();
        let mut areas: BTreeMap<String, (u64, u64)> = AREAS
            .iter()
            .map(|&(_, area)| (area.to_string(), (0, 0)))
            .collect();
        for (path, &(covered, total)) in &files {
            if let Some(counts) = area_of(path).and_then(|area| areas.get_mut(area)) {
                counts.0 += covered;
                counts.1 += total;
            }
        }
        Baseline {
            hosts: hosts.clone(),
            total: sum(files.values()),
            areas,
            files,
        }
    }

    fn parse(text: &str) -> Baseline {
        let mut hosts = BTreeSet::new();
        let mut total = None;
        let mut areas = BTreeMap::new();
        let mut files = BTreeMap::new();
        let counts =
            |covered: &str, total: &str| Some((covered.parse().ok()?, total.parse().ok()?));
        for line in text
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        {
            let words: Vec<&str> = line.split(' ').collect();
            match words.as_slice() {
                ["host" | "hosts", names @ ..] => {
                    hosts = names.iter().map(|h| h.trim().to_string()).collect()
                }
                ["total", c, t, ..] => total = counts(c, t),
                ["area", area, c, t, ..] => {
                    if let Some(counts) = counts(c, t) {
                        areas.insert(area.to_string(), counts);
                    }
                }
                _ => {
                    let mut words = line.splitn(3, ' ');
                    let (Some(c), Some(t), Some(path)) = (words.next(), words.next(), words.next())
                    else {
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
        Baseline {
            hosts,
            total,
            areas,
            files,
        }
    }

    fn render(&self) -> String {
        let mut out = format!(
            "# Branches covered by end-to-end tests: covered, total, then what. The\n\
             # gate holds the total and each area to their share; files only warn.\n\
             # CI's coverage job raises it on main; never edit a count by hand.\n\
             # coverage/compiler/src/main.rs has the rest.\n\
             hosts {}\n\
             total {} {} {}\n",
            self.hosts
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(" "),
            self.total.0,
            self.total.1,
            percent(self.total.0, self.total.1)
        );
        for (area, &(covered, total)) in &self.areas {
            out.push_str(&format!(
                "area {area} {covered} {total} {}\n",
                percent(covered, total)
            ));
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
    let mut report = format!(
        "branch coverage: {} in the baseline, {} now\n",
        share(baseline.total),
        share(now.total)
    );
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
        fell.push(format!(
            "the total: {} -> {}",
            share(baseline.total),
            share(now.total)
        ));
    }
    for (area, &counts) in &now.areas {
        let Some(&was) = baseline.areas.get(area) else {
            continue;
        };
        if falls(was, counts) {
            fell.push(format!("{area}: {} -> {}", share(was), share(counts)));
            for (_, line) in lost
                .iter()
                .filter(|(path, _)| area_of(path) == Some(area.as_str()))
            {
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
            lost.iter()
                .map(|(_, line)| line.as_str())
                .collect::<Vec<_>>()
                .join("\n  ")
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

/// rustc's host triple, and the directory its `llvm-profdata` and `llvm-cov`
/// are in.
fn toolchain() -> (String, PathBuf) {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let version = output(Command::new(&rustc).arg("-vV"));
    let host = version
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .expect("rustc -vV names a host")
        .to_string();
    let sysroot = output(Command::new(&rustc).args(["--print", "sysroot"]));
    let tools = Path::new(sysroot.trim())
        .join("lib/rustlib")
        .join(&host)
        .join("bin");
    (host, tools)
}

fn run(command: &mut Command) {
    let status = command
        .status()
        .unwrap_or_else(|e| panic!("{command:?} didn't start: {e}"));
    assert!(status.success(), "{command:?} failed: {status}");
}

fn output(command: &mut Command) -> String {
    let out = command
        .output()
        .unwrap_or_else(|e| panic!("{command:?} didn't start: {e}"));
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
                (
                    path.to_string(),
                    Counts {
                        branches: total,
                        branches_covered: covered,
                        ..Counts::default()
                    },
                )
            })
            .collect();
        Baseline::of(&BTreeSet::from(["h".to_string()]), &measured)
    }

    #[test]
    fn a_lower_total_share_fails_however_small() {
        // 66.67% either way, to two places, but 666_666/1_000_000 is under 2/3.
        let was = baseline(&[(SYNTAX, 2, 3)]);
        let verdict = compare(&was, &baseline(&[(SYNTAX, 666_666, 1_000_000)]));
        assert!(verdict.failed, "{}", verdict.report);
        assert!(
            verdict.report.contains("the total: 66.67% (2/3) -> 66.67%"),
            "{}",
            verdict.report
        );
    }

    #[test]
    fn an_area_falling_fails_though_the_total_rises() {
        let was = baseline(&[
            (SYNTAX, 5, 10),
            ("crates/syntax/src/parser.rs", 5, 10),
            (LINT, 0, 10),
        ]);
        let now = baseline(&[
            (SYNTAX, 4, 10),
            ("crates/syntax/src/parser.rs", 5, 10),
            (LINT, 9, 10),
        ]);
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
        let verdict = compare(
            &was,
            &baseline(&[(SYNTAX, 3, 5), ("crates/syntax/src/b.rs", 3, 5)]),
        );
        assert!(!verdict.failed, "{}", verdict.report);
        assert!(
            verdict.report.contains("warning: 1 file(s)"),
            "{}",
            verdict.report
        );
        assert!(
            verdict.report.contains("lexer.rs: 6 -> 3"),
            "{}",
            verdict.report
        );
    }

    #[test]
    fn a_gain_passes_and_names_what_rose() {
        let was = baseline(&[(SYNTAX, 5, 10), (LINT, 5, 10)]);
        let verdict = compare(&was, &baseline(&[(SYNTAX, 6, 10), (LINT, 5, 10)]));
        assert!(!verdict.failed);
        assert!(
            verdict
                .report
                .contains("rose: the total, crates/syntax. On main"),
            "{}",
            verdict.report
        );
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
        assert_eq!(
            (raised.total, raised.areas["cli/lint"]),
            ((14, 20), (9, 10))
        );
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
        assert_eq!(again.hosts, was.hosts);
        assert_eq!(again.total, (7, 9));
        assert_eq!(again.areas, was.areas);
        assert_eq!(again.files, was.files);
        assert!(
            was.render().contains("\narea cli/other 1 2 50.00%\n"),
            "{}",
            was.render()
        );
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
        let prefixes: std::collections::BTreeSet<&str> =
            AREAS.iter().map(|&(prefix, _)| prefix).collect();
        assert_eq!(prefixes.len(), AREAS.len(), "two areas share a prefix");
    }

    #[test]
    fn a_file_in_no_area_is_named() {
        let files: BTreeMap<String, Counts> = [("crates/new/src/lib.rs", 1, 2), (SYNTAX, 1, 2)]
            .into_iter()
            .map(|(p, c, t)| {
                (
                    p.to_string(),
                    Counts {
                        branches: t,
                        branches_covered: c,
                        ..Counts::default()
                    },
                )
            })
            .collect();
        assert_eq!(unmapped(&files), ["crates/new/src/lib.rs"]);
    }

    #[test]
    fn every_file_in_the_baseline_is_in_an_area() {
        let checked_in = Baseline::parse(include_str!("../../compiler-branches.txt"));
        let missing: Vec<&String> = checked_in
            .files
            .keys()
            .filter(|path| area_of(path).is_none())
            .collect();
        assert!(missing.is_empty(), "{missing:?}");
    }

    #[test]
    fn an_export_counts_as_llvm_cov_report_does() {
        // `pick<T>` at two types, each taking one side of line 2's `if`; a guard
        // on `A | B` (line 7), which compiles to one condition per alternative;
        // and a file in no counted source.
        let export = r#"{"data": [{"functions": [
            {"filenames": ["/r/crates/x/src/a.rs"], "regions": [[1, 1, 3, 2, 1, 0, 0, 0]],
             "branches": [[2, 8, 2, 12, 1, 0, 0, 0, 4]]},
            {"filenames": ["/r/crates/x/src/a.rs"], "regions": [[1, 1, 3, 2, 1, 0, 0, 0]],
             "branches": [[2, 8, 2, 12, 0, 1, 0, 0, 4]]},
            {"filenames": ["/r/crates/x/src/a.rs", "/r/cli/tests/t.rs"], "regions": [[6, 1, 9, 2, 1, 0, 0, 0]],
             "branches": [[7, 20, 7, 25, 1, 0, 0, 0, 4], [7, 20, 7, 25, 0, 0, 0, 0, 4], [1, 1, 1, 2, 1, 1, 1, 0, 4]]}
          ],
          "files": [
            {"filename": "/r/crates/x/src/a.rs", "summary": {"branches": {"count": 5}},
             "segments": [[1, 1, 2, 1, 1, 0], [2, 8, 0, 1, 1, 0], [3, 2, 2, 1, 0, 0], [4, 1, 0, 0, 1, 0], [5, 1, 0, 0, 0, 0]]},
            {"filename": "/r/cli/tests/t.rs", "summary": {"branches": {"count": 2}}, "segments": []}
          ]}]}"#;
        let export = json::parse(export).unwrap();
        let measured = Measured::of_export(&export, |p| {
            Some(p.strip_prefix("/r/").unwrap().to_string()).filter(|p| counted(p))
        });
        assert_eq!(
            measured.files.keys().collect::<Vec<_>>(),
            ["crates/x/src/a.rs"]
        );
        let a = &measured.files["crates/x/src/a.rs"];
        // Six sides, one of them folded: `report` counts five.
        assert_eq!((a.branches.len(), a.folded), (6, 1));
        // Line 4 starts a skipped region, and line 5 only ends one.
        assert_eq!(a.lines, BTreeMap::from([(1, 2), (2, 2), (3, 0)]));
        let counts = &merge(&BTreeMap::from([("h".to_string(), measured)]))["crates/x/src/a.rs"];
        assert_eq!((counts.branches_covered, counts.branches), (3, 5));
    }

    #[test]
    fn a_path_through_a_parent_directory_is_where_it_leads() {
        assert_eq!(
            lexical("cli/src/../runtime/allocator.rs"),
            "cli/runtime/allocator.rs"
        );
        assert!(!counted(&lexical("cli/src/../runtime/allocator.rs")));
        assert_eq!(lexical("/r/./a/../../b.rs"), "/b.rs");
        assert_eq!(lexical("../x.rs"), "../x.rs");
    }

    #[test]
    fn the_cli_is_broken_down_by_command_and_module() {
        assert_eq!(
            module_of("cli/src/commands/lint.rs").as_deref(),
            Some("commands/lint")
        );
        assert_eq!(
            module_of("cli/src/commands/test/runner.rs").as_deref(),
            Some("commands/test")
        );
        assert_eq!(
            module_of("cli/src/language_server/mod.rs").as_deref(),
            Some("language_server")
        );
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

    /// A program built and profiled as `buri` is, which echoes each line it
    /// reads and says `done` at the end of its input.
    const ECHO: &str = r#"use std::io::{BufRead, Write};

fn main() {
    let mut out = std::io::stdout();
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        if line == "even" {
            writeln!(out, "took even").unwrap();
        } else {
            writeln!(out, "took odd").unwrap();
        }
        out.flush().unwrap();
    }
    writeln!(out, "done").unwrap();
}
"#;

    /// In continuous mode a killed process's counts said it ran what it never
    /// reached, and some came out below zero (CI run 39423043418).
    #[test]
    fn a_process_a_signal_kills_covers_nothing_it_never_reached() {
        use std::io::{BufRead as _, Write as _};
        let dir = std::env::temp_dir().join(format!("buri-coverage-killed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let profiles = dir.join("profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(dir.join("echo.rs"), ECHO).unwrap();
        let program = dir.join("echo");
        let (_, tools) = toolchain();
        run(
            Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string()))
                .args(["--edition", "2021"])
                .args(RUSTFLAGS.split_whitespace())
                .arg(dir.join("echo.rs"))
                .arg("-o")
                .arg(&program)
                // `-Z` on the workspace's stable toolchain, as the nightly takes it.
                .env("RUSTC_BOOTSTRAP", "1"),
        );
        let start = || {
            Command::new(&program)
                .env(
                    "LLVM_PROFILE_FILE",
                    profiles.join(format!("{POOL}.profraw")),
                )
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap()
        };

        let mut finished = start();
        finished
            .stdin
            .take()
            .unwrap()
            .write_all(b"even\nodd\n")
            .unwrap();
        assert!(finished.wait().unwrap().success());

        // Killed while it waits for its second line.
        let mut killed = start();
        let mut input = killed.stdin.take().unwrap();
        input.write_all(b"even\n").unwrap();
        let mut echoed = String::new();
        std::io::BufReader::new(killed.stdout.take().unwrap())
            .read_line(&mut echoed)
            .unwrap();
        assert_eq!(echoed, "took even\n");
        killed.kill().unwrap();
        let _ = killed.wait();
        drop(input);

        let profdata = dir.join("echo.profdata");
        run(Command::new(tools.join("llvm-profdata"))
            .args(["merge", "-sparse", "-o"])
            .arg(&profdata)
            .args(
                std::fs::read_dir(&profiles)
                    .unwrap()
                    .map(|e| e.unwrap().path()),
            ));
        let text = output(
            Command::new(tools.join("llvm-cov"))
                .args(["export", "-format=text", "--instr-profile"])
                .arg(&profdata)
                .arg(&program),
        );
        let measured = Measured::of_export(&json::parse(&text).unwrap(), |path| {
            path.ends_with("echo.rs").then(|| "echo.rs".to_string())
        });
        let echo = &measured.files["echo.rs"];
        // Only the finished run said `done`.
        assert_eq!(echo.lines.get(&13), Some(&1), "{:?}", echo.lines);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
