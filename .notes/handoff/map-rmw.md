# Map read-change-write without `update`/`take`

Phase 7, 2026-11-12. See PERFORMANCE.md §6.125: monomorphization rewrites
`let w = m.get(k).withDefault(d); ... m.insert(ctx, k, <uses w>)` into the
map module's private `upserted`, one descent that changes the value in place
(`crates/middle/src/compiler/middle/monomorphize/upsert.rs`).

- `run-dev/kvstore` went from 1,186.3 M to 1,163.2 M instructions (arm64 VM)
  and from 589,381 to 583,491 allocations. The soak output is unchanged.
- `cli/tests/repositories/middle/map_defaults` covers rewritten and left-alone
  shapes on both backends, with older copies held.
- It fixed `runtime.js`'s `$map_exchangeAt`, which didn't share the element it
  handed out of a copied list, and an `rc` leak when a `?` to the right of an
  already-made argument or field update fails (`held_until_taken`).
- Only repository code is rewritten. Rewriting the stdlib's `groupBy` gave
  `upserted` instances whose collision arm no key type there can reach, and
  `core/map`'s MC/DC fell to 94.6%.

Phase 6: `exchangeAt` swaps a placeholder into a child's slot (§6.123).

## What's left

About 38.7k list copies remain in kvstore's soak. `copies.sh` in the VM
breaks them down by call site.

- **Wider upserts.** The rewrite skips a change that reads two or more other
  locals, a `?` or lambda in the value, an insert under a lambda, and a map or
  key that isn't a name or a field of one. Pack extra locals into a tuple
  `arg` to widen it.
- **`compactAll`**: an `insert` inside `foldLeafCtx` over the map it also
  accumulates into. That sharing is inherent.
- **`popped` with `.None => m`** (`network.receive`). `m` stays alive through
  the call, so `pop` copies. kvstore doesn't reach it.
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
- Phase 7 deleted `maprmw` too. `~/.buri-agent/rmw7/run.sh <name>` benches
  `<name>.tar` from `~/.buri-agent/rmw/`; `same.sh <a> <b>` compares two
  saved soak outputs.
