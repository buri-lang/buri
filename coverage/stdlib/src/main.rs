//! The standard library's MC/DC gate: the conformance corpus under
//! `buri test --coverage=mcdc` on four backends, merged, and held to
//! `coverage/stdlib-mcdc.txt`.
//!
//! ```text
//! cargo run -p buri-stdlib-coverage              # measure and check
//! cargo run -p buri-stdlib-coverage -- --bless   # and rewrite the baseline
//! cargo run -p buri-stdlib-coverage -- --raise   # CI: a rise becomes the baseline
//! ```
//!
//! `BURI_COVERAGE_STD=1` is what makes `buri` count the standard library as
//! if it were the repository's own source. The platforms stay out, as they do
//! for users.

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
use std::process::Command;

/// Each backend the corpus runs on: its name, whether its suites are moved to
/// the native backends, and its flags. LLVM builds only under `--release`, so
/// it has no debug run.
const RUNS: [(&str, bool, &[&str]); 4] = [
    ("javascript", false, &[]),
    ("javascript --release", false, &["--release"]),
    ("stencil", true, &[]),
    ("llvm --release", true, &["--release"]),
];

/// The corpus's packages the native backends can't build yet, which stay on
/// JavaScript in every run: `deriveArrayHash`, `core/math`'s transcendentals
/// and a page's `mount` (`cli/tests/native/conformance.rs` has why).
const JAVASCRIPT_ONLY: [&str; 6] = ["bignum", "numbers", "random", "ui_mount", "uuid", "web"];

/// Pairs of runs that run the same suites, so count alike.
const ALIKE: [(usize, usize); 2] = [(0, 1), (2, 3)];

/// How many modules `stdlib.txt` lists as having the most unshown conditions.
const WORST: usize = 30;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let gate = Gate::new();
    if args.iter().any(|a| a == "--raise") {
        gate.raise();
        return;
    }
    let bless = args.iter().any(|a| a == "--bless");

    let buri = gate.build();
    let mut runs = Vec::new();
    for (name, native, flags) in RUNS {
        runs.push((name, gate.measure(&buri, name, native, flags)));
    }
    let disagreements: Vec<String> = ALIKE.iter().flat_map(|&(a, b)| disagree(&runs[a], &runs[b])).collect();
    let merged = merge(runs.iter().map(|(_, m)| m));
    let now = Baseline::of(&gate.host, &merged);

    let baseline_path = gate.root.join("coverage").join("stdlib-mcdc.txt");
    let summary_path = gate.root.join("coverage").join("stdlib.txt");
    std::fs::write(gate.target.join("stdlib-mcdc.txt"), now.render()).unwrap();
    std::fs::write(gate.target.join("stdlib.txt"), summary(&merged)).unwrap();
    print!("{}", summary(&merged));
    if !disagreements.is_empty() {
        eprintln!("\nThe backends counted differently, which is a coverage bug:\n  {}", disagreements.join("\n  "));
        std::process::exit(1);
    }

    let baseline = Baseline::parse(&std::fs::read_to_string(&baseline_path).unwrap_or_default());
    if bless {
        if !baseline.host.is_empty() && baseline.host != gate.host {
            eprintln!(
                "the baseline was measured on {}, and this is {}. Bless on {} or commit the CI \
                 run's artifact.",
                baseline.host, gate.host, baseline.host
            );
            std::process::exit(2);
        }
        std::fs::write(&baseline_path, now.render()).unwrap();
        std::fs::write(&summary_path, summary(&merged)).unwrap();
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
    /// `target/stdlib-coverage`: the corpus copies and what they measured.
    target: PathBuf,
    /// rustc's host triple.
    host: String,
}

impl Gate {
    fn new() -> Gate {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
        let target = root.join("target").join("stdlib-coverage");
        std::fs::create_dir_all(&target).unwrap();
        let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
        let version = output(Command::new(rustc).arg("-vV"));
        let host = version
            .lines()
            .find_map(|l| l.strip_prefix("host: "))
            .expect("rustc -vV names a host")
            .to_string();
        Gate { root, target, host }
    }

