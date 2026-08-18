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
| [`LLVM-tips.md`](./LLVM-tips.md) | Four lines of instruction that the native codegen documents treat as normative. |
| [`native/`](./native/) | The native backend's design: architecture, value model, memory, the two code generators, build and watch, and the open questions. |

Two neighbours that are also not user documentation:
[`formal/`](../formal/) is the Lean 4 formalisation of the type system, and
[`cli/tests/README.md`](../cli/tests/README.md) explains how the toolchain is
tested.
