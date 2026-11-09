//! The compile goals' benchmark (`design/PERFORMANCE.md` §6.82): its generated
//! workspace, a real build of it, and the report and baseline `--goals` prints.
use crate::harness::*;
use crate::workspace::{self, MONOREPO_TINY, TINY};

#[path = "../../benches/goals/report.rs"]
#[allow(dead_code)]
mod report;

use report::{Baseline, Events, Measured, Profile, PHASES};

/// Goals the tests rate against: round numbers, not the real ones.
const GOALS: [f64; 3] = [1e7, 1e6, 1e5];

/// **The generator writes the same workspace every time**, so a figure
/// measured today and one measured tomorrow are of the same source.
#[test]
fn the_generated_workspace_is_the_same_every_time() {
    for size in workspace::SIZES {
        let (a, b) = (workspace::files(size), workspace::files(size));
        assert_eq!(
            a.len(),
            b.len(),
            "{} wrote a different file list",
            size.name
        );
        for (a, b) in a.iter().zip(&b) {
            assert_eq!(a.path, b.path, "{} wrote a different file list", size.name);
            assert!(
                a.text == b.text,
                "{} wrote {} differently",
                size.name,
                a.path
            );
        }
    }
}

/// The tiny workspace, written into a scratch repository.
fn tiny() -> Scratch {
    let scratch = Scratch::empty("goals-tiny");
    for f in workspace::files(&TINY) {
        scratch.write(&f.path, &f.text);
    }
    scratch
}

/// **A real build of the workspace is lint-clean, and its report names each
/// goal phase with a CPU time, a rate and a share of its goal.** The build is
/// the one `--goals` measures: cold, dev, `//...`, on one thread.
#[test]
fn the_report_names_each_phase_of_a_real_build() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::goals", &why);
        return;
    }
    let scratch = tiny();
    let run = scratch.run_with_env(
        &["build", "//..."],
        &[("BURI_PROFILE", "1"), ("BURI_THREADS", "1")],
    );
    run.ok().silent_about("warning");
    let profile = Profile::read(&run.all());
    let lines = profile.lines.expect("the profile counted lines of code");
    let figures = profile.phases().map(|(cpu, n)| (Some(cpu), n));
    let report = Measured::new("tiny", lines, GOALS, figures, None).render();
    for phase in PHASES {
        let row = report
            .lines()
            .find(|l| l.trim_start().starts_with(&format!("{phase} ")))
            .unwrap_or_else(|| panic!("no {phase} row in\n{}", indent(&report)));
        let words: Vec<&str> = row.split_whitespace().collect();
        assert!(
            row.contains(" s ") && row.contains('%'),
            "{phase} has no CPU time or share of its goal:\n{}",
            indent(&report)
        );
        assert!(
            !words.contains(&"0.000"),
            "{phase} took no CPU:\n{}",
            indent(&report)
        );
    }
    let counted = format!("{} lines of code", report::commas(lines as f64));
    assert!(report.contains(&counted), "{}", indent(&report));
}

/// **A real build of the monorepo is lint-clean, and every test in it
/// passes**: its generators run, and its `node` programs and pages build
/// beside its native ones.
#[test]
fn the_monorepo_builds_lint_clean_and_its_tests_pass() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::goals", &why);
        return;
    }
    let scratch = Scratch::empty("goals-monorepo-tiny");
    for f in workspace::files(&MONOREPO_TINY) {
        scratch.write(&f.path, &f.text);
    }
    scratch
        .run(&["build", "//..."])
        .ok()
        .silent_about("warning");
    scratch
        .run(&["test", "//..."])
        .ok()
        .silent_about("warning")
        .says(" passed, 0 failed, 0 skipped");
}

/// A `buri test //...` profile: the tests' processes took 9 s.
const TEST_PROFILE: &str = "\
buri profile           instructions        cpu       busy  child instructions  child cpu
  lex+parse                100.0 M    0.010 s    0.010 s               0.0 M    0.000 s
  check                    300.0 M    0.030 s    0.030 s               0.0 M    0.000 s
  emit                     600.0 M    0.060 s    0.060 s               0.0 M    0.000 s
  run                       50.0 M    0.005 s    0.213 s           90000.0 M    9.000 s
  other                    250.0 M    0.025 s    1.414 s               0.0 M    0.000 s
  all phases              1300.0 M    0.130 s    1.727 s           90000.0 M    9.000 s
source: 1000 lines of code lexed, each file once
process: 1.400 B instructions, 0.140 s cpu, 0.220 s runnable but not running, peak 83 MB
child processes: 9.100 s cpu
";

