# Open questions

Only what genuinely needs a decision from the person who owns the language.
Everything else in `design/native/` is decided, and where a decision was close it
is recorded with the alternative and the reason, rather than deferred here.

Three items. Two are language-visible; one is a policy.

---

## 1. Does `Checked` keep its backend-dependent bound?

**Where:** VALUE-MODEL.md §11.3, row 2 of §12.

SPEC 6.2.2 says a `Checked` method answers `.None` "outside the type's range, or
above what the backend represents exactly", and names 2^53 - 1 as the JavaScript
bound. Natively there is no such second bound, so `(1 << 60).checkedAdd(1)` is
`.Some` on a native build and `.None` on a JavaScript one.

The amendment drafted in §11.3 keeps that, on the argument that `.Some(v)` is
always a promise that `v` is the answer and the JavaScript backend simply
declines to promise what it cannot keep. That is defensible and it is what the
SPEC already says.

The alternative is to make `Checked` **backend-independent** by having the native
backend also answer `.None` above 2^53. That would make a `Checked` result
portable, at the cost of `Checked` being useless on `I64` natively — which is
where a program would most want it.

This needs you because it is the only place in the design where a *defined*
behaviour differs between backends. Everything else that differs is undefined
(overflow) or is native being strictly better (`Option<Option<T>>`, `I128`).

**Recommendation:** keep the divergence, ship §11.3 as drafted, and list it in
`backend_agreement.rs`. But it is a language decision, not a codegen one.

---

## 2. Should `Str` slicing keep its parent buffer alive?

**Where:** MEMORY.md §6, VALUE-MODEL.md §3.

`core/str` promises that `slice`, `trim` and `splitOnce` are pure because they
return views rather than copies (`str.buri:3-4`, `str.buri:42-43`). Under
reference counting, a view holds a reference to the whole parent allocation. So

```buri ignore why="illustrative"
let (key, _) = tenMegabyteLine.splitOnce("=")?;
key
```

retains ten megabytes to keep three bytes. On JavaScript, engines either copy the
slice or do the same thing and are not asked about it.

Three answers, and the design takes the first:

1. **Keep it, document it.** `core/str`'s declaration of `slice` says so. Zero
   cost, one footgun, and the footgun is the one every slice-based string type
   has.
2. **Copy above a ratio.** `slice` copies when the view is under, say, a quarter
   of its parent. That makes `slice` allocate, which means it must name `Alloc`,
   which is a **language change**: `fn slice(self: Str, ...)` becomes
   `fn slice<C: Alloc>(self: Str, ctx: C, ...)`, and `splitOnce` with it. Every
   caller changes, and `str.buri`'s "pure because it is immutable and sliceable"
   claim goes away.
3. **Copy on retention.** Compact a view when the middle end can prove it
   outlives its parent's other uses. Sound, and the analysis is real work for a
   case that is hard to detect.

**Recommendation:** (1), and a sentence in `core/str`. It is listed here because
(2) is a change to a signature in the standard library and only you can decide
whether that trade is worth making before anybody depends on the current one.

---

## 3. Is the LLVM pin allowed to force a nixpkgs bump later?

**Where:** CODEGEN-LLVM.md §8, BUILD-AND-WATCH.md §3.1.

The design pins LLVM 21.1 because `nixos-25.05` — what `flake.lock` currently
holds — provides `llvmPackages_18/19/20/21` and no 22. So the pin is chosen by
what the flake already has, which is the right default and is not obviously the
right rule.

The question is what happens at the next LLVM bump. Two policies:

- **The flake leads.** LLVM is whatever the pinned nixpkgs offers, and the
  backend follows. Predictable, and the toolchain never needs a channel bump for
  a codegen reason.
- **The backend leads.** The backend pins the LLVM it wants and the flake's
  nixpkgs is bumped to supply it. Faster access to codegen improvements, at the
  cost of a nixpkgs bump — which moves `cargo`, `bun` and `elan` too, and
  therefore moves every artifact this toolchain produces.

The second is the one with a hidden cost, and it is the reason this is a question
rather than a decision: bumping nixpkgs to get an LLVM changes the Rust
compiler that builds `buri`, which changes `buri`'s own hash, which every
repository pins (`build/toolchain.rs`).

**Recommendation:** the flake leads, and an LLVM bump rides along with a nixpkgs
bump that was going to happen anyway. Written here because "which of these two
things is allowed to move the other" is a project policy and not something a
design document should decide on its own.

---

## Decided here rather than asked

Listed so that nobody reads the shortness of the list above as thoroughness
below, and so the reasons are findable.

| Decision | Where | The alternative that lost |
|---|---|---|
| Reference counting, not a GC | MEMORY.md §3 | A tracing GC needs statepoints or conservative scanning; the first contradicts `LLVM-tips.md:2`, the second makes the heap data-dependent and irreproducible. |
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
| Cross-compilation refused, explicitly | ARCHITECTURE.md §9 | `TODO.md:1760` permits a refusal; the blocker is the host-only runtime archive, and naming that makes the fix a packaging problem rather than a compiler one. |
| `Arena` is accounting-only in v1 | MEMORY.md §7.2 | A real region-freeing arena needs a scoped-context construct the language does not have. |
| One symbol prefix, `buri_rt_`, for every runtime entry including the host ones | VALUE-MODEL.md §10 | A split `buri_rt_` / `buri_host_` makes "is this symbol the runtime's" a table lookup in the one place a compiler asks it per call site. |
| The v1 allocator is `malloc`-backed with no size classes | MEMORY.md §5.4 | Nothing observable differs — the header, `cap`, the reuse test and the *defined* `Alloc` cost model are identical — so the free lists can land later under a green suite instead of being co-developed with two backends. |
| The runtime archive is precompiled by `cli/build.rs`, not compiled on demand | BUILD-AND-WATCH.md §2.2 | On demand puts a `rustc` in the build loop and makes the archive hash depend on whatever `rustc` was on `PATH` at *use* time. |
| `Net::fetch` is cleartext `http://` only, and says so | `cli/runtime/http.rs` | Bundling a TLS stack fails the dependency bar today and silently downgrading to cleartext would be worse than refusing; the growth path is a `net-tls` feature over `rustls`, which clears all three clauses. |
