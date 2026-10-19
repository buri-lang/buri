# Guard phase 66 handoff

Phase 65 left 46 guards by the plan-rule count (definitions included). See "Guard phase 65" in `guard-zero-plan.md`.

## Candidates

- Stencil library lookups:
  - `emit.rs` "an arm key names a stencil" and "the level has a branch stencil";
  - `mod.rs` root frame `or_ice`;
  - `jit.rs` `ent_mut` and "a length is a class of its own";
  - `emit.rs` "an edge's register copies deadlock only on register sources".

  The first two need key indices that the library build checks, so a key the emitter names can't be missing. Phase 65 didn't start on any of these.
- `rtcall.rs` `narrow`'s `ice("a tag destination is an enum")`. An `Answer::Int(Narrow::I32)` row could carry the destination's enum the way `Answer::Sum` does.
- `monomorphize.rs` `resolve_trait_call`, and `lower.rs` `continue_` and `release_except`. Phase 65's handoff has the reasons; they're unchanged.
- llvm attribute and intrinsic lookups, `emit.rs` 4181 terminator, and `mod.rs` verify.

Search for the message; line numbers drift.

## Notes

- `cli/src/build/programs.rs:147` is a racy branch: `try_lock` succeeds but the entry is no longer stale. Base covered it on one local run and head didn't, with no change to that file, so `cli/build` can move by one branch from run to run. Rerun coverage before you blame a change for it.
- No stencil branch below 100% is known to be unreachable, so a phase that removes covered stencil branches must also cover a real case. `jit.rs` `copyless_test`'s `!frame(*p)` never fires, including on the conformance corpus run through stencil, because no promoted header reaches it. Proving that needs `promote`'s header shape.
- Run conformance through stencil: copy `cli/tests/conformance` without `.buri` and delete each `backends: [JS]`. Four packages don't compile, because stencil lacks `math.sin` and a few others.
- Removing covered branches lowers an area's fraction. Compare exact fractions, not percentages.
- Build `buri` with `cargo build -p buri --features backend-llvm`.
- The cache names objects by toolchain, so compare base and head objects by content hash, not path. A JS test file's `$t.seed` changes on every run.