/// **Of a `buri test //...`, the report measures the build**: the tests'
/// processes are left out, and the runner's own work around them is other.
#[test]
fn a_test_builds_report_leaves_the_tests_out() {
    let profile = Profile::read(TEST_PROFILE);
    let [parse, check, other, codegen] = profile.phases();
    assert_eq!(parse, (0.010, Some(100e6)));
    assert_eq!(check, (0.030, Some(300e6)));
    let near = |a: f64, b: f64| (a - b).abs() < 1e-6 * b.max(1.0);
    assert!(
        near(other.0, 0.030) && near(other.1.unwrap(), 300e6),
        "{other:?}"
    );
    // The process and the linker's 0.1 s, less parse, check and other.
    assert!(near(codegen.0, 0.140 + 0.100 - 0.070), "{codegen:?}");
    assert!(near(codegen.1.unwrap(), 1400e6 - 700e6), "{codegen:?}");
    let figures = profile.phases().map(|(cpu, n)| (Some(cpu), n));
    let measured = Measured::new("monorepo-tiny", 1000, GOALS, figures, None).of_tests();
    let report = measured.render();
    assert!(
        report.contains("what a cold `buri test //...` builds of the monorepo-tiny workspace"),
        "{}",
        indent(&report)
    );
    let figures = measured.figures("aarch64");
    let key = "aarch64 monorepo-tiny test-build dev-codegen instructions";
    assert!(figures.contains_key(key), "no {key} in {figures:?}");
}

/// One callgrind output file with cache simulation, as `--goals=count` reads.
fn callgrind(instructions: u64, l1: u64, ll: u64) -> String {
    format!(
        "version: 1\ncreator: callgrind\ncmd: buri build //...\n\
         events: Ir Dr Dw I1mr D1mr D1mw ILmr DLmr DLmw\n\
         fn=main\n0 1 1 1 1 1 1 1 1 1\n\
         totals: {instructions} 9 9 {l1} 0 0 {ll} 0 0\n"
    )
}

/// **The report holds each goal phase's cache cost per line**: instructions,
/// plus 10 for each L1 miss and 100 for each last-level miss, from the
/// callgrind runs of each phase group and of the whole build.
#[test]
fn the_report_holds_each_phases_cache_cost() {
    let read = |i, l1, ll| Events::read(&callgrind(i, l1, ll)).unwrap();
    let [parse, check, other] = [read(1000, 10, 1), read(4000, 20, 2), read(500, 0, 0)];
    let whole = read(20_000, 100, 10);
    let codegen = whole.minus(parse).minus(check).minus(other);
    let events = [parse, check, other, codegen];
    let measured = Measured::new(
        "tiny",
        100,
        GOALS,
        events.map(|e| (None, Some(e.instructions))),
        Some(events.map(|e| e.cost())),
    );
    let report = measured.render();
    for (phase, cost) in [
        ("parse", "12.0"),
        ("check", "44.0"),
        ("dev codegen", "159.0"),
    ] {
        let row = report
            .lines()
            .find(|l| l.trim_start().starts_with(&format!("{phase} ")))
            .unwrap_or_else(|| panic!("no {phase} row in\n{}", indent(&report)));
        assert!(
            row.trim_end().ends_with(cost),
            "{phase}'s cost per line should be {cost}:\n{}",
            indent(&report)
        );
    }
    assert!(report.contains("cost/line"), "{}", indent(&report));
    let figures = measured.figures("aarch64");
    for phase in ["parse", "check", "dev-codegen"] {
        for what in ["instructions", "cost"] {
            let key = format!("aarch64 tiny {phase} {what}");
            assert!(figures.contains_key(&key), "no {key} in {figures:?}");
        }
    }
}

/// A callgrind file without cache simulation is refused rather than read as
/// a cost of bare instructions.
#[test]
fn a_count_without_cache_simulation_is_refused() {
    let text = "events: Ir\ntotals: 1000\n";
    assert!(Events::read(text).unwrap_err().contains("--cache-sim=yes"));
}

/// **The baseline only falls.** A figure over it by more than the margin
/// fails, one within it passes, one under it is kept as the new baseline, and
/// another toolchain asks for a re-bless.
#[test]
fn the_baseline_only_falls() {
    let base = Baseline::read(
        "# header\ntoolchain aarch64 rustc 1\n\
         aarch64 ci parse instructions    100.0\n\
         aarch64 ci check cost            200.0\n",
    );
    let measured = |parse: f64, check: f64| {
        [
            ("aarch64 ci parse instructions".to_string(), parse),
            ("aarch64 ci check cost".to_string(), check),
        ]
        .into_iter()
        .collect()
    };
    let within = base.check("aarch64", "rustc 1", &measured(100.9, 200.0));
    assert!(!within.failed, "{}", within.report);
    let over = base.check("aarch64", "rustc 1", &measured(101.5, 200.0));
    assert!(
        over.failed && over.report.contains("REGRESSED"),
        "{}",
        over.report
    );
    assert_eq!(over.next.figures["aarch64 ci parse instructions"], 100.0);
    let under = base.check("aarch64", "rustc 1", &measured(90.0, 210.0));
    assert!(under.failed, "{}", under.report);
    assert_eq!(under.next.figures["aarch64 ci parse instructions"], 90.0);
    assert_eq!(under.next.figures["aarch64 ci check cost"], 200.0);
    let again = Baseline::read(&under.next.render());
    assert_eq!(again.figures, under.next.figures);
    let moved = base.check("aarch64", "rustc 2", &measured(100.0, 200.0));
    assert!(
        moved.failed && moved.report.contains("re-bless"),
        "{}",
        moved.report
    );
}
