//! The JavaScript backend's corners: repositories whose programs reach the
//! parts of the generator and the minifier no other suite reaches through
//! `buri`.
//!
//! A test bundle skips the minifier, so each case runs its programs with
//! `buri run`, once in debug and once with `--release`, which also mangles.
//! See `harness/case.rs` for the format.
//!
//! ```text
//! BURI_BLESS=1 cargo test -p buri --test build javascript::   # record the goldens
//! ```
use crate::harness::hang::Capped;
use crate::harness::*;

#[test]
fn the_javascript_backend() {
    run_corpus(
        &tests_dir().join("repositories/javascript"),
        "javascript",
        1,
    );
}

/// What the driver prints: one `status body` line per call to the entry.
const DRIVE: &str = r#"
import handle from "./.buri/out/platform/batch/cmd/batch/handle.mjs";
const r = (path) => new Request(`https://batch.example${path}`);
for (const [batch, extra, sizes] of [
  [[r("/a"), r("/b")], r("/c"), [1n, 2n, 3n]],
  [[], undefined, []],
  [[r("/only")], null, [40n]],
]) {
  const response = await handle(batch, extra, sizes);
  console.log(`${response.status} ${await response.text()}`);
}
"#;

/// `repositories/javascript/crossing_arguments`: the entry reads each request
/// in a list and an optional one, whether that is absent as `undefined` or as
/// `null`, takes numbers that are bigints on the caller's side, and reaches
/// the file system from inside the artifact.
#[test]
fn an_entry_reads_its_arguments_across_the_boundary() {
    let scratch = Scratch::copy_of(
        "js-crossing",
        &tests_dir().join("repositories/javascript/crossing_arguments/repo"),
    );
    scratch.run(&["build", "//cmd/batch"]).ok();
    scratch.write("drive.mjs", DRIVE);
    let out = std::process::Command::new(js_runtime())
        .arg(scratch.path("drive.mjs"))
        .current_dir(&scratch.root)
        .capped_output()
        .unwrap();
    assert!(
        out.status.success(),
        "the entry failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "200 /a,/b /c 6 true\n200  none 0 true\n200 /only none 40 true\n"
    );
}
