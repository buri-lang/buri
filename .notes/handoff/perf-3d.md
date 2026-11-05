# Perf phase 3D handoff

I measured cross-program sharing of release units on a copy of
`~/workspace` and found too little to build for. Details are in
`design/PERFORMANCE.md` §6.89. No compiler change landed.

- Identical units across programs: 0.13% of IR bytes, 0.20% with closure
  numbers ignored. `Flights` in `cli/src/build/actions.rs` already dedupes a
  shared key.
- Cause: libraries are generic over each program's effect handler stack, so
  units hold different instantiations per program.
- `emit` split: 80 G glue pre-pass, 3,125 G lowering plus `default<O2>`,
  3,770 G instruction selection.
- Next lever, if wanted: one body for instantiations that differ only in a
  handler type they don't touch (up to 42% of bytes). It changes binaries, so
  it needs its own decision.
- Instruction selection is half of release `emit`. perf-2i's note on huge
  basic blocks and `insertvalue`/`extractvalue` chains is the cheaper target.
