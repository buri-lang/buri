# Guard phase 72 handoff

Phase 71 left 38 guards by the plan rule (definitions included), and 19 by the `ice(`/`unreachable!`/`.expect(`/`panic!` count in non-test compiler code (`parser.rs`'s `self.expect(` and `json/suite.rs` excluded). See "Guard phase 71" in `guard-zero-plan.md`.

## Candidates

- `monomorphize.rs` `resolve_trait_call`, `lower.rs` `continue_` and `release_except`. Phase 65's handoff has the reasons.
- `jit.rs`'s own `ice`s: the stencil a key names (`None => ice(...)` in `emit_key`) and `HoleName::Slot`. Both need a type per stencil, or key indices the library build checks.
- Stdlib lookups outside phase 71's list: `monomorphize.rs` `members`' `IndexSet.toList` and `of`. A `KnownIndexSet` would have to be built after `register_conformance`, since methods are registered there, not in `register_known_names`.
- `emit.rs` `check_terminators` and `mod.rs` verify stay: LLVM crashes on a block with no terminator, and the verifier is the floor.

Search for the message; line numbers drift.

## Notes

- Every attribute kind and intrinsic the LLVM backend names is a row of `build::Known`. Add a row there, and to the destructuring in `Known::look_up`, rather than looking a name up where it's used.
- Branch coverage counts `if`, `if let`, `let else`, and the left side of each `&&`/`||`. It doesn't count `match` arms, `or_ice` or library calls such as `unwrap_or`. A match guard counts.
- To measure base coverage from a `git archive`, `git init` and commit it first: the corpus tests run `git ls-files`.
- Run conformance through both backends: copy `cli/tests/conformance` without `.buri` and delete each `backends: [JS]` line. `buri test` and `buri test --release` each print "4 failed to compile", on base and head alike.
- Objects sit in `.buri/cache` beside entries that name the toolchain. Compare only the Mach-O files (magic `cffaedfe`) by content hash.
- Build `buri` with `cargo build -p buri --features backend-llvm`. To build the base, `git archive staging` into a scratch directory with its own `CARGO_TARGET_DIR`.
