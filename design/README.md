# design/

**Working notes, roadmaps and design documents. Written for somebody changing
the toolchain, not somebody using it.**

User documentation lives under [`cli/src/docs/`](../cli/src/docs/), and `buri
docs` serves it. Nothing here goes into the binary, nothing here is served, and
nothing here has to meet the "every example runs" standard the documentation
suite holds those pages to. Notes here may be provisional, may argue with
themselves, and may go stale the moment the code lands. That freedom is what
makes them worth writing.

The one rule: **when a decision made here becomes true of the toolchain, it
moves.** A shipped design document is a second copy of the reference, and it
drifts. Say it once, under `cli/src/docs/`, and leave the argument here.

| File | What it is |
|---|---|
| [`ui-reactivity.md`](./ui-reactivity.md) | Why `ui/*` is shaped the way it is: signals as inert handles, meaning as a role and arrangement as a style, two style tiers, and exhaustive themes. It shipped, so it keeps the argument and points at the reference. Its "As built" section records where compiling it overruled it. |
| [`grammar-rationale.md`](./grammar-rationale.md) | Every decision that keeps the grammar context-free and unambiguous, and what each one cost. Once section 12 of the specification. Nobody writing Buri needs it, but the compiler's own comments cite it by item number. |
| [`static-rules.md`](./static-rules.md) | Every well-formedness rule the grammar cannot express, numbered. Once section 13 of the specification. The checker enforces each rule, the error catalogue is how a reader meets it, and the compiler's comments cite it by rule number. |
| [`non-goals.md`](./non-goals.md) | What the language deliberately leaves out, what is deferred and why those items are deferred together, and which trade-offs are still open. Once section 14 of the specification. It also holds the struct-of-arrays argument the standard library kept sending people to. |
| [`resolved-questions.md`](./resolved-questions.md) | The arguments behind decisions [`non-goals.md`](./non-goals.md) now states in one line: `for`/`while` and `|>`, specified and then cut, plus the `I64`-on-JavaScript question that got an answer. Each one constrains the next proposal that asks for the same thing. |
| [`PERFORMANCE.md`](./PERFORMANCE.md) | What "fast" means for this toolchain, what the benchmarks measure, and what the numbers say. The benchmark harness's own READMEs treat it as normative. |
| [`native/`](./native/) | The native backend's design: architecture, value model, memory, the two native code generators, build and watch, and the decisions taken. |

Three neighbours are also not user documentation.
[`formal/`](../formal/) formalises the type system in Lean 4.
[`cli/tests/README.md`](../cli/tests/README.md) walks through the test suites
and how they are arranged. [`reference/README.md`](../reference/README.md) is
the reading list: every paper the design documents argue from, with a link to
each.

## Wave numbering

The native backend went out in labelled waves, and the labels outlived the
rollout. They still head modules in the source (`//! ... **Wave 2b.**`), and
they turn up in the design documents next to the decisions they carried. If you
meet one, look it up here. The collision map that let the waves run in parallel
is gone: it said who could write which file during a rollout that is over.

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

**A second set of labels sits beside these**, on a different scheme. The
concurrency-and-servers program that followed was cut into slices named by a
letter and a number — `B6`, `C4`, `C7`, `D4`, `E13`, `F2`–`F8`, `G5`, `H3` and
the rest — and they appear in `DECISIONS.md` rows, in `cli/runtime/` comments
and in `core/*` sources. They name slices, not a rollout order, so there is no
table of them and none is needed: every one sits inside a sentence saying what
that slice did, and `native/DECISIONS.md` holds those sentences.

One piece of wave 3c did **not** land: the golden re-record for a *Linux* host.
Two fixtures still name `linux` as the platform this toolchain cannot produce,
and on a Linux machine it now can.
[`native/ARCHITECTURE.md`](./native/ARCHITECTURE.md) §4's last paragraph has it.
