# Open questions

Only what genuinely needs a decision from the person who owns the language.
Everything else in `design/native/` is decided, and where a decision was close it
is recorded with the alternative and the reason, rather than deferred here.

**The list is empty.** All three items have been ruled on; each is below with the
ruling, the reasoning that survived it, and where the ruling landed in code and
in text. A new item goes above the "Resolved" heading, not inside it.

---

## Open

Nothing.

---

## Resolved

### 1. Does `Checked` keep its backend-dependent bound? — **Yes.**

**Ruled:** `Checked` is bounded by the numbers the **platform** has. Natively a
checked operation answers `.None` when the true result leaves the *type* — `I64`,
`U64`, `I128` and the rest — and the backend computes the true machine result
otherwise. On JavaScript it answers `.None` above 2^53 - 1, what a double
represents exactly. `(1 << 60).checkedAdd(1)` is `.Some` natively and `.None` on
JavaScript.

**Why.** Both answers are the *same* promise kept over different numbers:
`.Some(v)` means `v` is the exact true result as that backend represents numbers,
and `.None` means that backend will not name a value it cannot hold. The
alternative — a native backend that also stops at 2^53, which is what shipped for
a wave — buys a portable `Checked` result at the price of making `Checked`
useless on `I64` natively, which is exactly where a program reaches for it. And
the portability it buys is of a result nobody should branch on: a program whose
behaviour depends on which answer it gets is relying on a `Checked` method to
*fail*.

**Where it landed.**

- `cranelift/emit.rs`: `outside_exact_range` is deleted and `checked` builds its
  `Option` straight from `overflowing`'s flag. `Div` still reports `MIN / -1`,
  because `2^63` is not an `i64` — two's-complement overflow, not a special case.
- `llvm/emit.rs`: `checked` range-tests the 128-bit widening against `int_range`
  rather than `exact_int_range`. The widening design carries over unchanged and
  answers "did it overflow the type" directly; the `select`-guarded divisor stays,
  because an `sdiv` by zero is undefined behaviour in LLVM even on a dead path.
- `cli/runtime/lib.rs`: `buri_rt_i128_checked` — which both backends use at 128
  bits — applied the 2^53 bound and no longer does. It is `i128`/`u128`'s own
  `checked_*`, including `i128::MIN.checked_div(-1)`.
- `saturating*` needed nothing: it clamped at `int_range` on every backend
  already, and `row_02_saturating_is_bounded_by_the_type_on_both_backends` now
  says so rather than leaving it to be assumed. `Wrapping` needed nothing either
  — natively it *is* `iadd`/`isub`/`imul`.
- SPEC: `cli/src/docs/lang/expressions.md` §6.2.2, assembled into `cli/src/docs/SPEC.md`,
  states both bounds concretely and keeps the closing line about relying on
  `Checked` to fail. VALUE-MODEL.md §11.3 is the same text; §12 row 2 is a listed
  divergence again and row 3 records that `Wrapping` was checked against the
  ruling and did not move.
- Tests: the divergent band left the shared conformance corpus, which
  `native/conformance.rs` runs natively and which may therefore only assert what
  both backends answer. `conformance/lib/numbers/test/integers.buri` keeps the
  agreeing halves — sums inside 2^53, and overflow of the type itself — and
  `cli/tests/native/agreement.rs`'s `row_02_checked_above_the_exact_range` is a
  `diverge()` row pinning both answers, with the agreeing cases either side of
  the band asserted in the same program.

### 2. Should `Str` slicing keep its parent buffer alive? — **Yes, and it is not a language question.**

**Ruled:** no language change. `slice`, `trim`, `trimStart`, `trimEnd` and
`splitOnce` stay allocator-free views, and a view keeps its parent allocation
alive for as long as it lives. That is the current behaviour, so nothing in the
compiler, the runtime or the standard library's signatures changed.

