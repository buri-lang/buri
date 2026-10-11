//! The middle end's corners: repositories whose programs reach the parts of
//! monomorphization, the tree passes and lowering no other suite reaches
//! through `buri`.
//!
//! Each case builds natively in debug mode and on JavaScript, so the two agree
//! on every block. See `harness/case.rs` for the format.
//!
//! ```text
//! BURI_BLESS=1 cargo test -p buri --test build middle::   # record the goldens
//! ```
use crate::harness::*;

#[test]
fn the_middle_end() {
    run_corpus(&tests_dir().join("repositories/middle"), "middle", 14);
}

/// Calls each entry of `repositories/middle/entries_without_a_host`.
const DRIVE: &str = r#"
const out = ".buri/out/platform/bare/cmd/app";
const { default: pick } = await import(`./${out}/pick.mjs`);
const { default: greet } = await import(`./${out}/greet.mjs`);
const { default: tag } = await import(`./${out}/tag.mjs`);
console.log(String(await pick(5n, 1n)), String(await pick(undefined, 1n)));
console.log(await greet({ name: "Ada" }, "hello"), await greet({ name: "Grace" }, "hi"));
console.log(String(await tag({ tags: ["a", "b"] }, "xyz")));
"#;

/// An entry whose first parameter is no host takes every argument it is
/// called with and answers what its body does, whether that parameter is an
/// `Option` or a struct another platform declares.
#[test]
fn an_entry_without_a_host_answers_its_caller() {
    let scratch = Scratch::copy_of(
        "middle-no-host",
        &tests_dir().join("repositories/middle/entries_without_a_host/repo"),
    );
    scratch.run(&["build", "//cmd/app"]).ok();
    scratch.write("drive.mjs", DRIVE);
    let out = std::process::Command::new(js_runtime())
        .arg(scratch.path("drive.mjs"))
        .current_dir(&scratch.root)
        .output()
        .unwrap();
    assert!(out.status.success(), "an entry failed:\n{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "5 1\nhello Grace\n5\n");
}
