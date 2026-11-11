# Map read-change-write without `update`/`take`

Phase 3, 2026-11-10, landed `f76fb8ac2` and the commit after it.
See PERFORMANCE.md §6.117 to §6.119:

- an arm that binds a field of a record and hands the record on no longer
  holds the record
- `core/orderedmap`'s `remove` goes through `takeIn`
- `push`, `insertAt` and `replaceAt` own the item they store natively

`run-dev/kvstore` is at 1,209.2 M instructions and 589,381 allocations, down
from 1,276.9 M and 617,876 after phase 2.

## What's left

About 38.7k list copies remain in kvstore's soak. `copies.sh` in the VM
breaks them down by call site.

- **`network.send` / `request`**, the biggest. The queue is read with
  `get(..).withDefault(..)` and inserted back. Vacate's let-sourced form cost
  +3.5% instructions for 4k allocations, and sending through `take` was worse.
  Don't redo either. A placeholder write that costs no extra descent is the
  only open idea.
- **`core/map`'s `insert` of a key it holds**: no blocks, but about eight
  retain-and-release pairs a field of the value (344 instructions a field,
  against 46 for `core/orderedmap`, in `glue_counts.rs`). The inline `Entry`
  goes through `get`'s `Option`, the match and the answered tuple, and each
  copy retains it.
- **`compactAll`**: an `insert` inside `foldLeafCtx` over the map it also
  accumulates into. That sharing is inherent.
- **`mergeEntries`**: `slice_owned` copies, about 0.13% of instructions.
- **`popped` with `.None => m`** (`network.receive`). `m` stays alive through
  the call, so `pop` copies. One idea is case-of-case once `pop` is inlined.

## Dead ends

- Splitting an ordered map's node into halves that keep headroom (cut from
  the end) saved 20k allocations and no instructions.

## Traps

- The "copy" count in `copies.sh` also counts `give_back(null)` and frees of
  lists a removal emptied. Judge by instructions and allocations.
- Inlined stdlib methods that read `self` more than once count as more than
  one taking read of a field, so the field won't move. Take `self` apart once
  with `let`, as `insert` and `remove` do.
- Compiler coverage counts only `buri` processes. `cli/tests/native/*` tests
  compile in-process and add nothing, so reach each new branch from a
  repository case such as `cli/tests/repositories/middle/record_updates`,
  which runs `buri test` on both backends.
- `rc`'s `live` holds only counted names, so a check for a counted name on
  something read from it is a branch no test can take.
- Vacate passes a unit context to the early insert, and is off under
  `--coverage`, which keeps insert's context.
- Callgrind with `--separate-callers` over 20 runs out of memory on the soak.
- Helper scripts are in `~/.buri-agent/rmw/`. They ran in the `maprmw` VM,
  which is deleted; `setup.sh` makes a new one. `prof.sh <name>`,
  `copies.sh <name>`, `bless.sh <name>`, `two.sh <name>` and
  `flat.sh <name>` each work on `<name>.tar`, made with `git archive`.
