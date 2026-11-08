# Stencil code quality against LLVM -O0 and -O1

Measure in a native aarch64 Lima VM under `nix develop .#perf`:

```sh
cargo bench -p buri --features backend-llvm --bench instructions -- --levels
```

It exits 1 when a program is over -O0, but the numbers still hold.

For a per-instruction profile, run the stencil binary under `valgrind --tool=callgrind --dump-instr=yes`. Strip the mapping symbols before you disassemble, or objdump shows stencil code as data:

```sh
llvm-objcopy --wildcard --strip-symbol='$d*' --strip-symbol='$x*' soak soak.nod
llvm-objdump -d --no-show-raw-insn soak.nod
```

Instructions on aarch64 at b1fed055a, as stencil / -O0 and stencil / -O1:

- lists 2997M: 0.998, 1.68
- strings 2265M: 0.99, 1.28
- floats 712M: 0.98, 1.17
- sort 1441M: 0.94, 5.16
- tree 1744M: 0.93, 1.13
- maps 2656M: 0.88, 1.34
- kvstore 1800M: 0.82, 1.54
- match 1289M: 0.69, 2.09

Every program now runs fewer instructions than at -O0.

Dev codegen instructions per line under `--goals=count`, from 58de80e25 to b1fed055a: ci -0.4%, monorepo-ci -3.3%. Both monorepo-ci dev-codegen goals were already about 70% over their baseline before this work.

## Done

- An integer, `Char` or `Bool` compare is one `cmp3/{type}/{width}` stencil at the call: sort -31%.
- A box is one `box/{n}` stencil, an unzeroed allocation plus a copy: tree -13.5%.
- `strip_dead_clears` also drops a clear that clang scheduled before the call, when nothing after it reads the register: kvstore -3.9%.
- A load or store at a constant offset from a pointer is one `pload`/`pput` stencil: kvstore -1.2%.
- `Str.length` is a `strlen` stencil at the call.
- A match arm's tag test is one branch, because the switch, guards, checked and saturating pick `arm_key(.., Jump::F)`.
- A match on a niche `Option` tests the pointer in place.
- A short number or list body is emitted at its call site (`emit.rs::open_body`).
- Each arm64 stencil carries a narrow twin (`extract::narrow`) whose immediate pairs are one `movz` each. The emitter takes it when every pair's value fits 16 bits: kvstore -2.8%. The twin lives inside its stencil's record, so the key index didn't grow. Each arm64 library did, from 5.0 MB to 7.9 MB.

## Gaps left, biggest first

- Re-bless `aarch64 run-dev/kvstore`. Staging blessed 1805717564 for the orderedmap in-place inserts, measured without this work, and this work alone took kvstore from 1897M to 1800M.

- Glue retain, release and element walks are about 13% of kvstore. `glue$elems` multiplies the index by the element size with `mul`, recomputes the slot address every time, and stores then reloads each temporary.
- A runtime call that answers an `Order` writes to scratch, reloads, and stores one byte (`rtcall.rs`, the `narrow` store). Then the match reloads the tag once per arm. It's hot in `core_orderedmap$seek`.
- `str xR, [x0, #o]` straight before `ldr xR, [x0, #o]` is 0.7% of kvstore. Loading the same slot into another register is another 0.6%.
- `mov/24` and `mov/32` take two address adds when one of the offsets is above 4095. A fold twin is all-or-nothing, so the small offset doesn't fold either.
- `fold_addressing` declines when a later hole re-materialises the same register. It needs liveness that treats `movz` as a kill.
- `fold_imm` declines on `cmp x1, x8; cset w8, ne`, because `kills` doesn't know `csinc`.
- The closures that lists passes to `mapCtx`, `filter`, `fold` and `sortBy` stay separate calls. -O1 inlines them.
