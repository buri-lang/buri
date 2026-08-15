//! What the backend emits, recorded.
//!
//! Every other suite asks whether a program *behaves*. This one asks what it
//! *compiles to*, because that is the only way an optimisation is visible: a
//! pass that removes an allocation or a call frame changes no answer anywhere,
//! so without a record of the output it lands unseen and regresses unnoticed.
//!
//! Each case in `tests/golden_js/` is one small program exercising one
//! construct, together with three recordings:
//!
//! * `expected.mjs` — the generated half of the debug artifact. The
//!   hand-written runtime is removed, because it is the same thousand lines in
//!   every case and it is not what any pass here changes.
//! * `expected.out` — what the program prints. A record of *output* with no
//!   record of *behaviour* would happily bless a miscompile, so every case runs,
//!   in both build modes, and the two must agree.
//! * `sizes.txt` — sizes for the whole corpus in one file, so the size effect
//!   of a change is a single reviewable diff rather than a number buried in
//!   each case. Two columns: the release artifact, which is what a user ships
//!   and is mostly runtime, and the generated code, which is what the backend
//!   emitted and where a pass either shows up or did not land.
//!
//! `BURI_BLESS=1 cargo test -p buri --test golden_js` records. Read the diff:
//! blessing without reading it is the one way this suite proves nothing.

mod harness;
use harness::*;

/// The generated half of an artifact.
///
/// The runtime is spliced in one declaration at a time (`codegen::generate`),
/// each verbatim in a debug build, so removing each declaration's own source
/// text leaves exactly what the backend produced. Dropping whichever
/// declarations dead-code elimination kept is order-independent and survives a
/// program reaching more or less of the runtime than its neighbours.
fn program_only(artifact: &str) -> String {
    let mut rest = artifact.to_string();
    for (_, src) in buri::js::split_declarations(buri::runtime_source()) {
        rest = rest.replacen(src.trim(), "", 1);
    }
    // The entry epilogue is the same fifteen lines of `try`/`catch` in every
    // case, modulo the name it calls, and nothing here changes it.
    let body: Vec<&str> = rest
        .lines()
        .filter(|l| !l.starts_with("try{const r="))
        .filter(|l| !l.starts_with("import{createRequire"))
        .filter(|l| !l.starts_with("const $require="))
        .collect();

    // Collapse the blank runs the removals left behind, so the record is the
    // program and nothing else.
    let mut out = String::new();
    let mut blank = true;
    for line in body {
        if line.trim().is_empty() {
            blank = true;
            continue;
        }
        if blank && !out.is_empty() {
            out.push('\n');
        }
        blank = false;
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Records what a program prints if nothing has been recorded yet, and
/// otherwise compares — whether or not this run is blessing.
///
/// The rest of this corpus is a record of *output* and is meant to move with
/// every pass. This is a record of *behaviour*, and a pass may not move it. A
/// `BURI_BLESS=1` that could rewrite it would turn the one check here that
/// catches a miscompile into a check that rubber-stamps one.
fn behaviour(g: &mut Golden, path: &std::path::Path, label: &str, actual: &str) {
    match std::fs::read_to_string(path) {
        Ok(recorded) if recorded == actual => {}
        Ok(recorded) => g.fail(format!(
            "{label}: the program prints something else now. This is a change in \
             behaviour, not in output, so it is never blessed — fix it, or delete \
             the file to re-record deliberately.\n  recorded:\n{}\n  printed:\n{}",
            indent(&recorded),
            indent(actual)
        )),
        Err(_) => std::fs::write(path, actual).unwrap(),
    }
}

#[test]
fn generated_javascript_matches_its_record() {
    let dir = tests_dir().join("golden_js");
    let cases = case_dirs(&dir, "main.buri", 15);

    let mut g = Golden::new();
    let mut sizes = String::new();
    let (mut total_release, mut total_generated) = (0usize, 0usize);

    for case in &cases {
        let name = case.file_name().unwrap().to_string_lossy().to_string();
        let source = std::fs::read_to_string(case.join("main.buri")).unwrap();

        let scratch = Scratch::repo(&format!("golden-js-{name}"));
        scratch.binary_package("cmd/x", &source);

        // Debug first: it is what `expected.mjs` records, and unmangled names
        // are what make the diff readable.
        scratch.run(&["build", "//cmd/x", "--force"]).ok();
        let debug = std::fs::read_to_string(scratch.artifact("cmd/x")).unwrap();
        let generated = program_only(&debug);
        g.check(
            &case.join("expected.mjs"),
            &format!("golden_js/{name}/expected.mjs"),
            &generated,
        );
        let debug_out = scratch.exec_js("cmd/x");
        debug_out.ok();

        scratch.run(&["build", "//cmd/x", "--release", "--force"]).ok();
        let release = std::fs::read_to_string(scratch.artifact("cmd/x")).unwrap();
        let release_out = scratch.exec_js("cmd/x");
        release_out.ok();

        // Per case, what `release_and_debug_agree` asserts for the suite as a
        // whole. Here it is what stops a blessed `expected.mjs` from recording
        // a program that stopped working.
        if debug_out.stdout != release_out.stdout {
            g.fail(format!(
                "{name}: release and debug print differently.\n  debug:\n{}\n  release:\n{}",
                indent(&debug_out.stdout),
                indent(&release_out.stdout)
            ));
        }
        // Recorded once and never re-recorded. `expected.mjs` is a record of
        // *how* a program compiles and is meant to move; `expected.out` is a
        // claim about what it computes, and blessing one of those would launder
        // exactly the failure this corpus exists to catch. It caught a
        // tail-call rebinding that read a parameter after overwriting it, and
        // would have recorded `4950` for a sum of `5050` had it been writable.
        // To change one deliberately, delete it and re-record.
        behaviour(
            &mut g,
            &case.join("expected.out"),
            &format!("golden_js/{name}/expected.out"),
            &debug_out.stdout,
        );

        if release.len() >= debug.len() {
            g.fail(format!(
                "{name}: the release artifact ({} bytes) is not smaller than the debug one ({} bytes)",
                release.len(),
                debug.len()
            ));
        }
        // Two numbers, because they answer different questions. The artifact
        // is what a user ships; most of it is the runtime, which no pass here
        // changes, so it moves slowly. The generated column is what the
        // backend actually emitted, and it is where a pass either shows up or
        // did not land.
        sizes.push_str(&format!(
            "{name}: {} bytes artifact, {} generated\n",
            release.len(),
            generated.len()
        ));
        total_release += release.len();
        total_generated += generated.len();
    }
    sizes.push_str(&format!(
        "\ntotal: {total_release} bytes artifact, {total_generated} generated\n"
    ));

    g.check(&dir.join("sizes.txt"), "golden_js/sizes.txt", &sizes);
    g.finish("golden-js", cases.len());
}
