//! `buri test --verbose`: what a golden can't hold.
//!
//! `repositories/testing/verbose` records the list with every time blanked.
//! This holds what the blanking hides: the unit a time is spelled in, the times
//! a cached suite keeps, build times that fit in the run, cache records written
//! before there were times, and the list each pass of a watch loop prints.
use crate::harness::*;

use std::io::Read as _;
use std::path::Path;
use std::time::{Duration, Instant};

/// The line of a `--verbose` list that names the test `name`.
fn test_line<'a>(stdout: &'a str, name: &str) -> &'a str {
    stdout
        .lines()
        .find(|l| (l.starts_with("  ok  ") || l.starts_with("  FAIL  ")) && l.contains(name))
        .unwrap_or_else(|| panic!("no test line names {name:?}:\n{}", indent(stdout)))
}

/// A listed line's time in nanoseconds, once the line passes the golden's check
/// that its time is spelled the runner's way, and how far the spelling may be
/// from the time it rounds: whole microseconds drop the rest, and tenths of a
/// millisecond or a second round to the nearest.
fn time_of(line: &str) -> (u64, u64) {
    assert!(blank_test_time(line).ends_with("<time>"), "not a time the runner spells: {line:?}");
    let mut words = line.rsplit(' ');
    let unit = words.next().unwrap_or_default();
    let number = words.next().unwrap_or_default();
    let tenths = || -> u64 { number.replace('.', "").parse().unwrap() };
    match unit {
        "µs" => (number.parse::<u64>().unwrap() * 1_000, 1_000),
        "ms" => (tenths() * 100_000, 50_000),
        _ => (tenths() * 100_000_000, 50_000_000),
    }
}

/// A suite line's tests' time and its build time, each as [`time_of`] reads
/// one. No build time for a suite built with others.
fn suite_times(line: &str) -> ((u64, u64), Option<(u64, u64)>) {
    match line.split_once("  built in ") {
        Some((tests, build)) => (time_of(tests), Some(time_of(&format!("//  {build}")))),
        None => {
            let tests = line.strip_suffix("  built with others");
            (time_of(tests.unwrap_or_else(|| panic!("no build on the suite line {line:?}"))), None)
        }
    }
}

/// The run's elapsed time from its summary, in nanoseconds, and how far that
/// may be from what it rounds.
fn elapsed_of(stdout: &str) -> (u64, u64) {
    let summary = stdout.lines().find(|l| l.contains(" passed, ")).unwrap_or_default();
    let seconds = summary.split_once(" (").and_then(|(_, s)| s.split_once('s')).map(|(s, _)| s);
    let tenths: u64 = seconds.and_then(|s| s.replace('.', "").parse().ok()).unwrap_or_else(|| panic!("no elapsed time in {summary:?}"));
    (tenths * 100_000_000, 50_000_000)
}

/// The `shared build` lines of a `--verbose` list.
fn shared_lines(stdout: &str) -> Vec<&str> {
    stdout.lines().filter(|l| l.starts_with("shared build")).collect()
}

/// A tail-recursive loop: a turn is a call, an add and a remainder.
const SPIN: &str = "export fn spin(n: Int, acc: Int): Int {\n    \
                    if (n == 0) { acc } else { spin(n - 1, (acc + n) % 1000) }\n}\n";

/// Each test's time is its own, and the suite's is theirs added up.
///
/// Which unit a time is spelled in is `commands::test`'s unit test of the same
/// name, on fixed durations. A run's real times move with the machine's load,
/// so this holds only what load can't change: a hundred million turns can't
/// take under 10 ms, which would be ten billion a second, and a sum of times
/// doesn't depend on how long any of them took.
#[test]
fn a_time_is_spelled_in_the_unit_its_size_calls_for() {
    let scratch = Scratch::repo("verbose-units");
    scratch.write("lib/spin/BUILD.buri", "library {\n    test {\n        sources: [\"test/spin.buri\"]\n    }\n}\n");
    scratch.write("lib/spin/lib.buri", SPIN);
    scratch.write(
        "lib/spin/test/spin.buri",
        "from \"core/testing/assert\" import * as assert;\n\
         from \"//lib/spin\" import { spin };\n\
         \n\
         test \"nothing at all\" {\n    assert.equal(1, 1);\n}\n\
         \n\
         test \"a hundred million turns\" {\n    assert.equal(spin(100000000, 0) >= 0, true);\n}\n",
    );
    let run = scratch.run(&["test", "//lib/spin", "--verbose"]);
    run.heap_ok();
    assert_eq!(run.code, 0, "{}", indent(&run.all()));
    let tests = ["nothing at all", "a hundred million turns"].map(|name| time_of(test_line(&run.stdout, name)));
    let (spun, _) = tests[1];
    assert!(spun >= 10_000_000, "a hundred million turns took {spun} ns:\n{}", indent(&run.stdout));
    let suite = run.stdout.lines().find(|l| l.starts_with("//lib/spin")).unwrap_or_default();
    let ((total, slack), _) = suite_times(suite);
    let sum: u64 = tests.iter().map(|t| t.0).sum();
    let slack = slack + tests.iter().map(|t| t.1).sum::<u64>();
    assert!(total.abs_diff(sum) <= slack, "the suite's time is not its tests' added up:\n{}", indent(&run.stdout));
}

