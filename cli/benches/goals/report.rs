//! What `--goals` reads and prints: the `BURI_PROFILE` report of one build,
//! callgrind's totals, the goals table, and the never-lower baseline
//! (`design/PERFORMANCE.md` §6.82).
//!
//! No file system and no processes, so the suite can feed it a run of its own
//! (`cli/tests/build/goals.rs`).

use std::collections::BTreeMap;

/// The simulated caches, pinned so a figure doesn't depend on the runner:
/// 32 KiB 8-way L1s and an 8 MiB 16-way last level, 64-byte lines.
pub const CACHES: [&str; 3] = ["--I1=32768,8,64", "--D1=32768,8,64", "--LL=8388608,16,64"];

/// How much a per-line figure may grow over its baseline before the gate fails.
pub const MARGIN: f64 = 0.01;

/// One `BURI_PROFILE=1` report.
#[derive(Default, Debug)]
pub struct Profile {
    /// Per phase row: instructions, CPU seconds, child instructions, child CPU seconds.
    pub rows: BTreeMap<String, [Option<f64>; 4]>,
    /// Lines of code lexed, each file once.
    pub lines: Option<u64>,
    /// The whole process's instructions and CPU seconds.
    pub process: Option<(f64, f64)>,
    /// CPU seconds of every child process.
    pub child_cpu: Option<f64>,
}

impl Profile {
    pub fn read(text: &str) -> Profile {
        let mut p = Profile::default();
        let mut table = false;
        for line in text.lines() {
            if line.starts_with("buri profile") {
                table = true;
                continue;
            }
            if let Some(rest) = line.strip_prefix("source: ") {
                p.lines = rest.split_whitespace().next().and_then(|n| n.parse().ok());
            } else if let Some(rest) = line.strip_prefix("process: ") {
                let numbers = numbers(rest);
                if let [(Some(instructions), "B"), (Some(cpu), "s"), ..] = numbers[..] {
                    p.process = Some((instructions * 1e9, cpu));
                }
            } else if let Some(rest) = line.strip_prefix("child processes: ") {
                p.child_cpu = numbers(rest).first().and_then(|(v, _)| *v);
            } else if table && line.starts_with("  ") && line.len() > 16 {
                let (name, rest) = line.split_at(16);
                let pairs = numbers(rest);
                let mut m = pairs
                    .iter()
                    .filter(|(_, u)| *u == "M")
                    .map(|(v, _)| v.map(|v| v * 1e6));
                let mut s = pairs.iter().filter(|(_, u)| *u == "s").map(|(v, _)| *v);
                let instructions = m.next().flatten();
                let cpu = s.next().flatten();
                let _busy = s.next();
                let row = [instructions, cpu, m.next().flatten(), s.next().flatten()];
                p.rows.insert(name.trim().to_string(), row);
            } else {
                table = false;
            }
        }
        p
    }

    fn row(&self, name: &str, at: usize) -> f64 {
        self.rows.get(name).and_then(|r| r[at]).unwrap_or(0.0)
    }

    /// Parse, check, other and dev codegen: CPU seconds and, where this
    /// machine counts them, instructions. Dev codegen is the rest of the
    /// process and its children, the linker among them. The processes the
    /// `run` row waited for are left out, the generators' tools and the tests
    /// of a `buri test`, and the work around them counts as other.
    pub fn phases(&self) -> [(f64, Option<f64>); 4] {
        let counted = self.rows.values().any(|r| r[0].is_some());
        let own = |name: &str| (self.row(name, 1), counted.then(|| self.row(name, 0)));
        let [parse, check] = [own("lex+parse"), own("check")];
        let (other, run) = (own("other"), own("run"));
        let other = (other.0 + run.0, other.1.zip(run.1).map(|(a, b)| a + b));
        let children = |at: usize| {
            self.rows
                .iter()
                .filter(|(n, _)| *n != "all phases" && *n != "run")
                .map(|(_, r)| r[at].unwrap_or(0.0))
                .sum::<f64>()
        };
        let (cpu, instructions) = match self.process {
            Some((instructions, cpu)) => (
                cpu + self.child_cpu.unwrap_or(0.0) - self.row("run", 3),
                counted.then(|| instructions + children(2)),
            ),
            None => (
                self.row("all phases", 1) + children(3),
                counted.then(|| self.row("all phases", 0) + children(2)),
            ),
        };
        let less = |whole: Option<f64>, f: fn(&(f64, Option<f64>)) -> Option<f64>| {
            whole.map(|w| w - [parse, check, other].iter().filter_map(f).sum::<f64>())
        };
        let codegen = (
            less(Some(cpu), |p| Some(p.0)).unwrap_or(0.0),
            less(instructions, |p| p.1),
        );
        [parse, check, other, codegen]
    }
}

