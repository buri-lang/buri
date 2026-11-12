# Map read-change-write without `update`/`take`

Phase 8, 2026-11-12, finished. See PERFORMANCE.md §6.127. `monomorphize/upsert.rs`
now also rewrites a read inside the stored value, a change reading two or more
names (packed into a tuple), and a `let` worked out from `m.get(k)` some other
way into the map's private `remade`.

- `run-dev/kvstore` went from 1,163.2 M to 1,130.0 M instructions (arm64 VM).
  Allocations stay at 583,491. The soak output is unchanged.
- `cli/tests/repositories/middle/map_defaults` covers each shape in
  `test/inline.buri` and `test/remade.buri`. The conformance map tests cover
  every branch of each `remade` instance.

Phase 7 added `upserted` (§6.125); phase 6 `exchangeAt` (§6.123).

## What's left

Nothing in this line of work is worth its complexity now.

- **`compactAll`**: about 1.3% of kvstore at most. Its `insert` runs inside
  `foldLeafCtx` over the map it also accumulates into, so the sharing is
  inherent.
- **`popped` with `.None => m`** (`network.receive`). kvstore doesn't reach it.
- `core/orderedmap`'s `insertAt` and `removeAt` pay musl's memmove, about
  2,100 instructions on a full node. A runtime copy loop for short moves
  might help.

## Dead ends

- Splitting an ordered map's node into halves that keep headroom (cut from
  the end) saved 20k allocations and no instructions.
- `holdsEntry` in `core/map`: `insertNode`'s same-key arm and parts of the
  descent became unreachable for small maps of some key types. The stdlib
  MC/DC gate counts each instance, so core/map fell from 95.6% to 92.2%.
  Dropping the arm, `withDefault`, and a flags test with `Access` keys each
  moved the gap elsewhere or made it worse.

## Traps

- The stdlib MC/DC gate counts conditions per instance. A branch that one
  instance can't take both ways lowers the module even when another instance
  takes it. `~/.buri-agent/rmw4/unshown.py <module>` lists the gaps from
  `target/stdlib-coverage/*/.buri/coverage/lcov.info`.
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
- Helper scripts are in `~/.buri-agent/rmw/` and `~/.buri-agent/rmw4/`. They
  ran in the `maprmw` VM, which is deleted; `setup.sh` makes a new one.
  `prof.sh <name>`, `copies.sh <name>`, `bless.sh <name>`, `two.sh <name>`
  and `flat.sh <name>` each work on `<name>.tar`, made with `git archive`.
  `bench2.sh <name> <warm-from>` runs the kvstore bench with a heap report.
- Phase 6 deleted the `maprmw` VM; `setup.sh` makes a new one.
- Phase 5 remade the `maprmw` VM. `~/.buri-agent/rmw5/run.sh <name>`
  benches `<name>.tar` and prints the kvstore row and the heap report.
- Phase 8 deleted `maprmw` too; its scripts are in `~/.buri-agent/rmw8/`.
- Phase 7 deleted `maprmw` too. `~/.buri-agent/rmw7/run.sh <name>` benches
  `<name>.tar` from `~/.buri-agent/rmw/`; `same.sh <a> <b>` compares two
  saved soak outputs.
