# design/

**Working notes, roadmaps and design documents. The audience is somebody
changing the toolchain, not somebody using it.**

User documentation lives under [`cli/src/docs/`](../cli/src/docs/) and is
served by `buri docs`. Nothing here is compiled into the binary, nothing here
is served, and nothing here is held to the "every example runs" standard the
documentation suite applies over there. What is written here is allowed to be
provisional, to argue with itself, and to go out of date the moment the code
lands — that is what makes it useful to write.

The one rule: **when a decision made here becomes true of the toolchain, it
moves.** A design document that has shipped is a second, drifting copy of the
reference; say it once, under `cli/src/docs/`, and leave the argument behind
here.

| File | What it is |
|---|---|
| [`TODO.md`](./TODO.md) | What is not done: open gaps, deferred work with its reasons, and the decisions to keep saying no to. Completed work is not recorded there. Cite it by heading anchor, never by line number. |
| [`STANDARD-LIBRARY.md`](./STANDARD-LIBRARY.md) | Why `core/*` contains what it contains, and what the deliberate absences would cost to close. |
| [`ui-reactivity.md`](./ui-reactivity.md) | Why `ui/*` is shaped the way it is: signals as inert handles, meaning as a role and arrangement as a style, two style tiers, and exhaustive themes. It has shipped, so it keeps the argument and points at the reference; its "As built" section records where compiling it overruled it. |
| [`PERFORMANCE.md`](./PERFORMANCE.md) | What "fast" means for this toolchain, how it is measured, and what the measurements say. The benchmark harness's own READMEs cite it as normative. |
| [`native/`](./native/) | The native backend's design: architecture, value model, memory, the two native code generators, build and watch, and the decisions taken. |

Three neighbours that are also not user documentation:
[`formal/`](../formal/) is the Lean 4 formalisation of the type system,
[`cli/tests/README.md`](../cli/tests/README.md) explains how the toolchain is
tested, and [`reference/README.md`](../reference/README.md) is the reading list —
every paper the design documents argue from, with a link to each.

## Wave numbering

The native backend was built in labelled waves, and the labels outlived the
rollout: they are still module headers in the source (`//! ... **Wave 2b.**`)
and they appear in the design documents next to the decisions they carried. A
reader who meets one needs somewhere to look it up, and this is it. The
collision map that made the waves safe to run in parallel is not kept — it
described who was allowed to write which file during a rollout that is over.

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

**A second set of labels appears beside these**, and it is not the same scheme:
the concurrency-and-servers program that followed was cut into slices named by a
letter and a number — `B6`, `C4`, `C7`, `D4`, `E13`, `F2`–`F8`, `G5`, `H3` and
the rest — and they are in `DECISIONS.md` rows, in `cli/runtime/` comments and in
`core/*` sources. They are slice names rather than a rollout order, there is no
table of them, and none is needed: every one is named inside a sentence that says
what that slice did, and `native/DECISIONS.md` is where the sentences are.

One thing from wave 3c did **not** land: the golden re-record for a *Linux*
host. Two fixtures name `linux` as the platform this toolchain cannot produce,
and on a Linux machine it now can.
[`native/ARCHITECTURE.md`](./native/ARCHITECTURE.md) §4's last paragraph has it.