    /// The ratchet: replaces the checked-in baseline and summary with the last
    /// measurement's when a module or the total shows more of its conditions.
    fn raise(&self) {
        let checked_in = self.root.join("coverage").join("stdlib-mcdc.txt");
        let measured = self.target.join("stdlib-mcdc.txt");
        let was = Baseline::parse(&std::fs::read_to_string(&checked_in).unwrap_or_default());
        let now = Baseline::parse(&std::fs::read_to_string(&measured).unwrap_or_else(|e| {
            panic!("{} can't be read ({e}); measure first", measured.display())
        }));
        if compare(&was, &now).failed || !rises(&was, &now) {
            println!("MC/DC didn't rise above {}; the baseline stays", percent(was.total()));
            return;
        }
        std::fs::copy(&measured, &checked_in).unwrap();
        std::fs::copy(self.target.join("stdlib.txt"), self.root.join("coverage").join("stdlib.txt")).unwrap();
        println!("raised the baseline from {} to {}", percent(was.total()), percent(now.total()));
    }

    /// A `buri` that can build for every backend.
    fn build(&self) -> PathBuf {
        run(self.cargo().args(["build", "-p", "buri", "--features", "backend-llvm", "--profile", "validate"]));
        self.root.join("target").join("validate").join("buri")
    }

    /// The conformance corpus under `--coverage=mcdc` with `flags`, in a copy
    /// of its own. Every suite runs on JavaScript, as the corpus says, or with
    /// `native`, on the native backends where they can build it.
    fn measure(&self, buri: &Path, name: &str, native: bool, flags: &[&str]) -> Modules {
        let repo = self.target.join(name.replace([' ', '-'], ""));
        let _ = std::fs::remove_dir_all(&repo);
        copy(&self.root.join("cli/tests/conformance"), &repo);
        if native {
            for package in std::fs::read_dir(repo.join("lib")).unwrap() {
                let package = package.unwrap();
                if JAVASCRIPT_ONLY.iter().any(|p| package.file_name() == *p) {
                    continue;
                }
                let build = package.path().join("BUILD.buri");
                let text = std::fs::read_to_string(&build).unwrap();
                std::fs::write(&build, text.replace("backends: [JS]", "backends: [NATIVE]")).unwrap();
            }
        }
        let started = std::time::Instant::now();
        let status = Command::new(buri)
            .current_dir(&repo)
            .args(["test", "//...", "--coverage=mcdc"])
            .args(flags)
            .env("BURI_COVERAGE_STD", "1")
            .status()
            .expect("buri runs");
        eprintln!("{name}: {:.0?}", started.elapsed());
        if !status.success() {
            eprintln!("the conformance corpus failed on {name}, so its coverage can't count");
            std::process::exit(1);
        }
        let lcov = std::fs::read_to_string(repo.join(".buri/coverage/lcov.info")).unwrap();
        parse(&lcov)
    }

