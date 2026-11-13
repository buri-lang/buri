# Check speed

Landed: the intern fast path, a single path resolution in `elaborate`, and `Prim::named` (PERFORMANCE.md §6.134). Check instructions fell 2.5%, check cost 1.1–1.3%. The check rows hold the VM's figures.

The +0.2% cost rise after 177a853ab was instruction-cache misses from code layout. Ir and data misses stayed flat. Fewer check instructions is the only lever.

What's left, from a line-table callgrind profile of `ci-count/check` at 9e5e03e1e (123M Ir):

- Allocation is about 14% of check Ir. DHAT's top check sites: `styles::Edges::add` (10k blocks, 2.8 MB), `Infer::call_fn`, `Flow::positions` (a `Vec<Pos>` per block, `if`, `match` and call), and `fresh_int`'s `vec![lit]`.
- `styles.rs` is 13% of `monorepo-ci` check cost: `Flow::edges`, `Extractor::alphas`, and misses reading `e.ty`/`e.kind`. Several whole-tree walks (icons, reactive, endless, styles, const cycles) each miss the cache; merging them saves misses.
- `check_icons_and_builders`' walk: 27.7M cost on `monorepo-ci`.
- `ModuleScope::name` hashes the string up to three times, about 3M Ir. `std`'s map has no stable `raw_entry`, so this needs a merged table or a hash-once map.
- `carries_effect` (2.3M) and `Name::new` (1.5M).

Tooling, in `/tmp/cs/` (gone after a reboot): `m.sh <rev>` counts the check group of both workspaces in a Lima VM; `dbg-in.sh` takes a line-table profile; `dhat.py` groups a DHAT profile by its first own frame.
