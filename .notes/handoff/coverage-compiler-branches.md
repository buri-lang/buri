# Compiler branch coverage

Phase 1 covered five branches with e2e tests:

- lint.rs 845, 939: `lint --fix` on read-only files (`fix_refusals::a_fix_that_cannot_be_written_is_named_and_the_file_is_left_alone`).
- lint.rs 1267: a package subdirectory lint can't list (`rare_paths::a_directory_lint_cannot_list_holds_no_unused_source`).
- lint.rs 2747: a variant named in a body that didn't check (`linting/a_body_that_did_not_check_invents_no_unused_variant`).
- modules.rs 223: a tool rule with no `tool.buri` (last step of `generators/generator_diagnostics`).

Probe one branch without a full run: build an instrumented buri, run it with `LLVM_PROFILE_FILE`, merge with `llvm-profdata`, then `llvm-cov show -show-branches=count` on the file. The tools live in the nightly toolchain's `lib/rustlib/aarch64-apple-darwin/bin`.

## Unreachable, for a refactor phase

- `crates/project/src/languages/json/schema.rs` 1114, 1115, 1122, 1123, 1210, 1225, 1310, 1434, 1521, 1642, 1708: `Registry::new` walks every node and `$ref` target and refuses bad schemas, and `json/mod.rs:210` skips validation when it does. Validation re-checks what the registry already proved.
- `cli/src/compiler/driver.rs` 954 (`host_platform`): only a unit test calls it.
- `cli/src/compiler/modules.rs` 300 (`load_source`): no production caller.
- `cli/src/compiler/driver.rs` 892, 900: write failures inside a pid-named temp dir.
- `cli/src/compiler/driver.rs` 847: monomorphize errors after a clean check.
- `crates/diagnostics/src/profile.rs` 315, 360, 370: need the `alloc-counter` feature or hardware counters.
- `cli/src/main.rs` 64 and `arguments.rs` 321: need a panic other than a stdout print failure.
- link.rs 289, 292, 1044, 1675, 1898, 1904, 1920, 2124; actions.rs 2059, 2070; runtime_cross.rs 188; runtime_native.rs 100: branch on build constants (`musl::BAKED`, `runtime_native::AVAILABLE`, `runtime_src::AVAILABLE`). Not fully verified.
- code_actions.rs 225, 234, 273, 310, 377, 378: diagnostics with no code, or an empty path. Not verified.
- javascript.rs 2061, 2563, 2722, 2800, 2897, 2938, 2943: the generator only makes fresh names and never an empty `default`. Not verified.

## Leads

- test.rs 1751: a solo native suite with `--filter` that `left_out` lets through (needs a generated test module).
- test.rs 4802: a batch whose filter selects nothing.
- interrupt.rs 32: a second signal.
- watch.rs 479: passes reached.
- programs.rs 68, 82, 147: lock and symlink failures; see build_driver.rs.
- modules.rs 224 (a tool checked twice), 464 (no generator outcome), 120 (platform already set).

## UX notes, not fixed

- `lint --fix` prints "nothing to fix" after its writes fail.
- A missing `tool.buri` (like a missing `lib.buri`) is reported with no location, because `load_unit` passes `Span::NONE`. `lint //...` also repeats it inside `tool-failed`.
- Only `lint` reaches modules.rs 223. `build //cmd/app` loads the tool another way.

Local macOS run: 14874/15107 before, 14878/15107 after the first four tests (cli/lint 560 to 564 of 566). modules.rs 223 came after that run and was confirmed by probe.
