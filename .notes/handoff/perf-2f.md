# Perf phase 2F handoff

Profile the code repo's "three nodes keep every invariant over 12 seeds" suite as in `perf-phase1.md`. The runner is now `.buri/out/native/macos-arm64/test-runner`, a direct child of `buri`.

Landed:

- `stdlib: an ordered map reads its nodes by index`: `xs[i]` instead of `list.get`, so a lookup no longer retains and releases every key and child it passes.
- `middle: a descent into a piece of a borrowed node borrows`: a loop variable bound from a piece of a borrowed parameter stays borrowed in `rc.rs`'s `loop_variables_taken`.
- `middle: a derived order compares a Str field once`: `derives.rs` emits one `str.compare` instead of `Lt` then `Gt`.

Shares of the profile, before and after the first two (the third came after, and targets about 1,400 samples of memcmp and `str_compare` from `$derive$cmp$Attribute`):

- orderedmap: 17.7% → 14.4%
- retain/release: 18.7% → 18.1%
- memmove: 3.8% → 4.3%
- memcmp: 3.0% → 3.7%
- `Sim_memberOf`: 5.5% → 5.3%

Remaining hotspots:

- Retain/release is still the biggest group at about 18%. Look for pairs around closure calls in `Sim_memberOf`.
- memmove comes from `memberOf`'s closure, `map_replace_at` and `insert_at` in orderedmap's `insertInto`, and `list_join` in `core_path.of`. The testing filesystem's share belongs to another agent.
- Stencil index lowering checks the bound with two compares; one unsigned compare would do.
- A word-at-a-time `byte_order` in `cli/runtime/text.rs` didn't cut instructions, so don't retry it.
