# Map read-change-write without `update`/`take`

Phase 5, 2026-11-12. See PERFORMANCE.md §6.121: `lower::lend_gets` makes
each call of `list.get` an `xs[i]`, so `rc` lends the element.

- `run-dev/kvstore` went from 1,209.2 M to 1,199.8 M instructions (arm64
  VM), with 589,381 allocations before and after.
- `glue_counts.rs`'s `core/map` cost a field went from 209 to 201 (stencil)
  and from 192 to 184 (LLVM).
- Lending exposed an `rc` bug: `xs[xs.length() - 1]` released `xs` before
  the load. Fixed in the same landing, with
  `cli/tests/repositories/middle/index_reads_its_base`.
- It also exposed a `pairs` bug: an inlined `xs[i]` merged into a block
  parameter lost its retain across the list's release, as in
  `list.last().show(ctx)`. `pairs::Paths::within` fixes it;
  `cli/tests/repositories/middle/elements_outlive_their_list` covers it.
- `match_moves.rs`'s walks now match on what `element` builds, since a
  match on `get` builds nothing any more.

## What's left

About 38.7k list copies remain in kvstore's soak. `copies.sh` in the VM
breaks them down by call site.

- **`network.send` / `request`.** The copy is about 1% of instructions and
  an extra descent costs about 3%, so a fix must not descend twice. Vacate's
  let-sourced form and sending through `take` both failed. Don't redo them.
- **`core/map`'s same-key insert, second half.** Reading the held entry with
  a lent `children[idx]` and replacing it in place cut the field cost to 168.
  It needs a form that keeps per-instance MC/DC (see Dead ends).
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
- Phase 5 remade the `maprmw` VM. `~/.buri-agent/rmw5/run.sh <name>`
  benches `<name>.tar` and prints the kvstore row and the heap report.
