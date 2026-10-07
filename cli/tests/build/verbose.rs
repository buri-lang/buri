//! `buri test --verbose`: what a golden can't hold.
//!
//! `repositories/testing/verbose` records the list with every time blanked.
//! This holds what the blanking hides: the unit a time is spelled in, a cache
//! record written before there were times, and the list each pass of a watch
//! loop prints.
use crate::harness::*;

use std::io::Read as _;
use std::path::Path;
use std::time::{Duration, Instant};

/// The test lines of `//lib/<package>` in a `--verbose` run, keyed by name.
fn test_line<'a>(stdout: &'a str, name: &str) -> &'a str {
    stdout
        .lines()
        .find(|l| (l.starts_with("  ok  ") || l.starts_with("  FAIL  ")) && l.contains(name))
        .unwrap_or_else(|| panic!("no test line names {name:?}:\n{}", indent(stdout)))
}

/// The unit a listed line ends in, once the line passes the golden's check
/// that its time is spelled the runner's way.
fn unit_of(line: &str) -> &str {
    assert!(blank_test_time(line).ends_with("<time>"), "not a time the runner spells: {line:?}");
    line.rsplit(' ').next().unwrap_or_default()
}

/// A tail-recursive loop the native backend runs at a few nanoseconds a turn.
const SPIN: &str = "export fn spin(n: Int, acc: Int): Int {\n    \
                    if (n == 0) { acc } else { spin(n - 1, (acc + n) % 1000) }\n}\n";

/// Microseconds under a millisecond, milliseconds under a second, then seconds.
///
/// Each loop sits well inside its unit on the machine this was measured on:
/// ten million turns took 30 ms, and six hundred million about 1.8 s.
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
         test \"ten million turns\" {\n    assert.equal(spin(10000000, 0) >= 0, true);\n}\n\
         \n\
         test \"six hundred million turns\" {\n    assert.equal(spin(600000000, 0) >= 0, true);\n}\n",
    );
    let run = scratch.run(&["test", "//lib/spin", "--verbose"]);
    run.heap_ok();
    assert_eq!(run.code, 0, "{}", indent(&run.all()));
    assert_eq!(unit_of(test_line(&run.stdout, "nothing at all")), "µs");
    assert_eq!(unit_of(test_line(&run.stdout, "ten million turns")), "ms");
    assert_eq!(unit_of(test_line(&run.stdout, "six hundred million turns")), "s");
    let suite = run.stdout.lines().find(|l| l.starts_with("//lib/spin")).unwrap_or_default();
    assert_eq!(unit_of(suite), "s", "the suite's time is not its tests' time:\n{}", indent(&run.stdout));
}

/// A verdict cached before there were times still serves: the suite says
/// `cached` and no time, and so does every test in it.
///
/// The record is made old by hand, the way the last toolchain wrote it: each
/// test's `ms` was always 0, and nothing else said how long it took.
#[test]
fn a_verdict_cached_before_there_were_times_serves_without_them() {
    let scratch = Scratch::copy_of("verbose-old-record", &tests_dir().join("repositories/testing/verbose/repo"));
    let first = scratch.run(&["test", "//...", "--verbose"]);
    assert_eq!(first.code, 0, "{}", indent(&first.all()));
    let rewritten = make_records_old(&scratch.path(".buri/cache"));
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

/// Rewrites every verdict record under `cache` into the shape it had before
/// tests were timed, and says how many it rewrote.
fn make_records_old(cache: &Path) -> usize {
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
            let old = without_times(&text);
            assert_ne!(old, text, "a verdict record held no time to take out:\n{text}");
            std::fs::write(&file, old).unwrap();
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
/// terminal. The edit is written again until a second pass starts, because the
/// loop takes its stamps just after the first pass prints, and an edit that
/// lands before them wakes nothing.
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
    let mut edits = 0;
    let mut last_edit = Instant::now();
    while seen.matches(" passed, ").count() < 2 && Instant::now() < deadline {
        if let Ok(bytes) = rx.recv_timeout(Duration::from_millis(100)) {
            seen.push_str(&String::from_utf8_lossy(&bytes));
        }
        let first_done = seen.contains(" passed, ");
        if first_done && (edits == 0 || last_edit.elapsed() > Duration::from_secs(2)) && !seen.contains("run 2") {
            edits += 1;
            let perimeter = ["side * 4", "side + side + side + side", "2 * (side + side)", "4 * side"];
            scratch.write(
                "lib/shapes/shapes.buri",
                &format!(
                    "export fn square(side: Int): Int {{\n    side * side\n}}\n\n\
                     export fn perimeter(side: Int): Int {{\n    {}\n}}\n",
                    perimeter[(edits - 1) % perimeter.len()]
                ),
            );
            last_edit = Instant::now();
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let text = seen.replace('\r', "");
    let passes: Vec<&str> = text.split("── ").skip(1).collect();
    assert_eq!(passes.len(), 2, "expected two passes:\n{}", indent(&text));
    for pass in passes {
        let listed: Vec<String> =
            pass.lines().skip(1).take_while(|l| !l.is_empty()).map(blank_test_time).collect();
        assert_eq!(
            listed.join("\n"),
            "//lib/shapes  native  3 tests  <time>\n\
             \x20 ok    shapes.buri      a square has four equal sides        <time>\n\
             \x20 ok    shapes.buri      a square's area is its side squared  <time>\n\
             \x20 ok    solid/cube.buri  a cube has six square faces          <time>",
            "a pass did not list its suite:\n{}",
            indent(&text)
        );
        assert!(pass.contains("3 passed, 0 failed, 0 skipped"), "{}", indent(&text));
    }
}
