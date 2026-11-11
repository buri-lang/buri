# Map read-change-write without `update`/`take`

Phase 4, 2026-11-11, landed `75fad0cc4`. See
PERFORMANCE.md §6.120: `core/map`'s `insert` and `remove` hold the child's
place with an empty branch instead of shifting the children.

- An insert of a key a 64-key map holds went from 7,255 to 3,003
  instructions with a value of eight `Str`s, and from 2,447 to 1,442 with
  one (arm64 VM).
- `glue_counts.rs`'s cost a field went from 344 to 209 (stencil) and from
  321 to 192 (LLVM).
- `run-dev/kvstore` didn't change: 1,209.2 M instructions and 589,381
  allocations. It uses no `core/map`.

## What's left

About 38.7k list copies remain in kvstore's soak. `copies.sh` in the VM
breaks them down by call site.

- **`list.get` retains its element.** A prototype that rewrites each call of
  the `list.get` intrinsic into `ExprKind::Index` after `vacate`, so `rc`
  lends it, cut kvstore by 0.12% instructions (1,207.8 M) with the same
  allocations. It's in `~/.buri-agent/rmw4/lend.patch`. Write it with
  `expect` rather than `?` or `if`, so it adds no branch a test can't take,
  and keep `list_get`'s lowering for function values. It needs a
  `collection_costs.rs` guard: `get` against `xs[i]` on records of `Str`s.
  This also covers `mergeEntries`' `log.get(i).map(..)`.
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
