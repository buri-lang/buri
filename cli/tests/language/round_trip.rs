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

/// Every `.buri` source under a tree, in a stable order. `BUILD.buri` and
/// `REPO.buri` are build files rather than Buri sources, and `.buri/` is the
/// cache directory a build leaves behind.
fn sources(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
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
/// One test rather than three because they share a driver build, and building
/// the driver is most of what this costs.
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
    let manifest: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
    let manifest_path = driver.write("manifest.txt", &manifest.join("\n"));
    let listed = manifest_path.display().to_string();

    let read_all =
        || -> Vec<String> { files.iter().map(|p| std::fs::read_to_string(p).unwrap()).collect() };

    let before = read_all();
    let first = driver.run(&["run", "//bin/roundtrip", "--", &listed]);
    assert_eq!(
        first.code,
        0,
        "`core/buri/ast`'s `parse` refused a source the compiler accepts:\n{}",
        indent(&first.all())
    );
    let once = read_all();

    // A driver that read nothing, or wrote every file back unchanged, would
    // pass everything below without having proved anything.
    let moved = before.iter().zip(&once).filter(|(a, b)| a != b).count();
    assert!(
        moved >= files.len() / 2,
        "only {moved} of {} sources came back changed, so the driver did not run over them",
        files.len()
    );

    let second = driver.run(&["run", "//bin/roundtrip", "--", &listed]);
    assert_eq!(
        second.code,
        0,
        "`parse` refused a module `print` wrote:\n{}",
        indent(&second.all())
    );
    let twice = read_all();
    let drifted: Vec<String> = once
        .iter()
        .zip(&twice)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| files[i].display().to_string())
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