    /// A `cargo` in the workspace without `cargo run`'s own state.
    fn cargo(&self) -> Command {
        let mut cargo = Command::new("cargo");
        for (name, _) in std::env::vars() {
            if name.starts_with("CARGO_") && name != "CARGO_HOME" && name != "CARGO_BUILD_JOBS" {
                cargo.env_remove(&name);
            }
        }
        cargo.current_dir(&self.root);
        cargo
    }
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == ".buri" {
            continue;
        }
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

/// One module's records: each branch with how often it ran, `None` where its
/// decision was never reached; and each condition, by line, place in the
/// line's group and text, with whether it was shown.
#[derive(Default, PartialEq, Eq, Debug)]
struct Module {
    branches: BTreeMap<(u64, u64, String), Option<u64>>,
    conditions: BTreeMap<(u64, u64, String), bool>,
}

type Modules = BTreeMap<String, Module>;

/// The standard library's modules in an lcov file: those named by module
/// path, as `core/list` is, rather than by a `.buri` file in the corpus.
fn parse(lcov: &str) -> Modules {
    let mut modules = Modules::new();
    let mut current: Option<&mut Module> = None;
    for line in lcov.lines() {
        if let Some(file) = line.strip_prefix("SF:") {
            current = (!file.ends_with(".buri")).then(|| modules.entry(file.to_string()).or_default());
            continue;
        }
        let Some(m) = current.as_mut() else { continue };
        if let Some(rest) = line.strip_prefix("BRDA:") {
            // line,block,branch,taken; the branch may hold commas.
            let (head, taken) = rest.rsplit_once(',').unwrap();
            let mut parts = head.splitn(3, ',');
            let (l, block, branch) = (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap());
            let taken = if taken == "-" { None } else { Some(taken.parse().unwrap()) };
            m.branches.insert((l.parse().unwrap(), block.parse().unwrap(), branch.to_string()), taken);
        } else if let Some(rest) = line.strip_prefix("MCDC:") {
            // line,size,sense,taken,index,expression; both senses alike.
            let parts: Vec<&str> = rest.splitn(6, ',').collect();
            if parts[2] == "t" {
                let key = (parts[0].parse().unwrap(), parts[4].parse().unwrap(), parts[5].to_string());
                m.conditions.insert(key, parts[3] != "0");
            }
        }
    }
    modules
}

/// Where two runs counted a module differently, record by record.
fn disagree((one, a): &(&str, Modules), (two, b): &(&str, Modules)) -> Vec<String> {
    let names: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    names
        .into_iter()
        .filter(|module| a.get(*module) != b.get(*module))
        .map(|module| {
            let (x, y) = (a.get(module).map(Stats::of), b.get(module).map(Stats::of));
            format!("{module}: {one} {} and {two} {}", show(x), show(y))
        })
        .collect()
}

fn show(stats: Option<Stats>) -> String {
    stats.map_or("nothing".to_string(), |s| {
        format!("{}/{} conditions, {}/{} branches", s.shown, s.conditions, s.hit, s.branches)
    })
}

/// Every run's records together: a branch ran if it ran anywhere, and a
/// condition was shown if it was shown anywhere.
fn merge<'a>(runs: impl Iterator<Item = &'a Modules>) -> BTreeMap<String, Stats> {
    let mut all = Modules::new();
    for run in runs {
        for (name, m) in run {
            let into = all.entry(name.clone()).or_default();
            for (key, taken) in &m.branches {
                let n = into.branches.entry(key.clone()).or_insert(None);
                *n = match (*n, *taken) {
                    (Some(a), Some(b)) => Some(a + b),
                    (a, b) => a.or(b),
                };
            }
            for (key, shown) in &m.conditions {
                *into.conditions.entry(key.clone()).or_insert(false) |= *shown;
            }
        }
    }
    all.iter().map(|(name, m)| (name.clone(), Stats::of(m))).collect()
}

/// A module's counts.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct Stats {
    shown: u64,
    conditions: u64,
    decided: u64,
    decisions: u64,
    hit: u64,
    branches: u64,
}

impl Stats {
    fn of(m: &Module) -> Stats {
        let mut decisions: BTreeMap<(u64, &str), bool> = BTreeMap::new();
        for ((line, _, expression), shown) in &m.conditions {
            // `'c' in 'decision'`, then the instantiation, if any.
            let decision = expression.split_once("' in '").map_or(expression.as_str(), |(_, d)| d);
            *decisions.entry((*line, decision)).or_insert(true) &= *shown;
        }
        Stats {
            shown: m.conditions.values().filter(|s| **s).count() as u64,
            conditions: m.conditions.len() as u64,
            decided: decisions.values().filter(|d| **d).count() as u64,
            decisions: decisions.len() as u64,
            hit: m.branches.values().filter(|t| t.is_some_and(|n| n > 0)).count() as u64,
            branches: m.branches.len() as u64,
        }
    }

    fn add(&mut self, other: &Stats) {
        self.shown += other.shown;
        self.conditions += other.conditions;
        self.decided += other.decided;
        self.decisions += other.decisions;
        self.hit += other.hit;
        self.branches += other.branches;
    }
}

fn percent((covered, total): (u64, u64)) -> String {
    if total == 0 {
        "-".to_string()
    } else {
        format!("{:.1}%", covered as f64 * 100.0 / total as f64)
    }
}

fn fraction((covered, total): (u64, u64)) -> String {
    format!("{covered}/{total}")
}

