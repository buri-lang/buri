# Guard phase 71 handoff

Phase 70 left 40 guards by the plan rule (definitions included), and 19 by the `ice(`/`unreachable!`/`.expect(`/`panic!` count in non-test compiler code (`parser.rs`'s `self.expect(` and `json/suite.rs` excluded). See "Guard phase 70" in `guard-zero-plan.md`.

## Candidates

- `monomorphize.rs` `resolve_trait_call`, `lower.rs` `continue_` and `release_except`. Phase 65's handoff has the reasons.
- llvm attribute and intrinsic lookups, `emit.rs` terminator, `mod.rs` verify.
- `jit.rs`'s own `ice`s: the stencil a key names (`None => ice(...)` in `function`) and `HoleName::Slot`.

Search for the message; line numbers drift.

## Notes

- `ir::PerValue`, `PerBlock` and `PerType` index by a typed id. `Code::per_value_in` and `per_block_in` reuse a spare table's storage; `per_value_from` fills each row from its id.
- `ValueId::at` and `BlockId::at` are private to `ir.rs`, so a table outside it gets its ids from `iter`, `iter_mut` or the code.
- Branch coverage counts `if`, `if let`, `let else`, and the left side of each `&&`/`||`. It doesn't count `match` arms or library calls such as `unwrap_or`. A match guard counts.
- Removing covered branches lowers an area that is below 100%. Keep a real decision as an `if`/`if let` when a refactor would otherwise drop it.
- To measure base coverage from a `git archive`, `git init` and commit it first: the corpus tests run `git ls-files`.
- `cli/src/build/programs.rs:147` is a racy branch, so `cli/build` can move by one branch between runs.
- Run conformance through stencil: copy `cli/tests/conformance` without `.buri` and delete each `backends: [JS]`. One run prints "failed to compile", on base and head alike.
- Build `buri` with `cargo build -p buri --features backend-llvm`, and compare objects by content hash. To build the base, `git archive staging` into a scratch directory with its own `CARGO_TARGET_DIR`.
