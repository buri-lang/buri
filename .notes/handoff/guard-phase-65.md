# Guard phase 65 handoff

Phase 64 left 52 guards by the prompt's count (53 with definitions). See "Guard phase 64" in `guard-zero-plan.md`.

## Candidates

- `monomorphize.rs` `resolve_trait_call`'s "checking proved every effect call's context binds the effect". Making it total needs evidence passing: `Infer::satisfies` records which context field satisfies the bound, and every instantiation carries it to monomorphization. That's several phases. An `if let` fallback would add an uncovered branch and a misleading diagnostic.
- `monomorphize.rs` "a requested context was checked". CtxCall could carry the checked constructor, but `context A { ..A() }` compiles today with no diagnostic: the spread's error-typed CtxCall is dropped. Fix that first with a cycle diagnostic, an e2e test and a docs page.
- `monomorphize.rs` `derived_operator`'s "only a struct derives an arithmetic operator". It could move to the derived-impl path, where `con` is known, but that changes descriptor interning order and so the output.
- `lower.rs` `continue_`'s "`tail_calls` writes a back edge only inside its loop". It needs back edges split by type, or a loop header in every function, which changes IR and value ids.
- `lower.rs` `release_except`'s "a local `rc` releases is bound". The env is `Option` by design. `read()` already falls back to `Undef` and rc skips unbound `Target::Local`s, so skipping here would be one more silent default.
- `rtcall.rs`:
  - 410 "a scalar row answers one scalar" and 1263 "float result is an F64": F32, I128, Unit and multi-word aggregates have no C return shape. A typed runtime-row return would fix both.
  - 171 (row names every context), 353 (error has bytes), 575 (`crt` arity) and 1226 (aggregate is whole words) check the runtime table against layouts. A table built from typed signatures would make them total.
- Stencil `mod.rs` 621 root frame, `emit.rs` 1636, 1654 and 1848, and `jit.rs` 627 and 2182.
- llvm attribute and intrinsic lookups, `emit.rs` 4181 terminator, and `mod.rs` verify.

Search for the message; line numbers drift.

## Notes

- A private `DenseId` (see rc.rs `Pre`) makes a "pushed above" lookup total. Only `push` mints one.
- Removing covered branches lowers an area's fraction. Compare exact fractions, not percentages.
- A new conformance file feeds the checking corpus sampler. Run `BURI_BLESS=1 cargo test -p buri --features backend-llvm --test checking` and commit what it rewrites.
- Build `buri` with `cargo build -p buri --features backend-llvm`. `--features buri/backend-llvm` leaves the binary without LLVM.
- The cache names objects by toolchain, so compare base and head by content hash, not path.
