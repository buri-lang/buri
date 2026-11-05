# Perf phase 2I handoff

## What landed

`llvm: a release program's glue is emitted once, in its own unit`. Each
release unit only declares the drop and copy glue it calls. One more unit,
`buri.glue`, defines it. See `design/PERFORMANCE.md` §6.88.

## Phase breakdown

Cold `buri build --release //...` of a copy of `~/workspace`,
`BURI_PROFILE=1`, instructions summed over threads:

| Phase | Before | After |
|---|---:|---:|
| lex+parse | 1.9 G | 1.9 G |
| check | 25.9 G | 24.1 G |
| monomorphize | 3.0 G | 3.0 G |
| middle | 9.6 G | 9.6 G |
| emit (IR build, LLVM `default<O2>`, codegen) | 8,855 G | 6,851 G |
| link | 2.1 G | 2.1 G |
| other | 24.4 G | 25.2 G |

`emit` is still 99% of the build. Objects compiled went from 1,996 to 2,028,
one `buri.glue` per release program.

## Remaining ideas

- `buri.glue`'s key hashes every unit's key, so any edit re-emits it. Keying
  it on the sorted glue names it defines would let an incremental build keep
  it.
- Phase 1 lowers every unit unoptimized only to collect the glue they ask
  for. On a cold build that IR could be kept and optimized instead of built
  twice; it costs memory per unit.
- In `emit`, `MachineScheduler` and instruction selection on very large
  basic blocks dominate the backend samples. Smaller blocks from lowering, or
  fewer `insertvalue`/`extractvalue` chains, would cut both.
- Every program still optimizes the standard library units it uses. The
  codegen cache shares them only when the key matches across programs.