**Why.** The retention is an **implementation detail of the runtime**, not a
property of the language. `slice` promises a view; `Alloc` is where allocation is
named; neither promise mentions reference counts, so the strategy underneath can
change under a green suite without a SPEC amendment — exactly as MEMORY.md §5.4's
allocator can. Option 2 (copy above a ratio) was refused because it makes `slice`
name `Alloc`, which changes a standard-library signature and retracts
`str.buri`'s "pure because it is immutable and sliceable"; option 3 (copy on
proven retention) is a real analysis for a case that is hard to detect, and is
still available later precisely because it is not language-visible.

**The sanctioned performance direction** is the one Roc took, and it is now
written into MEMORY.md §5.3 rather than left to be re-derived: Perceus-style
reference counting with **opportunistic in-place mutation at refcount 1**, with as
much ownership decided statically as possible so the runtime `rc == 1` test is
skipped where the answer is already known. The order of future work is more
static ownership, then cross-block reuse, then reuse across a function boundary.
Not on the path: a tracing collector beside the counts, or atomic counts before
the language has threads.

**Where it landed.** MEMORY.md §6's honest-costs note carries the resolution;
§5.3 carries the direction and the two references (Perceus, PLDI 2021; Roc's
implementation and Vermeulen's *Reference Counting with Reuse in Roc*). §6 had
asked for one thing that had not been done — "`core/str` should say so where
`slice` is declared" — and `str.buri`'s `slice` now says it, with the
ten-megabyte example and the way out.

The direction is not aspirational: in-place-at-refcount-1 is implemented for the
two operations that build the only heap blocks worth optimizing, per MEMORY.md
§5.3's "What has landed". `[T]` append is `cli/runtime/list.rs`'s `append_dest`
— shared by both backends behind `list.push` and `list.concat`, growing in place
when the block is uniquely owned — and `Str` concatenation is open-coded twice,
in `cranelift/helpers.rs`'s `concat` and `llvm/emit.rs`'s `concat`, with the
capacity test written to allow for a view that starts inside its block.

### 3. Is the LLVM pin allowed to force a nixpkgs bump later? — **One LLVM at a time, tracking latest.**

**Ruled:**

- **Exactly one** supported LLVM at any moment. Not a range, not a minimum.
- The pin is the **latest LLVM that inkwell and the flake's nixpkgs both carry**.
  Both, because a pin either side cannot supply is a pin nobody can build.
- The LLVM version is an **internal detail**: no BUILD file, `REPO.buri`,
  `Output`, diagnostic or flag names one. `Backend::identity()` is the sole
  exception and it is a cache key, not an interface.
- Bumping is a **routine chore**, not a compatibility event: bump the inkwell
  feature, the `LLVM_SYS_<N>1_PREFIX` name, `flake.nix`'s `llvmPackages_N` and
  `attrs.rs`'s `Location` list, and let the `MemoryEffects` bitmask canary catch
  the one that fails silently. No deprecation window, because there is no second
  version to deprecate.
- **Multi-version support is refused, permanently.** §3.5's `memory(...)` hazard
  is the reason: two LLVMs means the same attribute encoded two ways and a matrix
  that has to build both, for a benefit `nix develop` already delivers.

**Why neither posed policy survived as written.** "The flake leads" is right about
the default and wrong as a rule — it forbids a bump the backend actually needs.
"The backend leads" prices a nixpkgs bump into a routine chore, and that bump
moves `cargo`, `bun` and `elan`, and therefore `buri`'s own hash, which every
repository pins (`build/cache.rs`). Latest-available is the rule: the pin
moves when a nixpkgs bump that was happening anyway brings a newer LLVM along. A
bump *for* an LLVM stays possible, as a nixpkgs decision made on nixpkgs' terms.

**The current pin is correct, re-verified.** Evaluated against `flake.lock`'s
locked revision rather than the channel name, `nixos-25.05` carries
`llvmPackages_9` and `_12` through `_21`; `_21` is **21.1.2** and the default
`llvmPackages` is 19.1.7. **There is no `llvmPackages_22`.** inkwell 0.10 goes to
`llvm22-1`, so 21 is the latest the two sides share, which is what the rule asks
for.

**Where it landed.** CODEGEN-LLVM.md §8.1 (the policy), §8.2 (the four-line bump
checklist and the canary), §8.3 (why 21 today); BUILD-AND-WATCH.md §3.1's
`llvmPackages_21` note.

---

## Decided here rather than asked

Listed so that nobody reads the shortness of the list above as thoroughness
below, and so the reasons are findable.

| Decision | Where | The alternative that lost |
|---|---|---|
| Reference counting, not a GC | MEMORY.md §3 | A tracing GC needs statepoints or conservative scanning; the first contradicts CODEGEN-LLVM.md §0's second instruction, the second makes the heap data-dependent and irreproducible. |
| No arena per `Alloc` scope | MEMORY.md §4 | `Alloc` is a propagating bound and `Region` is a value; there is no scope to hang an arena on. |
| The middle end owns tail-call elimination; neither backend uses its own | CODEGEN-LLVM.md §5, CODEGEN-CRANELIFT.md §3.3 | `musttail`/`tailcc` and `return_call` are both viral, both incompatible with a C-ABI runtime, and both unnecessary once the tail-call graph is a DAG. |
| Neither linker links incrementally; recompile granularity is the codegen unit | CODEGEN-CRANELIFT.md §7 | mold rejected incremental linking partly *because it is not reproducible*, which this toolchain cannot accept. |
| A `stat` sweep over the declared source set, not `notify` | BUILD-AND-WATCH.md §1.2 | A general watcher is hard (Zed abandoned its own; Zig abandoned kqueue) and unnecessary when the input set is enumerated by the cache keys already. |
| One codegen unit per source module, in both profiles; no LTO | ARCHITECTURE.md §5 | Per-function objects lose colocated direct calls; a merged fixed count loses reuse; LTO re-derives an inlining decision the middle end took with an exact call graph. |
| `Str` is 24 bytes with a `base` pointer; `[T]` is 16 | VALUE-MODEL.md §3-4 | A uniform 16-byte shape would make `slice` allocate, contradicting `core/str`'s own contract. |
| Descriptors are folded at compile time and never reach a native artifact | VALUE-MODEL.md §9 | A runtime walker is an interpreter, defeats every memory attribute, and defeats DCE. |
| A context of zero-sized implementations costs nothing | VALUE-MODEL.md §8 | Nothing lost; `ctx_layouts` already records the layout statically. |
| `nsw`/`nuw` are never emitted | CODEGEN-LLVM.md §3.4 | Correct per SPEC 6.2, and it would replace a debuggable wrap with whatever the optimizer inferred. |
| Cranelift pinned to the wasmtime 36 LTS | CODEGEN-CRANELIFT.md §8 | Tracking latest means a monthly `identity()` change, which invalidates every cached object in every repository. |
| `backend-llvm` off by default | BUILD-AND-WATCH.md §2 | `cargo install buri` must not need a system LLVM. |
| Cross-compilation refused, explicitly | ARCHITECTURE.md §9 | `design/native/ARCHITECTURE.md` §9 permits a refusal; the blocker is the host-only runtime archive, and naming that makes the fix a packaging problem rather than a compiler one. |
| `Arena` is accounting-only in v1 | MEMORY.md §7.2 | A real region-freeing arena needs a scoped-context construct the language does not have. |
| One symbol prefix, `buri_rt_`, for every runtime entry including the host ones | VALUE-MODEL.md §10 | A split `buri_rt_` / `buri_host_` makes "is this symbol the runtime's" a table lookup in the one place a compiler asks it per call site. |
| The v1 allocator is `malloc`-backed with no size classes | MEMORY.md §5.4 | Nothing observable differs — the header, `cap`, the reuse test and the *defined* `Alloc` cost model are identical — so the free lists can land later under a green suite instead of being co-developed with two backends. |
| The runtime archive is precompiled by `cli/build.rs`, not compiled on demand | BUILD-AND-WATCH.md §2.2 | On demand puts a `rustc` in the build loop and makes the archive hash depend on whatever `rustc` was on `PATH` at *use* time. |
| `Net::fetch` is cleartext `http://` only, and says so | `cli/runtime/http.rs` | Bundling a TLS stack fails the dependency bar today and silently downgrading to cleartext would be worse than refusing; the growth path is a `net-tls` feature over `rustls`, which clears all three clauses. |
