//! `BURI_PROFILE=1`: one run says where its work went, by phase.
use crate::harness::*;

fn repo() -> Scratch {
    let scratch = Scratch::repo("profile");
    scratch.write("lib/a/BUILD.buri", "library {\n  test { sources: [\"test/a.buri\"] }\n}\n");
    scratch.write("lib/a/lib.buri", "export fn answer(): I64 { 42 }\n");
    scratch.write(
        "lib/a/test/a.buri",
        "from \"//lib/a\" import { answer };\n\
         from \"core/testing/assert\" import * as assert;\n\
         \ntest \"a answers\" {\n  assert.equal(answer(), 42);\n}\n",
    );
    scratch
}

/// A native `buri test` passes through every phase, and the report names each.
#[test]
fn a_profiled_test_run_names_every_phase_it_went_through() {
    let scratch = repo();
    let run = scratch.run_with_env(&["test", "//..."], &[("BURI_PROFILE", "1")]);
    run.ok().says("buri profile");
    for phase in ["lex+parse", "check", "monomorphize", "middle", "emit", "link", "run", "all phases"] {
        run.says(&format!("\n  {phase} "));
    }
    assert_eq!(run.tests_passed(), 1, "the suite did not run:\n{}", indent(&run.all()));
}

#[test]
fn an_unprofiled_run_prints_no_profile() {
    let scratch = repo();
    scratch.run(&["test", "//..."]).ok().silent_about("buri profile");
    scratch.run_with_env(&["test", "//..."], &[("BURI_PROFILE", "0")]).ok().silent_about("buri profile");
}
