# Guard phase 70 handoff

Phase 69 left 41 guards by the plan-rule count (definitions included), and 19 by the `ice(`/`unreachable!`/`.expect(`/`panic!` count in non-test compiler code. See "Guard phase 69" in `guard-zero-plan.md`.

## Candidates

- `jit.rs` `ent_mut`. Give it the whole phase, and land it in whole, green steps:
  1. Move each side table indexed by a value or a block onto `ir::PerValue` or `ir::PerBlock`, made with `Code::per_value` and `Code::per_block`. There are about 45 tables and 153 uses.
  2. Give the CSR tables (`at`, `next`, `items`) and the union-find tables typed ids.
  3. `ent_mut` is total once nothing indexes by a bare `usize`.
- `monomorphize.rs` `resolve_trait_call`, `lower.rs` `continue_` and `release_except`. Phase 65's handoff has the reasons.
- llvm attribute and intrinsic lookups, `emit.rs` terminator, `mod.rs` verify.

Search for the message; line numbers drift.

## Notes

- `semantics::resolve::Host` is the host an entry was checked with: its constructor and the constructor of each field. `Started` holds one, and is minted only when the host and every field are accepted. A handed entry with a host gets one too.
- `monomorphize::Handed` is `Hosted(&Started)` or `Bare(FnId)`, so a handed entry's host is never looked up again.
- Removing covered branches lowers an area that is below 100%. Before a refactor that deletes branches in `crates/middle`, find an uncovered branch a program can reach and cover it with an e2e test. The uncovered branches left there are mostly in `rc.rs`, `derives.rs` and `monomorphize.rs`; most are error paths or caches that can't be hit twice.
- To measure base coverage from a `git archive`, `git init` and commit it first: the corpus tests run `git ls-files`.
- `cli/src/build/programs.rs:147` is a racy branch, so `cli/build` can move by one branch between runs. Rerun before blaming a change.
- Run conformance through stencil: copy `cli/tests/conformance` without `.buri` and delete each `backends: [JS]`. Four suites fail to compile on stencil that way, on base and head alike.
- Build `buri` with `cargo build -p buri --features backend-llvm`, and compare objects by content hash. To build the base, `git archive staging` into a scratch directory with its own `CARGO_TARGET_DIR`.