/// `1.5 M`, `0.2 s`, `-` and bare numbers, in order: each a value and its unit.
fn numbers(text: &str) -> Vec<(Option<f64>, &str)> {
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| !w.is_empty())
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let value = if words[i] == "-" {
            None
        } else {
            words[i].parse::<f64>().ok()
        };
        if value.is_none() && words[i] != "-" {
            i += 1;
            continue;
        }
        let unit = words
            .get(i + 1)
            .copied()
            .filter(|u| matches!(*u, "M" | "B" | "s"));
        // A bare `-` is a count this machine didn't take, in millions.
        let unit = unit.unwrap_or(if words[i] == "-" { "M" } else { "" });
        out.push((value, unit));
        i += if words.get(i + 1) == Some(&unit) {
            2
        } else {
            1
        };
    }
    out
}

/// Callgrind's event totals for one process, or the sum of several.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Events {
    pub instructions: f64,
    /// L1 instruction-read, data-read and data-write misses.
    pub l1_misses: f64,
    /// The same at the last level.
    pub ll_misses: f64,
}

impl Events {
    /// The `totals:` line of a callgrind output file, against its `events:`
    /// line. A process that never counted has none, and counts nothing.
    pub fn read(text: &str) -> Result<Events, String> {
        let names: Vec<&str> = text
            .lines()
            .find_map(|l| l.strip_prefix("events:"))
            .ok_or("no `events:` line: not a callgrind output file")?
            .split_whitespace()
            .collect();
        let Some(totals) = text.lines().find_map(|l| {
            l.strip_prefix("totals:")
                .or_else(|| l.strip_prefix("summary:"))
        }) else {
            return Ok(Events::default());
        };
        let value = |name: &str| -> f64 {
            names
                .iter()
                .position(|n| *n == name)
                .and_then(|i| totals.split_whitespace().nth(i))
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0)
        };
        if !names.contains(&"D1mr") {
            return Err("no cache events: run callgrind with `--cache-sim=yes`".into());
        }
        Ok(Events {
            instructions: value("Ir"),
            l1_misses: value("I1mr") + value("D1mr") + value("D1mw"),
            ll_misses: value("ILmr") + value("DLmr") + value("DLmw"),
        })
    }

    /// Instructions + 10 × L1 misses + 100 × last-level misses.
    pub fn cost(&self) -> f64 {
        self.instructions + 10.0 * self.l1_misses + 100.0 * self.ll_misses
    }

    pub fn plus(self, o: Events) -> Events {
        Events {
            instructions: self.instructions + o.instructions,
            l1_misses: self.l1_misses + o.l1_misses,
            ll_misses: self.ll_misses + o.ll_misses,
        }
    }

    pub fn minus(self, o: Events) -> Events {
        Events {
            instructions: self.instructions - o.instructions,
            l1_misses: self.l1_misses - o.l1_misses,
            ll_misses: self.ll_misses - o.ll_misses,
        }
    }
}

/// One row of the table.
#[derive(Clone, Debug)]
pub struct Row {
    pub phase: &'static str,
    /// Lines a CPU second.
    pub goal: Option<f64>,
    pub cpu: Option<f64>,
    pub instructions: Option<f64>,
    /// [`Events::cost`].
    pub cost: Option<f64>,
}

/// The three goal phases, in the order the table prints them.
pub const PHASES: [&str; 3] = ["parse", "check", "dev codegen"];

/// A phase's CPU seconds and instructions, each where measured.
pub type Figure = (Option<f64>, Option<f64>);

/// What one `--goals` run found.
pub struct Measured {
    pub size: String,
    /// Whether this is what `buri test //...` builds, not `buri build //...`.
    pub tests: bool,
    pub lines: u64,
    /// The three phases, then `other` and the whole build.
    pub rows: Vec<Row>,
}

