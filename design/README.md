# design/

**Working notes, roadmaps and design documents. Written for somebody changing
the toolchain, not somebody using it.**

User documentation lives under [`cli/src/docs/`](../cli/src/docs/), and `buri
docs` serves it. Nothing here goes into the binary and nothing here is served,
so notes here may be provisional and may go stale the moment the code lands.

The one rule: **when a decision made here becomes true of the toolchain, it
moves.** A shipped design document is a second copy of the reference, and it
drifts. Say it once, under `cli/src/docs/`, and leave the argument here.

| File | What it is |
|---|---|
| [`ui-reactivity.md`](./ui-reactivity.md) | Why `ui/*` is shaped the way it is: signals as inert handles, meaning as a role and arrangement as a style, two style tiers, exhaustive themes. It shipped, so it keeps the argument and points at the reference. "As built" records where compiling it overruled it. |
| [`grammar-rationale.md`](./grammar-rationale.md) | Every decision that keeps the grammar context-free and unambiguous, and what each one cost. Once section 12 of the specification; the compiler's comments cite it by item number. |
| [`static-rules.md`](./static-rules.md) | Every well-formedness rule the grammar cannot express, numbered. Once section 13; the compiler's comments cite it by rule number. |
| [`non-goals.md`](./non-goals.md) | What the language leaves out, what is deferred and why those items are deferred together, and which trade-offs are still open. Once section 14. It also holds the struct-of-arrays argument. |
| [`resolved-questions.md`](./resolved-questions.md) | The arguments behind decisions [`non-goals.md`](./non-goals.md) now states in one line. Each one constrains the next proposal that asks for the same thing. |
| [`PERFORMANCE.md`](./PERFORMANCE.md) | What "fast" means for this toolchain, what the benchmarks measure, and what the numbers say. The benchmark harness's own READMEs treat it as normative. |
| [`native/`](./native/) | The native backend's design: architecture, value model, memory, the two native code generators, build and watch, and the decisions taken. |

Three neighbours are also not user documentation.
[`formal/`](../formal/) formalises the type system in Lean 4.
[`cli/tests/README.md`](../cli/tests/README.md) walks through the test suites.
[`reference/README.md`](../reference/README.md) is the reading list: every paper
the design documents argue from.

## Wave numbering

The native backend went out in labelled waves. The labels still head modules in
the source (`//! ... **Wave 2b.**`) and turn up beside the decisions they
carried, so look one up here.

| Wave | What it was |
|---|---|
| 0 | `transform` → `middle`; the `Backend`/`Linker` traits and `Emitted`; `Action::Codegen`; the cargo features; `middle/mod.rs` declaring every module the later waves fill in |
| 1a | `middle::ir` and `middle::lower` — the block-argument SSA CFG |
| 1b | `middle::layout` — the value model as a memoised table, plus the `Alloc` cost model |
| 1c | `cli/runtime` — the C-ABI runtime, and the `build.rs` that builds it |
| 1d | `middle::{decision, closures, dce, tail_calls}` — the tree passes, and the tail-call *rewrite* that replaced the emitter consulting a `Plan` |
| 1e | `middle::{derives, rc}` — generated derives, and own/borrow inference with reuse |
| 2a | The Cranelift backend — removed 2026-08-29, with its design document; [`native/CODEGEN-STENCIL.md`](./native/CODEGEN-STENCIL.md) §13 is the record |
| 2b | The LLVM backend |
| 2c | The link step, the object cache, and the manifest |
| 3a | Native `--check-reproducible` |
| 3b | `buri test --watch` |
| 3c | The `host_platform()` switch, the SPEC amendment, and the golden re-record |
| 3d | The `buri_rt_*` runtime surface as both backends call it |
| 4 | The allocator types and `Alloc` accounting |

**A second set of labels sits beside these.** The concurrency-and-servers
program that followed was cut into slices named by a letter and a number — `B6`,
`C4`, `C7`, `D4`, `E13`, `F2`–`F8`, `G5`, `H3` and the rest — appearing in
`DECISIONS.md` rows, `cli/runtime/` comments and `core/*` sources. They name
slices, not a rollout order, so there is no table: every one sits inside a
sentence in `native/DECISIONS.md` saying what that slice did.

One piece of wave 3c did **not** land: the golden re-record for a *Linux* host.
Two fixtures still name `linux` as the platform this toolchain cannot produce,
and on a Linux machine it now can.
[`native/ARCHITECTURE.md`](./native/ARCHITECTURE.md) §4's last paragraph has it.