/// `stdlib.txt`: every module's counts, then the ones with the most to show.
fn summary(modules: &BTreeMap<String, Stats>) -> String {
    let mut total = Stats::default();
    for s in modules.values() {
        total.add(s);
    }
    let mut out = String::from(
        "# The standard library's MC/DC from the conformance corpus on four backends\n\
         # (coverage/stdlib/src/main.rs). Conditions gate; decisions and branches are\n\
         # context.\n\n",
    );
    let row = |name: &str, s: &Stats| {
        format!(
            "{name:<28} {:>11} {:>7} {:>11} {:>7} {:>13} {:>7}\n",
            fraction((s.shown, s.conditions)),
            percent((s.shown, s.conditions)),
            fraction((s.decided, s.decisions)),
            percent((s.decided, s.decisions)),
            fraction((s.hit, s.branches)),
            percent((s.hit, s.branches)),
        )
    };
    out.push_str(&format!("{:<28} {:>19} {:>19} {:>21}\n", "module", "conditions", "decisions", "branches"));
    for (name, s) in modules {
        out.push_str(&row(name, s));
    }
    out.push_str(&row("total", &total));

    let mut worst: Vec<(&String, &Stats)> = modules.iter().filter(|(_, s)| s.shown < s.conditions).collect();
    worst.sort_by_key(|(name, s)| (std::cmp::Reverse(s.conditions - s.shown), name.as_str()));
    out.push_str(&format!("\nThe {WORST} modules with the most unshown conditions:\n\n"));
    out.push_str(&format!("{:>8} {:>19}  module\n", "unshown", "conditions"));
    for (name, s) in worst.into_iter().take(WORST) {
        out.push_str(&format!(
            "{:>8} {:>11} {:>7}  {name}\n",
            s.conditions - s.shown,
            fraction((s.shown, s.conditions)),
            percent((s.shown, s.conditions)),
        ));
    }
    out
}

/// `stdlib-mcdc.txt`: the host, then conditions shown and conditions per module.
struct Baseline {
    host: String,
    modules: BTreeMap<String, (u64, u64)>,
}

impl Baseline {
    fn of(host: &str, modules: &BTreeMap<String, Stats>) -> Baseline {
        Baseline {
            host: host.to_string(),
            modules: modules.iter().map(|(n, s)| (n.clone(), (s.shown, s.conditions))).collect(),
        }
    }

    fn parse(text: &str) -> Baseline {
        let mut host = String::new();
        let mut modules = BTreeMap::new();
        for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
            if let Some(h) = line.strip_prefix("host ") {
                host = h.trim().to_string();
                continue;
            }
            let mut words = line.splitn(3, ' ');
            let (Some(shown), Some(total), Some(module)) = (words.next(), words.next(), words.next()) else {
                continue;
            };
            if let (Ok(shown), Ok(total)) = (shown.parse(), total.parse()) {
                modules.insert(module.to_string(), (shown, total));
            }
        }
        Baseline { host, modules }
    }

    fn total(&self) -> (u64, u64) {
        self.modules.values().fold((0, 0), |(s, t), &(ms, mt)| (s + ms, t + mt))
    }

    fn render(&self) -> String {
        let total = self.total();
        let mut out = format!(
            "# The standard library's MC/DC conditions shown, per module: shown, total,\n\
             # module. CI's stdlib-coverage job raises it on main; never edit a count\n\
             # by hand. coverage/stdlib/src/main.rs has the rest.\n\
             # total {} {} ({})\n\
             host {}\n",
            total.0,
            total.1,
            percent(total),
            self.host
        );
        for (module, (shown, total)) in &self.modules {
            out.push_str(&format!("{shown} {total} {module}\n"));
        }
        out
    }
}

/// `a` against `b` as fractions, exactly: less, equal or greater.
fn cmp((a, at): (u64, u64), (b, bt): (u64, u64)) -> std::cmp::Ordering {
    (u128::from(a) * u128::from(bt)).cmp(&(u128::from(b) * u128::from(at)))
}

/// Whether a module or the total shows a larger share than the baseline did.
fn rises(was: &Baseline, now: &Baseline) -> bool {
    was.host != now.host
        || cmp(now.total(), was.total()).is_gt()
        || now.modules.iter().any(|(m, &s)| {
            was.modules.get(m).is_some_and(|&w| w.1 > 0 && s.1 > 0 && cmp(s, w).is_gt())
        })
}

struct Verdict {
    report: String,
    failed: bool,
}

