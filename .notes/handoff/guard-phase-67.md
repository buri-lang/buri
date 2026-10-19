# Guard phase 67 handoff

Phase 66 left 45 guards by the plan-rule count (definitions included). See "Guard phase 66" in `guard-zero-plan.md`.

## Candidates

- `emit.rs` "the level has a branch stencil". `cond_key` answers an index now, so only `br/<loc>` can be missing, and only for `Loc::Imm`. `library::Known` could hold `br/f` and `br/r0..r2`, but `Imm` stays an arm nobody reaches. Prove a condition is never an immediate first, or give `cond` a location type without one.
- `mod.rs` `thread_door`'s root frame `or_ice`. `main` takes its host, so the frame has no parameters only because every host is zero-sized. llvm's `thread_door` doesn't check at all. A type that says "a root's frame takes nothing" needs the host's size from layout.
- `jit.rs` `ent_mut`. Every side table is a `Vec` indexed by `usize`; `Dense<ValueId, _>` and `Dense<BlockId, _>` would make it total, but that touches every analysis in `jit.rs`.
- `jit.rs` "a length is a class of its own", and `emit.rs` "an edge's register copies deadlock only on register sources".
- `monomorphize.rs` `resolve_trait_call`, `lower.rs` `continue_` and `release_except`. Phase 65's handoff has the reasons.
- llvm attribute and intrinsic lookups, `emit.rs` terminator, `mod.rs` verify.

Search for the message; line numbers drift.

## Notes

- `library::Known` is the pattern for a stencil the emitter writes unasked: add the key to `Known::KEYS` and a field, and `sources::assemble` refuses a library without it.
- `cli/src/build/programs.rs:147` is a racy branch, so `cli/build` can move by one branch between runs. Rerun before blaming a change.
- Run conformance through stencil: copy `cli/tests/conformance` without `.buri` and delete each `backends: [JS]`.
- Build `buri` with `cargo build -p buri --features backend-llvm`, and compare objects by content hash.
