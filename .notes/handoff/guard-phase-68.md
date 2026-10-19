# Guard phase 68 handoff

Phase 67 left 42 guards by the plan-rule count (definitions included), and 19 by the `ice(`/`unreachable!`/`.expect(`/`panic!` count in non-test compiler code. See "Guard phase 67" in `guard-zero-plan.md`.

## Candidates

- `mod.rs` `thread_door`'s "a root has a frame and takes no arguments". The old premise was wrong: a root's wrapper (`monomorphize::entry`) never takes the host, so the host's size doesn't matter. A native root is always nullary because `check_entry_signature` makes `main` take only its host, `buildfile.rs` refuses a `js` file on a native entry (`js-outside-js-entry`), and tests take nothing. Removing the guard means the IR carries a nullary root by type (`ProgramRoots` or the wrapper's signature). llvm's `thread_door` would read the same type, and today it assumes it without a check.
- `jit.rs` `ent_mut`. Every side table is a `Vec` indexed by `usize`; `Dense<ValueId, _>` and `Dense<BlockId, _>` would make it total, but that touches every analysis in `jit.rs`. Give it a whole phase.
- `monomorphize.rs` `resolve_trait_call`, `lower.rs` `continue_` and `release_except`. Phase 65's handoff has the reasons.
- llvm attribute and intrinsic lookups, `emit.rs` terminator, `mod.rs` verify.

Search for the message; line numbers drift.

## Notes

- `abi::Reg` is the register type. `Reg::ALL` lists the three, and `Fn2::read_reg` answers where a value is read from when it isn't a frame slot.
- `library::Known` is the pattern for a stencil the emitter writes unasked: add the key to `Known::KEYS` and a field, and `sources::assemble` refuses a library without it.
- `cli/src/build/programs.rs:147` is a racy branch, so `cli/build` can move by one branch between runs. Rerun before blaming a change.
- Run conformance through stencil: copy `cli/tests/conformance` without `.buri` and delete each `backends: [JS]`.
- Build `buri` with `cargo build -p buri --features backend-llvm`, and compare objects by content hash.
