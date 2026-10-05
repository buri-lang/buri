//! **Generated matches**, checked in, on every backend.
//!
//! `cli/tests/matches/` holds programs that [`generate`] wrote: each is a few
//! type declarations and sixteen functions holding one `match` apiece, called
//! with two or three values, beside the `expected.out` the generator worked
//! out from its own model of the values and the patterns. Each program runs on
//! JavaScript and on every native backend this toolchain was built with,
//! under the heap check, and every one of them must print `expected.out`,
//! exit 0, say nothing on standard error and give back every block.
//!
//! The corpus is written by the generator and read by the suite; nothing is
//! generated while the suite runs. A second test holds the two together: run
//! the generator from the recorded [`generate::SEED`] and it writes exactly
//! the checked-in files.
//!
//! ```text
//! BURI_BLESS=1 cargo test -p buri --test native matches::   # rewrite the corpus
//! BURI_MATCHES_SEED=7 BURI_MATCHES_BATCHES=200 \
//!   cargo test -p buri --test native matches::generated     # a sweep, not written
//! ```

mod generate;

use crate::agreement;
use crate::shard;
use std::path::{Path, PathBuf};

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/matches")
}

fn batch_name(index: usize) -> String {
    format!("batch_{index:02}")
}

struct Case {
    name: String,
    source: String,
    expected: String,
}

/// A seed and a batch count from the environment, for a sweep over programs
/// the corpus does not hold.
fn sweep() -> Option<(u64, usize)> {
    let raw = std::env::var("BURI_MATCHES_SEED").ok()?;
    let raw = raw.trim().replace('_', "");
    let seed = match raw.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => raw.parse().ok(),
    }
    .unwrap_or_else(|| panic!("BURI_MATCHES_SEED={raw} is not a number"));
    let batches = std::env::var("BURI_MATCHES_BATCHES")
        .ok()
        .map_or(generate::BATCHES, |n| n.trim().parse().expect("BURI_MATCHES_BATCHES"));
    Some((seed, batches))
}

/// The checked-in corpus, or a sweep's programs where the environment names
/// one.
fn cases() -> Vec<Case> {
    if let Some((seed, batches)) = sweep() {
        return (0..batches)
            .map(|i| {
                let b = generate::batch(seed, i);
                Case { name: format!("seed {seed:#x} batch {i}"), source: b.source, expected: b.expected }
            })
            .collect();
    }
    (0..generate::BATCHES)
        .map(|i| {
            let dir = corpus_dir().join(batch_name(i));
            let read = |file: &str| {
                std::fs::read_to_string(dir.join(file)).unwrap_or_else(|e| {
                    panic!(
                        "{}: {e}. Write the corpus with \
                         `BURI_BLESS=1 cargo test -p buri --test native matches::`",
                        dir.join(file).display()
                    )
                })
            };
            Case { name: batch_name(i), source: read("main.buri"), expected: read("expected.out") }
        })
        .collect()
}

fn case_count() -> usize {
    sweep().map_or(generate::BATCHES, |(_, n)| n)
}

/// What is wrong with one program on one backend, or nothing.
fn check(case: &Case) -> Vec<String> {
    let name = &case.name;
    let (checked, paths) = agreement::analyze(name, &case.source);
    let mut wrong = Vec::new();
    let js = agreement::run_js(name, &checked, &paths);
    let mut said = vec![("javascript", js)];
    for native in agreement::natives(name) {
        let refusal = agreement::native_refusal(name, native, &checked, &paths);
        if !refusal.is_empty() {
            wrong.push(format!("{name}: `{}` refused the program: {refusal}", native.name));
            continue;
        }
        if let Some(ran) = agreement::run_native(name, native, &checked, &paths) {
            said.push((native.name, ran));
        }
    }
    for (backend, ran) in said {
        if let Some(n) = crate::shared::leaked_blocks(ran.status, &ran.stderr) {
            wrong.push(format!("{name}: `{backend}` leaked {n} block(s)"));
        }
        if ran.status != 0 || !ran.stderr.is_empty() {
            wrong.push(format!(
                "{name}: `{backend}` exited {} saying:\n{}",
                ran.status,
                ran.stderr.trim_end()
            ));
        }
        if ran.stdout != case.expected {
            let first = case
                .expected
                .lines()
                .zip(ran.stdout.lines().chain(std::iter::repeat("<nothing>")))
                .find(|(want, got)| want != got);
            wrong.push(match first {
                Some((want, got)) => {
                    format!("{name}: `{backend}` printed {got:?} where the expected arm prints {want:?}")
                }
                None => format!(
                    "{name}: `{backend}` printed more than expected:\n{}",
                    ran.stdout
                ),
            });
        }
    }
    wrong
}

fn generated_shard(at: usize, count: usize) {
    if let Some(why) = agreement::skip_reason() {
        crate::ci::skipped("generated matches", &why);
        return;
    }
    let cases = cases();
    let mut wrong = Vec::new();
    for case in shard::of(&cases, at, count) {
        let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(case)))
            .unwrap_or_else(|panic| {
                let message = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_string()))
                    .unwrap_or_default();
                vec![message]
            });
        wrong.extend(found);
    }
    assert!(wrong.is_empty(), "{} finding(s):\n\n{}", wrong.len(), wrong.join("\n\n"));
}

shards! {
    /// Every generated program prints the arms the generator expects, on
    /// every backend, and leaks nothing.
    generated_matches_print_the_expected_arms(generated_shard, case_count) =
        shard_0 shard_1 shard_2 shard_3 shard_4 shard_5 shard_6 shard_7;
}

/// The checked-in corpus is what the generator writes from the recorded seed,
/// byte for byte. `BURI_BLESS=1` writes it.
#[test]
fn the_checked_in_matches_are_what_the_generator_writes() {
    let bless = std::env::var_os("BURI_BLESS").is_some();
    let root = corpus_dir();
    let mut stale = Vec::new();
    let mut wanted = Vec::new();
    for i in 0..generate::BATCHES {
        let b = generate::batch(generate::SEED, i);
        let dir = root.join(batch_name(i));
        wanted.push(batch_name(i));
        for (file, bytes) in [("main.buri", &b.source), ("expected.out", &b.expected)] {
            let path = dir.join(file);
            if std::fs::read_to_string(&path).ok().as_ref() == Some(bytes) {
                continue;
            }
            if bless {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(&path, bytes).unwrap();
            } else {
                stale.push(path.display().to_string());
            }
        }
    }
    let mut present: Vec<String> = std::fs::read_dir(&root)
        .map(|entries| {
            entries.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().to_string()).collect()
        })
        .unwrap_or_default();
    present.sort();
    for name in present.iter().filter(|n| !wanted.contains(n)) {
        if bless {
            std::fs::remove_dir_all(root.join(name)).unwrap();
        } else {
            stale.push(format!("{} is not a batch the generator writes", root.join(name).display()));
        }
    }
    assert!(
        stale.is_empty(),
        "the generated matches are out of date with the generator:\n  {}\n\
         Rewrite them with `BURI_BLESS=1 cargo test -p buri --test native matches::`.",
        stale.join("\n  ")
    );
}
