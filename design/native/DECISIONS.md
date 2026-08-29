# The decision index

Everything in `design/native/` is decided. This file is the index: one row per
decision that was close enough to be worth arguing, the document where it is
argued in full, and the alternative that lost.

It exists because the arguments are spread across six documents and a reader
who wants to know *whether* something was considered should not have to read all
seven to find out. Where a row and its document disagree, the document is right —
this is a table of contents, not a second source.

Nothing here is open. Work that is not done is in
[`design/TODO.md`](../TODO.md), under "The native backend"; a decision that is
reversed is reversed in the document that made it, with the reversal recorded
there rather than deleted.

| Decision | Where | The alternative that lost |
|---|---|---|
| Reference counting, not a GC | MEMORY.md §3 | A tracing GC needs statepoints or conservative scanning; the first contradicts CODEGEN-LLVM.md §0's second instruction, the second makes the heap data-dependent and irreproducible. |
| `Checked` is bounded by the numbers the *backend* has | VALUE-MODEL.md §11, SPEC §6.2.2 | A native backend that also stopped at 2^53 shipped for a wave and was reversed: it makes `Checked` useless on `I64` natively, and buys portability of a result nobody should branch on. |
| `Str` slicing keeps its parent buffer alive, and that is not a language question | MEMORY.md §5.3, §6 | Copying above a ratio makes `slice` name `Alloc` and retracts `core/str`'s purity claim; copying on proven retention is a real analysis for a case that is hard to detect, and stays available precisely because it is not language-visible. |
| Exactly one supported LLVM at a time, tracking the latest both inkwell and the flake carry | CODEGEN-LLVM.md §8.1–8.3 | "The flake leads" forbids a bump the backend needs; "the backend leads" prices a nixpkgs bump — which moves `cargo`, `bun`, `elan` and therefore `buri`'s own hash — into a routine chore. Multi-version support is refused permanently: two LLVMs means one attribute encoded two ways. |
| No arena per `Alloc` scope | MEMORY.md §4 | `Alloc` is a propagating bound and `Region` is a value; there is no scope to hang an arena on. |
| The middle end owns tail-call elimination; no backend uses its own | CODEGEN-LLVM.md §5 | `musttail`/`tailcc` and `return_call` are both viral, both incompatible with a C-ABI runtime, and both unnecessary once the tail-call graph is a DAG. The `return_call` half was argued in CODEGEN-CRANELIFT.md §3.3 and went with that document on 2026-08-29 (CODEGEN-STENCIL.md §13); the row is unchanged, because the reason never rested on it. |
| Neither linker links incrementally; recompile granularity is the codegen unit | CODEGEN-STENCIL.md §12 | mold rejected incremental linking partly *because it is not reproducible*, which this toolchain cannot accept. |
| A `stat` sweep over the declared source set, not `notify` | BUILD-AND-WATCH.md §1.2 | A general watcher is hard (Zed abandoned its own; Zig abandoned kqueue) and unnecessary when the input set is enumerated by the cache keys already. |
| One codegen unit per source module, in both profiles; no LTO | ARCHITECTURE.md §5 | Per-function objects lose colocated direct calls; a merged fixed count loses reuse; LTO re-derives an inlining decision the middle end took with an exact call graph. |
| `Str` is 24 bytes with a `base` pointer; `[T]` is 16 | VALUE-MODEL.md §3-4 | A uniform 16-byte shape would make `slice` allocate, contradicting `core/str`'s own contract. |
| Descriptors are folded at compile time and never reach a native artifact | VALUE-MODEL.md §9 | A runtime walker is an interpreter, defeats every memory attribute, and defeats DCE. |
| A context of zero-sized implementations costs nothing | VALUE-MODEL.md §8 | Nothing lost; `ctx_layouts` already records the layout statically. |
| `nsw`/`nuw` are never emitted | CODEGEN-LLVM.md §3.4 | Correct per SPEC 6.2, and it would replace a debuggable wrap with whatever the optimizer inferred. |
| Cranelift pinned to the wasmtime 36 LTS — **reversed 2026-08-29**, with its subject | CODEGEN-STENCIL.md §13 | Tracking latest meant a monthly `identity()` change, which invalidated every cached object in every repository. The pin was answering a question the removal deleted: there is no version left to bump. |
| `backend-llvm` off by default; `backend-stencil` on | BUILD-AND-WATCH.md §2 | `cargo install buri` must not need a system LLVM. `backend-cranelift` was on by default beside `backend-stencil` until **2026-08-29** (CODEGEN-STENCIL.md §13); what is left needs nothing a machine able to link a native artifact does not already have, and adds nothing to the lockfile. |
| The stencil backend is the debug backend — **reversed 2026-08-29** | ARCHITECTURE.md §4, CODEGEN-STENCIL.md §13 | It was compiled in and never selected for as long as the seat was a decision about parity rather than about plumbing. Parity was met — 997 of 997, the same six refusals, the same blocks live at exit — so the decision was taken and Cranelift was removed: 38 transitive crates to 0, a ~42% smaller `buri`, and x86-64 covered by writing the emitter rather than by keeping a second code generator. What lost, and is written down rather than repaired: a 1.38× debug runtime and the `str.concat` allocation-count divergence. |
| Cross-*linking* refused, explicitly; cross codegen supported | ARCHITECTURE.md §9 | The blocker is the host-only runtime archive rather than any backend, and naming that makes the fix a packaging problem rather than a compiler one. |
| `Arena` is accounting-only in v1 | MEMORY.md §7.2 | A real region-freeing arena needs a scoped-context construct the language does not have. |
| One symbol prefix, `buri_rt_`, for every runtime entry including the host ones | VALUE-MODEL.md §10 | A split `buri_rt_` / `buri_host_` makes "is this symbol the runtime's" a table lookup in the one place a compiler asks it per call site. |
| The v1 allocator is `malloc`-backed with no size classes | MEMORY.md §5.4 | Nothing observable differs — the header, the capacity, the reuse test and the *defined* `Alloc` cost model are identical — so the free lists can land later under a green suite instead of being co-developed with two backends. |
| The runtime archive is precompiled by `cli/build.rs`, not compiled on demand | BUILD-AND-WATCH.md §2.2 | On demand puts a `rustc` in the build loop and makes the archive hash depend on whatever `rustc` was on `PATH` at *use* time. |
| `Net::fetch` is cleartext `http://` only, and says so | `cli/runtime/http.rs` | Bundling a TLS stack fails the dependency bar today and silently downgrading to cleartext would be worse than refusing; the growth path is a `net-tls` feature over `rustls`, which clears all three clauses. |
