# Guard phase 69 handoff

Phase 68 left 41 guards by the plan-rule count (definitions included), and 19 by the `ice(`/`unreachable!`/`.expect(`/`panic!` count in non-test compiler code. See "Guard phase 68" in `guard-zero-plan.md`.

## Candidates

- `jit.rs` `ent_mut`. Every side table is a `Vec` indexed by `usize`; `Dense<ValueId, _>` and `Dense<BlockId, _>` would make it total, but that touches every analysis in `jit.rs`. Give it a whole phase.
- `monomorphize.rs` `resolve_trait_call`, `lower.rs` `continue_` and `release_except`. Phase 65's handoff has the reasons.
- llvm attribute and intrinsic lookups, `emit.rs` terminator, `mod.rs` verify.

Search for the message; line numbers drift.

## Leftovers from phase 68

- `Checked.entry` is unused outside tests now; `Checked.started` replaced it. Removing it removes a covered `if name == "main"` in `resolve.rs`, which lowers the semantics fraction only if that branch was the last one keeping it whole. Check before deleting.
- `monomorphize::host_value` keeps its `_ => ExprKind::Error` arm for a host field it can't build. It isn't a guard, but it's a silent default worth a look.
- `monomorphize::host_parameter` is reached only by entries a `js` file calls (`Roots::Handed`).

## Notes

- `monomorphize::Root` is a function that takes nothing. Only `Root::push` (from a `Nullary`) and test requests mint one. `ir::Roots` and both native backends carry it, so `thread_door` reads the frame without a check.
- `semantics::resolve::Started` is an entry whose host was accepted. `monomorphize::Roots::Main` takes one; `Roots::Handed` is an entry a `js` file calls, and its export lives in `Hosted.export`.
- `cli/src/build/programs.rs:147` is a racy branch, so `cli/build` can move by one branch between runs. Rerun before blaming a change.
- Run conformance through stencil: copy `cli/tests/conformance` without `.buri` and delete each `backends: [JS]`.
- Build `buri` with `cargo build -p buri --features backend-llvm`, and compare objects by content hash.
