# Map read-change-write without `update`/`take`

Phase 2, 2026-11-10, `0de36f448`: a field changed in every arm of a match
still moves.
See PERFORMANCE.md §6.116. `rc::lent_reads` follows each path through an `if`
or `match`, and both maps' `remove` take `self` apart once.

`run-dev/kvstore` is at 1,276.9 M instructions and 617,876 allocations, down
from 1,334.8 M and 702,632.

## What's left, ranked by copies in kvstore's soak

About 66.6k map copies remain. `copies.sh` in the VM breaks them down by call
site.

- **`network.send` / `request`** (~25k copies, the biggest). The queue is read
  with `get(..).withDefault(..)` and inserted back. Vacate's let-sourced form
  cost +3.5% instructions for 4k allocations, and sending through `take` was
  worse (800k allocations). Don't redo either. A placeholder write that costs
  no extra descent is the only open idea.
- **`applyCommitted`'s `removeFrom`** (~5k). `remove` copies the path even in
  a tree nothing else holds. Asking `holds` first cost +0.18%.
- **`compactAll`** (~2k): an `insert` inside `foldLeafCtx`.
- **`mergeEntries`** (~2.5k): `slice_owned` copies.
- **`popped` with `.None => m`** (`network.receive`). `m` stays alive through
  the call, so `pop` copies. One idea is case-of-case once `pop` is inlined.

## Traps

- Inlined stdlib methods that read `self` more than once count as more than
  one taking read of a field, so the field won't move. Take `self` apart once
  with `let`, as `insert` and `remove` do.
- Compiler coverage counts only `buri` processes. `cli/tests/native/*` tests
  compile in-process and add nothing, so reach each new branch from a
  repository case such as `cli/tests/repositories/middle/record_updates`,
  which runs `buri test` on both backends.
- Vacate passes a unit context to the early insert, and is off under
  `--coverage`, which keeps insert's context.
- Callgrind with `--separate-callers` over 20 runs out of memory on the soak.
- Helper scripts are in `~/.buri-agent/rmw/`. In the `maprmw` VM:
  `prof.sh <name>`, `copies.sh <name>` and `bless.sh <name>`, each on
  `<name>.tar` made with `git archive`.
