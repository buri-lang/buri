//! Source in, `core/buri/ast` out, source back: the whole conformance
//! repository through `parse` and then `print`.
//!
//! `core/buri/ast` is a second implementation of the grammar. The compiler's
//! own parser cannot reach a program that runs on the JavaScript backend, which
//! is where a generator runs, so the module reads Buri in Buri (see
//! design/native/DECISIONS.md). A second implementation is worth having only if
//! something holds it to the first, and this is that something.
//!
//! The question is not "does the text come back the same". It does not, and it
//! should not: `print` has no page width, it drops line comments, it sorts the
//! leading import run and it moves a `derive` onto the declaration it is about.
//! The question is whether the **program** came back — so the corpus is
//! rewritten and then run, and a source that came back meaning something else
//! stops compiling, or stops passing, in the suite that owns it. That is the
//! compiler's own view of both texts, taken by the compiler rather than
//! described here.
//!
//! `cli/tests/ast_round_trip/` is the driver: an ordinary Buri binary that
//! reads a manifest of paths and rewrites each one. It is Buri because `parse`
//! is Buri, and there is no way to ask this question from Rust.
use crate::harness::*;
use std::path::{Path, PathBuf};

/// Every `.buri` source under a tree, in a stable order. `BUILD.buri` and
/// `REPO.buri` are build files rather than Buri sources, and `.buri/` is the
/// cache directory a build leaves behind.
fn sources(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if name != ".buri" {
                walk(&path, out);
            }
        } else if name.ends_with(".buri") && name != "BUILD.buri" && name != "REPO.buri" {
            out.push(path);
        }
    }
}

/// The built driver, run over a manifest. Straight through the JavaScript
/// runtime rather than `buri run`, because every source gets a run of its own
/// and they run at once: `buri run` would have each of them check the driver's
/// build, in one repository, at the same time.
fn drive(driver: &Scratch, artifact: &Path, manifest: &Path) -> Run {
    let out = std::process::Command::new(js_runtime())
        .arg(artifact)
        .arg(manifest)
        .current_dir(&driver.root)
        .output()
        .expect("the javascript runtime runs");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        what: format!("{} {} {}", js_runtime(), artifact.display(), manifest.display()),
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// One source's two passes: what the driver said and what the file held after
/// each. There is no second pass over a source the first pass refused.
struct Passes {
    first: Run,
    once: String,
    second: Option<(Run, String)>,
}

/// A failed run, under the source it was over. The driver names the file for
/// a refusal, but not when the runtime itself fell over.
fn failed(source: &Path, run: &Run) -> String {
    format!("{} (`{}` exited {}):\n{}", source.display(), run.what, run.code, indent(&run.all()))
}

/// Three claims about `core/buri/ast`, over one corpus and one driver build.
///
/// 1. **`parse` reads everything the compiler reads.** A path in the driver's
///    output is a source it refused, with the message and the byte range.
/// 2. **Printing is a fixed point**, which is `parse(print(m)) == m` in the one
///    form that can be asserted. Structural equality cannot be: a generated
///    node's `Origin` is `nowhere()` and a parsed one's names the text it came
///    from, so two such modules differ in every node by construction. Two
///    modules that print the same text are the same module everywhere `print`
///    looks, which is everywhere. The second pass reads source `print` wrote
///    rather than source a person wrote, which is the shape a generator hands
///    the compiler — a generator's own modules never reach the disk as text,
///    since `build/generators.rs` keeps the response in the cache.
/// 3. **The program came back.** The suite runs over the rewritten corpus, and
///    a red row is a source that parsed into something else.
///
/// One test rather than three because they share a driver build. The driver
/// is built once and then run over each source on its own, both passes back
/// to back, on the shared pool. A source's second pass waits on its first and
/// nothing else waits on anything, and the largest sources take the driver
/// most of a minute each — run as one manifest, the corpus took minutes.
#[test]
fn every_conformance_source_survives_parse_and_print() {
    let suite = Scratch::copy_of("round-trip", &tests_dir().join("conformance"));
    let driver = Scratch::copy_of("round-trip-driver", &tests_dir().join("ast_round_trip"));

    let files = sources(&suite.root);
    assert!(
        files.len() >= 100,
        "expected the conformance corpus, found {} source(s)",
        files.len()
    );

    driver.run(&["build", "//bin/roundtrip"]).ok();
    let artifact = driver.artifact("bin/roundtrip");
    let manifests: Vec<PathBuf> = files
        .iter()
        .enumerate()
        .map(|(i, p)| driver.write(&format!("manifests/{i}.txt"), &p.display().to_string()))
        .collect();

    let before: Vec<String> = files.iter().map(|p| read(p)).collect();
    // Largest first: the test lasts as long as its slowest source, so that one
    // should not wait behind a queue of small ones.
    let mut largest_first: Vec<usize> = (0..files.len()).collect();
    largest_first.sort_by_key(|&i| std::cmp::Reverse(before[i].len()));
    let answered = pool::map(&largest_first, |&i| {
        let first = drive(&driver, &artifact, &manifests[i]);
        let once = read(&files[i]);
        let second = (first.code == 0).then(|| {
            let run = drive(&driver, &artifact, &manifests[i]);
            (run, read(&files[i]))
        });
        (i, Passes { first, once, second })
    });
    let mut passes: Vec<Option<Passes>> = (0..files.len()).map(|_| None).collect();
    for (i, p) in answered {
        passes[i] = Some(p);
    }
    let passes: Vec<Passes> = passes.into_iter().map(|p| p.expect("every source ran")).collect();

    let refused: Vec<String> = passes
        .iter()
        .zip(&files)
        .filter(|(p, _)| p.first.code != 0)
        .map(|(p, source)| failed(source, &p.first))
        .collect();
    assert!(
        refused.is_empty(),
        "`core/buri/ast`'s `parse` refused {} source(s) the compiler accepts:\n{}",
        refused.len(),
        indent(&refused.join("\n"))
    );

    // A driver that read nothing, or wrote every file back unchanged, would
    // pass everything below without having proved anything.
    let moved = before.iter().zip(&passes).filter(|(a, p)| **a != p.once).count();
    assert!(
        moved >= files.len() / 2,
        "only {moved} of {} sources came back changed, so the driver did not run over them",
        files.len()
    );

    // Every first pass was clean, so every source had a second.
    let seconds: Vec<&(Run, String)> = passes.iter().filter_map(|p| p.second.as_ref()).collect();
    assert_eq!(seconds.len(), files.len());
    let reread: Vec<String> = seconds
        .iter()
        .zip(&files)
        .filter(|((run, _), _)| run.code != 0)
        .map(|((run, _), source)| failed(source, run))
        .collect();
    assert!(
        reread.is_empty(),
        "`parse` refused {} module(s) `print` wrote:\n{}",
        reread.len(),
        indent(&reread.join("\n"))
    );
    let drifted: Vec<String> = passes
        .iter()
        .zip(&seconds)
        .zip(&files)
        .filter(|((p, (_, twice)), _)| p.once != *twice)
        .map(|(_, source)| source.display().to_string())
        .collect();
    assert!(
        drifted.is_empty(),
        "{} module(s) are not a fixed point of parse-then-print:\n  {}",
        drifted.len(),
        drifted.join("\n  ")
    );

    let run = suite.run(&["test", "//...", "--force"]);
    run.ok();
    let passed = run.tests_passed();
    assert!(
        passed >= 1000,
        "the round-tripped suite holds {passed} assertions, so it did not run:\n{}",
        indent(&run.all())
    );
    eprintln!("round trip: {} sources, {passed} tests passed", files.len());
}