/// The total and every module that has conditions in both: a smaller share of
/// them shown fails. A module that's new, or has none, has nothing to fall from.
fn compare(baseline: &Baseline, now: &Baseline) -> Verdict {
    let mut report = format!(
        "MC/DC: {} ({}) in the baseline, {} ({}) now\n",
        percent(baseline.total()),
        fraction(baseline.total()),
        percent(now.total()),
        fraction(now.total())
    );
    let mut fell = Vec::new();
    if baseline.total().1 > 0 && cmp(now.total(), baseline.total()).is_lt() {
        fell.push("the total".to_string());
    }
    for (module, &s) in &now.modules {
        let Some(&w) = baseline.modules.get(module) else { continue };
        if w.1 > 0 && s.1 > 0 && cmp(s, w).is_lt() {
            fell.push(format!("{module}: {} ({}) -> {} ({})", percent(w), fraction(w), percent(s), fraction(s)));
        }
    }
    let failed = !fell.is_empty();
    if failed {
        report.push_str(&format!(
            "\nMC/DC fell:\n  {}\n\nAdd conformance tests that show the new or unshown \
             conditions deciding. Never lower the baseline to pass.\n",
            fell.join("\n  ")
        ));
    } else if rises(baseline, now) {
        report.push_str("\nMC/DC rose. On main, CI raises the baseline to this run's numbers itself.\n");
    }
    Verdict { report, failed }
}

fn run(command: &mut Command) {
    let status = command.status().unwrap_or_else(|e| panic!("{command:?} didn't start: {e}"));
    assert!(status.success(), "{command:?} failed: {status}");
}

fn output(command: &mut Command) -> String {
    let out = command.output().unwrap_or_else(|e| panic!("{command:?} didn't start: {e}"));
    assert!(out.status.success(), "{command:?} failed: {}", out.status);
    String::from_utf8(out.stdout).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline(modules: &[(&str, u64, u64)]) -> Baseline {
        Baseline {
            host: "h".to_string(),
            modules: modules.iter().map(|(m, s, t)| (m.to_string(), (*s, *t))).collect(),
        }
    }

    #[test]
    fn a_module_showing_a_smaller_share_fails() {
        let was = baseline(&[("core/list", 3, 4), ("core/map", 1, 4)]);
        let now = baseline(&[("core/list", 2, 4), ("core/map", 3, 4)]);
        assert!(compare(&was, &now).failed);
    }

    #[test]
    fn a_new_or_vanished_module_has_nothing_to_fall_from() {
        let was = baseline(&[("core/list", 3, 4), ("core/gone", 1, 4)]);
        let now = baseline(&[("core/list", 3, 4), ("core/new", 2, 2)]);
        assert!(!compare(&was, &now).failed);
    }

    #[test]
    fn a_new_module_that_lowers_the_total_fails() {
        let was = baseline(&[("core/list", 3, 4)]);
        let now = baseline(&[("core/list", 3, 4), ("core/new", 0, 9)]);
        assert!(compare(&was, &now).failed);
    }

    #[test]
    fn only_a_larger_share_raises_the_baseline() {
        let was = baseline(&[("core/list", 3, 4)]);
        assert!(!rises(&was, &baseline(&[("core/list", 6, 8)])));
        assert!(rises(&was, &baseline(&[("core/list", 4, 4)])));
    }

    #[test]
    fn records_merge_across_runs_and_disagreements_are_named() {
        let a = parse("SF:core/list\nBRDA:3,0,0,1\nBRDA:3,0,1,0\nMCDC:3,2,t,1,0,'a' in 'a && b'\nMCDC:3,2,f,1,0,'a' in 'a && b'\nMCDC:3,2,t,0,1,'b' in 'a && b'\nend_of_record\nSF:lib/x.buri\nBRDA:1,0,0,1\n");
        let b = parse("SF:core/list\nBRDA:3,0,0,0\nBRDA:3,0,1,2\nMCDC:3,2,t,0,0,'a' in 'a && b'\nMCDC:3,2,t,1,1,'b' in 'a && b'\n");
        let merged = merge([&a, &b].into_iter());
        let list = merged.get("core/list").unwrap();
        assert_eq!((list.hit, list.branches, list.shown, list.conditions, list.decided, list.decisions), (2, 2, 2, 2, 1, 1));
        assert!(!merged.contains_key("lib/x.buri"));
        assert_eq!(disagree(&("one", a), &("two", b)).len(), 1);
    }
}