impl Measured {
    /// The rows from a profile: parse, check and dev codegen against `goals`,
    /// then other and the whole build. `costs`, where valgrind measured them,
    /// are in the same order.
    pub fn new(
        size: &str,
        lines: u64,
        goals: [f64; 3],
        figures: [Figure; 4],
        costs: Option<[f64; 4]>,
    ) -> Measured {
        let [parse, check, other, codegen] = figures;
        let cost = |i: usize| costs.map(|c| c[i]);
        let sum = |f: fn(&Figure) -> Option<f64>| -> Option<f64> { figures.iter().map(f).sum() };
        Measured {
            size: size.to_string(),
            tests: false,
            lines,
            rows: vec![
                Row {
                    phase: "parse",
                    goal: Some(goals[0]),
                    cpu: parse.0,
                    instructions: parse.1,
                    cost: cost(0),
                },
                Row {
                    phase: "check",
                    goal: Some(goals[1]),
                    cpu: check.0,
                    instructions: check.1,
                    cost: cost(1),
                },
                Row {
                    phase: "dev codegen",
                    goal: Some(goals[2]),
                    cpu: codegen.0,
                    instructions: codegen.1,
                    cost: cost(3),
                },
                Row {
                    phase: "other",
                    goal: None,
                    cpu: other.0,
                    instructions: other.1,
                    cost: cost(2),
                },
                Row {
                    phase: "whole build",
                    goal: None,
                    cpu: sum(|f| f.0),
                    instructions: sum(|f| f.1),
                    cost: costs.map(|c| c.iter().sum()),
                },
            ],
        }
    }

    /// The same figures, for what `buri test //...` builds.
    pub fn of_tests(self) -> Measured {
        Measured {
            tests: true,
            ..self
        }
    }

    /// The table.
    pub fn render(&self) -> String {
        let lines = self.lines as f64;
        let build = if self.tests {
            "what a cold `buri test //...` builds"
        } else {
            "a cold dev `buri build //...`"
        };
        let mut out = format!(
            "compile goals: {build} of the {} workspace, on one thread\n\
             {} lines of code: the workspace's and the stdlib's it reaches, each file once\n\n",
            self.size,
            commas(lines)
        );
        out.push_str(&format!(
            "  {:<12} {:>9} {:>12} {:>9} {:>8} {:>13} {:>11}\n",
            "phase", "CPU", "lines/CPU s", "goal", "of goal", "instr/line", "cost/line"
        ));
        for r in &self.rows {
            let rate = r.cpu.filter(|c| *c > 0.0).map(|c| lines / c);
            let dash = || "-".to_string();
            out.push_str(&format!(
                "  {:<12} {:>9} {:>12} {:>9} {:>8} {:>13} {:>11}\n",
                r.phase,
                r.cpu.map_or_else(dash, |c| format!("{c:.3} s")),
                rate.map_or_else(dash, short),
                r.goal.map_or_else(dash, short),
                rate.zip(r.goal)
                    .map_or_else(dash, |(rate, goal)| format!("{:.0}%", rate / goal * 100.0)),
                r.instructions
                    .map_or_else(dash, |n| format!("{:.1}", n / lines)),
                r.cost.map_or_else(dash, |n| format!("{:.1}", n / lines)),
            ));
        }
        out.push_str(
            "\nother: the loader, lint, styles, the build cache and the test runner around the tests.\n\
             cost: instructions + 10 x L1 misses + 100 x last-level misses, under valgrind's cache\n\
             simulation (`--goals=count`).\n",
        );
        out
    }

    /// Each goal phase's per-line figures, keyed as the baseline keys them.
    pub fn figures(&self, arch: &str) -> BTreeMap<String, f64> {
        let mut out = BTreeMap::new();
        for r in self.rows.iter().filter(|r| PHASES.contains(&r.phase)) {
            let phase = r.phase.replace(' ', "-");
            for (what, n) in [("instructions", r.instructions), ("cost", r.cost)] {
                if let Some(n) = n {
                    let per_line = (n / self.lines as f64 * 10.0).round() / 10.0;
                    let test = if self.tests { " test-build" } else { "" };
                    out.insert(
                        format!("{arch} {}{test} {phase} {what}", self.size),
                        per_line,
                    );
                }
            }
        }
        out
    }
}

