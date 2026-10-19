# Guard phase 63 handoff

Phase 62 left 58 guards by the prompt's count (59 with definitions). See "Guard phase 62" in `guard-zero-plan.md`.

## Candidates

- `lower.rs` `field_type` and `payload_type` default with `map_or(Ty::UNIT, ..)`, and `StructUpdate`'s base type with `_ => Ty::UNIT`. They could read `TypeList::field` and `EnumHead::payload`.
- `monomorphize.rs` `resolve_trait_call`'s "checking proved every effect call's context binds the effect" `or_ice`. The checker could carry the binding's `FieldId` on the call, as it now does on `CtxGet`.
- llvm `holds_only` and `Boxes::boxes` still build whole `field_types` and `variant_types` lists. `TypeList::fields` serves records.
- `rtcall.rs` 410 "a scalar row answers one scalar", 1263 "float result is an F64", and its other invariants.
- Stencil `mod.rs` 621 root frame, `emit.rs` 1636, 1654 and 1848, and `jit.rs` 627 and 2182.
- llvm attribute and intrinsic lookups, `emit.rs` 4181 terminator, and `mod.rs` verify.
- `lower.rs` 1236, 2079 and 2504.

Line numbers are as of phase 62's commit.

## Notes

- Build `buri` with `cargo build -p buri --features backend-llvm`. `--features buri/backend-llvm` leaves the binary without LLVM.
- Before comparing release objects, check that `buri test --release` in `cli/tests/example` reports no suite that failed to compile.