/// The lines of a `--verbose` list, without the word `cached`.
fn listed(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|l| l.starts_with("//") || l.starts_with("  ok  ") || l.starts_with("  FAIL  "))
        .map(|l| l.replacen("  cached  ", "  ", 1))
        .collect()
}

/// Two suites that share one binary are still listed one by one, and served
/// from the cache they show the very times the run that cached them measured.
#[test]
fn a_batch_lists_each_suite_and_the_cache_keeps_its_times() {
    let scratch = Scratch::copy_of("verbose-batch", &tests_dir().join("repositories/testing/verbose/repo"));
    let cold = scratch.run(&["test", "//lib/...", "--verbose", "--explain"]);
    cold.heap_ok();
    assert_eq!(cold.code, 0, "{}", indent(&cold.all()));
    assert!(
        cold.stdout.lines().any(|l| l.starts_with("run    link //lib/money,//lib/shapes native ")),
        "the two suites did not share a binary:\n{}",
        indent(&cold.stdout)
    );
    let cold_list = listed(&cold.stdout);
    let shape: Vec<String> = cold_list.iter().map(|l| blank_test_time(l)).collect();
    assert_eq!(
        shape,
        [
            "//lib/money  native  3 tests  <time>  built with others",
            "  ok    cents.buri  adding cents carries into dollars  <time>",
            "  ok    cents.buri  the cents are what is left over    <time>",
            "  ok    rates.buri  a rate of zero keeps nothing       <time>",
            "//lib/shapes  native  3 tests  <time>  built with others",
            "  ok    shapes.buri      a square has four equal sides        <time>",
            "  ok    shapes.buri      a square's area is its side squared  <time>",
            "  ok    solid/cube.buri  a cube has six square faces          <time>",
        ],
        "{}",
        indent(&cold.stdout)
    );
    // The binary they share is built once, and its time is told once.
    let shared: Vec<String> = shared_lines(&cold.stdout).into_iter().map(blank_test_time).collect();
    assert_eq!(shared, ["shared build  <time>"], "{}", indent(&cold.stdout));

    let cached = scratch.run(&["test", "//lib/...", "--verbose"]);
    assert_eq!(cached.code, 0, "{}", indent(&cached.all()));
    assert_eq!(cached.stdout.matches("  cached  ").count(), 2, "{}", indent(&cached.stdout));
    assert_eq!(listed(&cached.stdout), cold_list, "the cache did not keep the times it was given");
    assert!(shared_lines(&cached.stdout).is_empty(), "nothing was built:\n{}", indent(&cached.stdout));
}

/// A suite built on its own shows how long that took, JavaScript or native,
/// and served from the cache it shows the very build time it was built in.
#[test]
fn a_suite_built_alone_shows_its_build_and_the_cache_keeps_it() {
    let scratch = Scratch::copy_of("verbose-alone", &tests_dir().join("repositories/testing/verbose/repo"));
    let cold = scratch.run(&["test", "//apps/web", "//lib/shapes", "--verbose"]);
    cold.heap_ok();
    assert_eq!(cold.code, 0, "{}", indent(&cold.all()));
    let suites: Vec<&str> = cold.stdout.lines().filter(|l| l.starts_with("//")).collect();
    assert_eq!(
        suites.iter().map(|l| blank_test_time(l)).collect::<Vec<_>>(),
        [
            "//apps/web  js  2 tests  <time>  built in <time>",
            "//lib/shapes  native  3 tests  <time>  built in <time>",
        ],
        "{}",
        indent(&cold.stdout)
    );
    let shared: Vec<String> = shared_lines(&cold.stdout).into_iter().map(blank_test_time).collect();
    assert_eq!(shared, ["shared build  <time>"], "{}", indent(&cold.stdout));
    // Load stretches a build and the run around it alike.
    let (elapsed, rounding) = elapsed_of(&cold.stdout);
    for line in &suites {
        let (_, build) = suite_times(line);
        let (build, slack) = build.unwrap_or_default();
        assert!(build <= elapsed + rounding + slack, "a build outlasted its run:\n{}", indent(&cold.stdout));
    }

    let cached = scratch.run(&["test", "//apps/web", "//lib/shapes", "--verbose"]);
    assert_eq!(cached.code, 0, "{}", indent(&cached.all()));
    assert_eq!(cached.stdout.matches("  cached  ").count(), 2, "{}", indent(&cached.stdout));
    assert_eq!(listed(&cached.stdout), listed(&cold.stdout), "the cache did not keep the build times");
    assert!(shared_lines(&cached.stdout).is_empty(), "nothing was built:\n{}", indent(&cached.stdout));
}

