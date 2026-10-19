//! The test processes `buri test` starts, under the Valgrind tool the
//! harness runs its own programs under (`valgrind.rs`, `BURI_TEST_RUNNER`).
//!
//! A shim with a memory error and a race stands in front of each test
//! process. Under a tool its errors reach `report`, which fails the workflow,
//! and without one the suite still runs through it.

use crate::hang::Capped;
use std::path::Path;
use std::process::Command;

#[test]
fn a_test_process_error_under_the_tool_is_reported() {
    crate::sweep::once();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-test-processes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let repo = dir.join("repo");
    std::fs::create_dir_all(repo.join("sums/test")).unwrap();
    std::fs::write(repo.join("REPO.buri"), "").unwrap();
    std::fs::write(
        repo.join("sums/BUILD.buri"),
        "library {\n    sources: [\"sums.buri\"]\n\n    test {\n        \
         sources: [\"test/sums_test.buri\"]\n    }\n}\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("sums/lib.buri"),
        "from \"//sums/sums.buri\" export { twice };\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("sums/sums.buri"),
        "export fn twice(n: Int): Int {\n    n * 2\n}\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("sums/test/sums_test.buri"),
        "from \"core/testing/assert\" import * as assert;\nfrom \"//sums\" import { twice };\n\n\
         test \"twice doubles\" {\n    assert.equal(twice(21), 42);\n}\n",
    )
    .unwrap();

    let shim = dir.join("shim");
    let compiled = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".into()))
        .args(["-O0", "-g", "-pthread", "-o"])
        .arg(&shim)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/native/test_processes/shim.c"
        ))
        .capped_output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "the shim didn't compile:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    let tool = crate::valgrind::chosen_tool();
    let reports = dir.join("reports");
    for (backend, flags) in crate::e2e::build_modes() {
        let ran_through = dir.join(format!("{backend}.ran"));
        let mut runner = tool
            .map(|t| crate::valgrind::runner(t, &reports, backend, "this test"))
            .unwrap_or_default();
        runner.push_str(&format!(" {} {}", shim.display(), ran_through.display()));
        let ran = crate::shared::ran_command(
            Command::new(env!("CARGO_BIN_EXE_buri"))
                .current_dir(&repo)
                .arg("test")
                .args(flags)
                .arg("//sums")
                .env(crate::valgrind::RUNNER, runner.trim()),
        );
        assert!(
            ran.status == 0 && ran.stdout.contains("1 passed, 0 failed"),
            "{backend}: the suite didn't pass through the shim:\n{}\n{}",
            ran.stdout,
            ran.stderr
        );
        assert!(
            std::fs::read_to_string(&ran_through).is_ok_and(|r| r.starts_with("ran")),
            "{backend}: `buri test` didn't start its test process through the runner"
        );
    }
    if tool.is_some() {
        assert!(
            crate::valgrind::report(&reports) > 0,
            "the shim's error under the tool wasn't reported"
        );
    }
}
