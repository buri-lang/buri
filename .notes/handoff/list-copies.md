# List copies: where kvstore's runtime copies come from

Done (design/PERFORMANCE.md §6.107): `[x, ..rest]` handed on where its list dies
is `slice.owned` (`middle/slices.rs`, `buri_rt_list_slice_owned`).

Elements retained by runtime copies in a kvstore release run, before §6.107:

- `map_replace_at` 999 K, `map_insert_at` 255 K, `map_remove_at` 133 K:
  orderedmap path copies of a shared map.
- `list_push` 660 K, 149 K of it `propose`'s log push.
- `list_slice` 362 K, from `take`/`drop`, not `..rest`.
- `list_concat` 165 K, 126 K of it `mergeEntries`.

Most come from `cluster.arrive`: `cluster.nodes.get(envelope.to)` shares the
node (log, store, matched/next maps) with the map, and the insert uses
`target.id`, another expression. The `down` branch reads the whole map
between. So a compiler take-then-insert is unsound; it needs a way to move a
value out of a map in the language or library.

Not done:

- A batched retain-elements glue for `copy_retaining`, about 1% estimated.
- Owned `[x, ..rest]` parameters for non-tail recursion, such as
  `x + total(rest)`; each step still copies.