/// One worker builds and runs one step at a time, and only the loading runs
/// beside it, so the build times can't add up to more than twice the run,
/// however loaded the machine is.
#[test]
fn the_build_times_add_up_within_the_run() {
    let scratch = Scratch::copy_of("verbose-sum", &tests_dir().join("repositories/testing/verbose/repo"));
    let run = scratch.run(&["test", "//...", "--verbose", "--jobs=1"]);
    run.heap_ok();
    assert_eq!(run.code, 0, "{}", indent(&run.all()));
    let mut builds: Vec<(u64, u64)> = run
        .stdout
        .lines()
        .filter(|l| l.starts_with("//"))
        .filter_map(|l| suite_times(l).1)
        .collect();
    let shared = shared_lines(&run.stdout);
    assert_eq!(shared.len(), 1, "{}", indent(&run.stdout));
    builds.push(time_of(shared[0]));
    let (sum, slack) = builds.iter().fold((0, 0), |(s, k), (t, r)| (s + t, k + r));
    let (elapsed, rounding) = elapsed_of(&run.stdout);
    assert!(sum <= 2 * (elapsed + rounding) + slack, "the builds outlasted the run twice over:\n{}", indent(&run.stdout));
}

/// A verdict cached before there were times still serves: the suite says
/// `cached` and no time, and so does every test in it.
///
/// The record is made old by hand, the way that toolchain wrote it: each
/// test's `ms` was always 0, and nothing said how long it or the build took.
#[test]
fn a_verdict_cached_before_there_were_times_serves_without_them() {
    let scratch = Scratch::copy_of("verbose-old-record", &tests_dir().join("repositories/testing/verbose/repo"));
    let first = scratch.run(&["test", "//...", "--verbose"]);
    assert_eq!(first.code, 0, "{}", indent(&first.all()));
    let rewritten = make_records_old(&scratch.path(".buri/cache"), |text| without_build(&without_times(text)));
    assert_eq!(rewritten, 3, "expected one verdict record per suite");

    let run = scratch.run(&["test", "//...", "--verbose"]);
    run.heap_ok();
    assert_eq!(run.code, 0, "{}", indent(&run.all()));
    assert_eq!(
        normalise(&run.stdout, &scratch.root),
        "//apps/web  js  2 tests  cached\n\
         \x20 ok    page.buri  the title renders\n\
         \x20 ok    page.buri  the title fits on a tab\n\
         //lib/money  native  3 tests  cached\n\
         \x20 ok    cents.buri  adding cents carries into dollars\n\
         \x20 ok    cents.buri  the cents are what is left over\n\
         \x20 ok    rates.buri  a rate of zero keeps nothing\n\
         //lib/shapes  native  3 tests  cached\n\
         \x20 ok    shapes.buri      a square has four equal sides\n\
         \x20 ok    shapes.buri      a square's area is its side squared\n\
         \x20 ok    solid/cube.buri  a cube has six square faces\n\
         \n\
         8 passed, 0 failed, 0 skipped (0.0s, 8 cached)\n",
    );
    let plain = scratch.run(&["test", "//..."]);
    assert_eq!(normalise(&plain.stdout, &scratch.root), "8 passed, 0 failed, 0 skipped (0.0s, 8 cached)\n");
}

/// A verdict cached before builds were timed keeps its tests' times, and says
/// nothing about its build.
#[test]
fn a_verdict_cached_before_builds_were_timed_keeps_its_test_times() {
    let scratch = Scratch::copy_of("verbose-untimed-build", &tests_dir().join("repositories/testing/verbose/repo"));
    let first = scratch.run(&["test", "//...", "--verbose"]);
    assert_eq!(first.code, 0, "{}", indent(&first.all()));
    let rewritten = make_records_old(&scratch.path(".buri/cache"), without_build);
    assert_eq!(rewritten, 3, "expected one verdict record per suite");

    let run = scratch.run(&["test", "//...", "--verbose"]);
    run.heap_ok();
    assert_eq!(run.code, 0, "{}", indent(&run.all()));
    let suites: Vec<String> = run.stdout.lines().filter(|l| l.starts_with("//")).map(blank_test_time).collect();
    assert_eq!(
        suites,
        [
            "//apps/web  js  2 tests  cached  <time>",
            "//lib/money  native  3 tests  cached  <time>",
            "//lib/shapes  native  3 tests  cached  <time>",
        ],
        "{}",
        indent(&run.stdout)
    );
    let first_tests: Vec<&str> = first.stdout.lines().filter(|l| l.starts_with("  ok  ")).collect();
    let tests: Vec<&str> = run.stdout.lines().filter(|l| l.starts_with("  ok  ")).collect();
    assert_eq!(tests, first_tests, "the tests' times were not kept");
}

