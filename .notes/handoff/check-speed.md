# Check speed

Landed:

- §6.134: the intern fast path, a single path resolution in `elaborate`, and `Prim::named`.
- §6.135: the style pass walks each declaration once to read it.
- §6.136: the icon and decorative-stack checks ride on `Flow::edges` (an `Also` callback, struct literals and updates only), so their own walk runs only where extraction draws no graph. Check instructions fell 1.6–1.9%, cost 1.3–2.7%.

The check rows hold the VM's figures. CI's check figures matched the VM's within 0.05%. CI's dev-codegen rows read higher than the VM's, so leave those to CI's figures.

A callback at every expression cost more instructions than the walk it replaced. Filter by expression kind before any dynamic call on the hot walk.

What's left, from a callgrind profile of the test build's check at e6a4fbe60 (477M Ir, about 783M cost):

- Restyle (`Checked::restyle`, once per program): about 55M Ir. `Flow::of` is 29.6M of it, spread over `Demand` hashing, `Edges::merge` of each kept part, and repeated `Walked::unsettled` iteration (resolve.rs, `holds` per body). Ideas: index `Demand` densely instead of hashing; precompute each program's held declaration list once.
- `Flow::edges`: 46M with 253k LL misses.
- `ModuleScope::name` hashes the string up to three times, about 7.7M Ir on the test build. `std`'s map has no stable `raw_entry`, so this needs a merged table or a hash-once map.
- Allocation is about 47M Ir on the test build. DHAT's top sites: `styles::Edges::add`, `Infer::call_fn`, `fresh_int`'s `vec![lit]`.
- `carries_effect` (2.3M) and `Name::new` (1.5M) on `ci`.

Tooling, in `/tmp/cs/` (gone after a reboot): `sync.sh <rev>` copies a tree into the Lima VM `buri-check`; `goals.sh <rev>` runs the counted goals there without writing anything back; `m.sh <rev>` counts the check group of both workspaces; `prof2.sh <label>` takes line-table profiles of the test build; `dhat.py` groups a DHAT profile by its first own frame. Delete `buri-check` when done; never `buri-goals`.
