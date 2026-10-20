# Compiler branch coverage

Phase 1 covered five branches with e2e tests:

- lint.rs 845, 939: `lint --fix` on read-only files (`fix_refusals::a_fix_that_cannot_be_written_is_named_and_the_file_is_left_alone`).
- lint.rs 1267: a package subdirectory lint can't list (`rare_paths::a_directory_lint_cannot_list_holds_no_unused_source`).
- lint.rs 2747: a variant named in a body that didn't check (`linting/a_body_that_did_not_check_invents_no_unused_variant`).
- modules.rs 223: a tool rule with no `tool.buri` (last step of `generators/generator_diagnostics`).

Probe one branch without a full run: build an instrumented buri, run it with `LLVM_PROFILE_FILE`, merge with `llvm-profdata`, then `llvm-cov show -show-branches=count` on the file. The tools live in the nightly toolchain's `lib/rustlib/aarch64-apple-darwin/bin`.

## Refactor phase 1 (cbed93de6 and on)

- json/schema.rs (all 11): `Registry::new` records each keyword as it read it (`Kind`: the resolved reference, the bound, the type names, the compiled pattern, the entries). The checker matches on that and re-reads nothing. The one new decision, `let Some(target) = reg.targets.get(..)` near line 416, is reached both ways: by any schema with an unresolved `$ref`.
- driver.rs `host_platform` and modules.rs `load_source`: deleted. Only tests called them.
- driver.rs 892, 900: the module and its chunks are written in one loop. One write-failure branch is left, near line 895.
- driver.rs 847: reachable. `command_line::a_documented_failure_found_only_by_compiling_is_the_one_claimed` covers it, with a `fail code=derive-operator-not-numeric` block, which the checker passes and monomorphize refuses. A probe confirmed it.
- code_actions.rs 225: the code is looked up among the asked-for codes, so an uncoded finding is just one nobody asked about.
- code_actions.rs 377, 378: `package_of` is gone. A regenerable finding points at its package's build file or at an import in one of its sources, so `owning_package` of that file is the package. Its `let Some(package)` near line 275 is still unreached.

## Refactor phase 2

- code_actions.rs 275: `Linted` carries its `target`, so the package is `linted.target.package` and there is no lookup left to fail.
- code_actions.rs 233, 312: removed. Lint edits are never in generated (pathless) files.
- The UX gap behind them is fixed: a generated module importing a library its rule doesn't list now gets the `buri gen` fix in the editor, as `lint --fix` gives it. Test: `lsp/a_generated_import_missing_a_dependency_has_the_gen_fix` (its golden was `[]` before).
- javascript.rs `to_switch`: always emits a `default`; an empty one does nothing, as the `else`-less chain did.
- driver.rs 895: no pid directory any more. The doctest module is `temp_dir()/buri-doctest-<pid>-<stem>.mjs` with its chunks beside it, written in one loop with one failure message.

## Unreachable, for a refactor phase

- interrupt.rs 32: a second signal. `serving::a_second_signal_stops_the_server_at_once` reaches it, but a process a signal kills writes no profile. Resetting `SIG_DFL` in the handler instead was reverted: a signal landing on another thread before the reset runs the handler again, so the process keeps going. Lead: continuous-mode profiles (`%c`) in the coverage harness.
- profile.rs 315, 370: the allocations column, only in a `--features alloc-counter` build. Moving it behind a `cfg` was reverted: that hides the decision from coverage.
- profile.rs 360: hardware counters. It is covered on real hardware and not on CI's VMs, so it was left alone.
- `cli/src/main.rs` 64 and `arguments.rs` 321: need a panic other than a stdout print failure.
- link.rs 289, 292, 1044, 1675, 1898, 1904, 1920, 2124; actions.rs 2059, 2070; runtime_cross.rs 188; runtime_native.rs 100: branch on build constants (`musl::BAKED`, `runtime_native::AVAILABLE`, `runtime_src::AVAILABLE`). Not fully verified.
- javascript.rs 2061, 2563, 2722, 2800, 2897, 2938, 2943 and the other fresh-name branches: the generator only makes fresh names. Not verified.
- watch.rs 479: `passes` is `Some` only in the unit tests. Removing it needs another way for `an_edit_while_sweeping_triggers_the_next_pass` to stop after its second pass. An empty set is never adopted, so returning no inputs does not end the loop.

## Leads

- test.rs 1751, 4802: need `left_out` to return `None` while no test matches. Escaped test names and a generated module shadowing a test source both failed: generators aren't registered when `left_out` runs.
- programs.rs 29, 68, 82: lock and symlink failures; see build_driver.rs.
- modules.rs 124, 245, 520; lint.rs 960; lint_cache.rs 247; spawn.rs 298; cache.rs 423; language_server/mod.rs 130, 140, 269.

## UX notes, not fixed

- `lint --fix` prints "nothing to fix" after its writes fail.
- A missing `tool.buri` (like a missing `lib.buri`) is reported with no location, because `load_unit` passes `Span::NONE`. `lint //...` also repeats it inside `tool-failed`.
- Only `lint` reaches modules.rs 223. `build //cmd/app` loads the tool another way.

Local macOS run: 14874/15107 before, 14878/15107 after the first four tests (cli/lint 560 to 564 of 566). After refactor phase 1: 14897/15105, with json/schema.rs fully covered. After refactor phase 2: 14889/15096 by lcov sums (215 uncovered down to 207; the tool reported 14897/15105 before). Reverting interrupt.rs 32 and profile.rs 315, 370 puts three back: 14889/15099.
