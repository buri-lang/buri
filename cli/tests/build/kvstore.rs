//! The runtime benchmark's repository, `cli/benches/kvstore`
//! (`design/PERFORMANCE.md` §9). The gate counts its soak run, so it has to
//! stay a program that builds, passes its own tests and settles every seed.
use crate::harness::*;

fn benchmark(name: &str) -> Scratch {
    Scratch::copy_of(name, &repo_root().join("cli/benches/kvstore"))
}

/// **Its tests pass and it is lint-clean**, as a user's repository would be.
#[test]
fn the_benchmark_passes_its_own_tests() {
    let scratch = benchmark("kvstore-test");
    let run = scratch.run(&["test", "//..."]);
    run.ok().says(" 0 failed");
    assert!(
        run.tests_passed() >= 60,
        "too few tests ran:\n{}",
        indent(&run.all())
    );
    scratch.run(&["lint", "//..."]).ok().silent_about("warning");
}

/// **The soak run settles every seed** and prints one line for each.
#[test]
fn the_soak_run_settles_every_seed() {
    let scratch = benchmark("kvstore-soak");
    let run = scratch.run(&["run", "//apps/soak"]);
    run.ok();
    for seed in 1..=8 {
        run.says(&format!("seed {seed}: commit "));
    }
}