/// `12,345`.
pub fn commas(n: f64) -> String {
    let digits = format!("{:.0}", n);
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `10.0 M`, `850.2 k`.
fn short(n: f64) -> String {
    if n >= 1e6 {
        format!("{:.2} M", n / 1e6)
    } else {
        format!("{:.1} k", n / 1e3)
    }
}

/// `cli/benches/goals/baseline.txt`: per architecture, the toolchain, and
/// each size's per-line figures, which only ever fall.
#[derive(Default)]
pub struct Baseline {
    pub toolchains: BTreeMap<String, String>,
    pub figures: BTreeMap<String, f64>,
}

const HEADER: &str = "\
# Per line of code, what each goal phase of a cold dev build costs under
# callgrind: instructions, and instructions + 10 x L1 misses + 100 x last-level
# misses. The gate fails when one grows by more than 1%, and `--bless` only
# lowers a figure. design/PERFORMANCE.md §6.82; never raise one by hand.
";

/// What [`Baseline::check`] found.
pub struct Verdict {
    pub report: String,
    pub failed: bool,
    /// The baseline to commit: every figure at its lowest.
    pub next: Baseline,
}

impl Baseline {
    pub fn read(text: &str) -> Baseline {
        let mut b = Baseline::default();
        for line in text
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        {
            if let Some(rest) = line.strip_prefix("toolchain ") {
                let (arch, tools) = rest.split_once(' ').unwrap_or((rest, ""));
                b.toolchains.insert(arch.to_string(), tools.to_string());
            } else if let Some((key, n)) = line.rsplit_once(' ') {
                if let Ok(n) = n.trim().parse() {
                    let key = key.split_whitespace().collect::<Vec<_>>().join(" ");
                    b.figures.insert(key, n);
                }
            }
        }
        b
    }

    pub fn render(&self) -> String {
        let mut out = String::from(HEADER);
        for (arch, tools) in &self.toolchains {
            out.push_str(&format!("\ntoolchain {arch} {tools}\n"));
            for (key, n) in self
                .figures
                .iter()
                .filter(|(k, _)| k.split(' ').next() == Some(arch))
            {
                out.push_str(&format!("{key:<56} {n:>10.1}\n"));
            }
        }
        out
    }

    /// `measured` against this baseline. A figure over its baseline by more
    /// than [`MARGIN`] fails, as does a missing one or another toolchain.
    pub fn check(&self, arch: &str, toolchain: &str, measured: &BTreeMap<String, f64>) -> Verdict {
        let same_tools = self.toolchains.get(arch).map(String::as_str) == Some(toolchain);
        let mut next = Baseline {
            toolchains: self.toolchains.clone(),
            figures: self.figures.clone(),
        };
        next.toolchains
            .insert(arch.to_string(), toolchain.to_string());
        let mut report = format!(
            "{:<56} {:>10} {:>10} {:>9}\n",
            "baseline, per line", "was", "now", "change"
        );
        let mut failed = false;
        let mut improved = false;
        for (key, &now) in measured {
            let base = self.figures.get(key).copied().filter(|_| same_tools);
            let kept = base.map_or(now, |b: f64| b.min(now));
            next.figures.insert(key.clone(), kept);
            match base {
                None => {
                    failed = true;
                    report.push_str(&format!(
                        "{key:<56} {:>10} {now:>10.1}   no baseline\n",
                        "-"
                    ));
                }
                Some(base) => {
                    let change = (now - base) / base;
                    let mark = if change > MARGIN {
                        failed = true;
                        "  REGRESSED"
                    } else if change < -MARGIN {
                        improved = true;
                        "  improved"
                    } else {
                        ""
                    };
                    report.push_str(&format!(
                        "{key:<56} {base:>10.1} {now:>10.1} {:>+8.2}%{mark}\n",
                        change * 100.0
                    ));
                }
            }
        }
        match self.toolchains.get(arch) {
            Some(was) if was == toolchain => {}
            Some(was) => {
                failed = true;
                report.push_str(&format!(
                    "\nThe toolchain changed, and figures move with it, so re-bless:\n  was {was}\n  now {toolchain}\n"
                ));
            }
            None => {
                failed = true;
                report.push_str(&format!("\nNo baseline for {arch} yet.\n"));
            }
        }
        if improved && !failed {
            report.push_str("\nSome figures fell by more than 1%. Re-bless to hold the gain.\n");
        }
        Verdict {
            report,
            failed,
            next,
        }
    }
}