/// `text` without the line that says how long the build took.
fn without_build(text: &str) -> String {
    text.split_inclusive('\n').filter(|l| !l.starts_with("{\"build\":")).collect()
}

/// Rewrites every verdict record under `cache` with `old`, into the shape an
/// older toolchain wrote, and says how many it rewrote.
fn make_records_old(cache: &Path, old: impl Fn(&str) -> String) -> usize {
    let mut rewritten = 0;
    for dir in std::fs::read_dir(cache).unwrap() {
        let dir = dir.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        for file in std::fs::read_dir(&dir).unwrap() {
            let file = file.unwrap().path();
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            if !text.starts_with("reads ") || !text.contains("\"ok\":true") {
                continue;
            }
            let older = old(&text);
            assert_ne!(older, text, "a verdict record held nothing to take out:\n{text}");
            std::fs::write(&file, older).unwrap();
            rewritten += 1;
        }
    }
    rewritten
}

/// `"ns":<digits>` as `"ms":0`, everywhere in `text`.
fn without_times(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("\"ns\":") {
        out.push_str(&rest[..at]);
        out.push_str("\"ms\":0");
        rest = rest[at + 5..].trim_start_matches(|c: char| c.is_ascii_digit());
    }
    out.push_str(rest);
    out
}

/// Under `--watch`, each pass prints the list of the pass it is.
///
/// `--watch` refuses a pipe, so the loop runs under `script`, which hands it a
/// terminal. The edit is written once the first pass prints: the loop takes its
/// stamps before printing, so an edit after it always wakes a second pass.
#[test]
fn each_pass_of_a_watch_loop_lists_its_own_suites() {
    let scratch = Scratch::copy_of("verbose-watch", &tests_dir().join("repositories/testing/verbose/repo"));
    let buri = env!("CARGO_BIN_EXE_buri");
    let mut cmd = std::process::Command::new("script");
    if cfg!(target_os = "macos") {
        cmd.args(["-q", "/dev/null", buri, "test", "//lib/shapes", "--watch", "--verbose"]);
    } else {
        cmd.args(["-qec", &format!("{buri} test //lib/shapes --watch --verbose"), "/dev/null"]);
    }
    let mut child = cmd
        .current_dir(&scratch.root)
        .env("BURI_HOME", sweep::kept::shared_cross_home())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("`script` runs");
    let mut pipe = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = pipe.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut seen = String::new();
    let deadline = Instant::now() + Duration::from_secs(240);
    let mut edited = false;
    while seen.matches(" passed, ").count() < 2 && Instant::now() < deadline {
        if let Ok(bytes) = rx.recv_timeout(Duration::from_millis(100)) {
            seen.push_str(&String::from_utf8_lossy(&bytes));
        }
        if !edited && seen.contains(" passed, ") {
            edited = true;
            scratch.write(
                "lib/shapes/shapes.buri",
                "export fn square(side: Int): Int {\n    side * side\n}\n\n\
                 export fn perimeter(side: Int): Int {\n    side + side + side + side\n}\n",
            );
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let text = seen.replace('\r', "");
    let passes: Vec<&str> = text.split("── ").skip(1).collect();
    assert_eq!(passes.len(), 2, "expected two passes:\n{}", indent(&text));
    for pass in passes {
        let lines: Vec<String> =
            pass.lines().skip(1).take_while(|l| !l.is_empty()).map(blank_test_time).collect();
        assert_eq!(
            lines.join("\n"),
            "//lib/shapes  native  3 tests  <time>  built in <time>\n\
             \x20 ok    shapes.buri      a square has four equal sides        <time>\n\
             \x20 ok    shapes.buri      a square's area is its side squared  <time>\n\
             \x20 ok    solid/cube.buri  a cube has six square faces          <time>\n\
             shared build  <time>",
            "a pass did not list its suite:\n{}",
            indent(&text)
        );
        assert!(pass.contains("3 passed, 0 failed, 0 skipped"), "{}", indent(&text));
    }
}
