# Guard phase 64 handoff

Phase 63 left 57 guards by the prompt's count (58 with definitions). See "Guard phase 63" in `guard-zero-plan.md`.

## Candidates

- `monomorphize.rs` `resolve_trait_call`'s "checking proved every effect call's context binds the effect" `or_ice`. The checker can't carry the binding's `FieldId`: in `fn f<C: Stdout>(ctx: C)` the receiver is `C` until monomorphization picks a context. Making it total needs the proof carried with the type arguments, or the call's receiver split out of `args` (its `if let Some(first)` is an uncovered branch too).
- `lower.rs` `continue_`'s "`tail_calls` writes a back edge only inside its loop". A `Loop` is always a whole function body, so the function could say it is a loop instead of `FnLower::loops` holding a stack.
- `lower.rs` `release_except`'s "a local `rc` releases is bound". Skipping an unbound local, as `rc` does for `Target::Local`, would be a silent default.
- `rtcall.rs` 410 "a scalar row answers one scalar", 1263 "float result is an F64", and its other invariants.
- Stencil `mod.rs` 621 root frame, `emit.rs` 1636, 1654 and 1848, and `jit.rs` 627 and 2182.
- llvm attribute and intrinsic lookups, `emit.rs` 4181 terminator, and `mod.rs` verify.
- llvm `boxed_payload` still builds a variant's field types with `variant_types`. `EnumHead::payloads` serves it once its callers hold an `EnumTy`.

Line numbers are as of phase 62's commit, so search for the message.

## Notes

- Removing covered branches lowers an area's fraction. Phase 63 removed two in `lower.rs` and added `memory/short_circuit.buri` to cover one in `rc.rs`.
- A new conformance file feeds the checking corpus sampler. Run `BURI_BLESS=1 cargo test -p buri --features backend-llvm --test checking` and commit what it rewrites.
- Build `buri` with `cargo build -p buri --features backend-llvm`. `--features buri/backend-llvm` leaves the binary without LLVM.
- The cache names objects by toolchain, so compare base and head by content hash, not path.
