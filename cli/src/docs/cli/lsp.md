## What it does

```
buri lsp
```

A language server, speaking LSP over stdin and stdout. Editors start it; you do
not run it by hand. It serves the same analysis `buri build` runs — the front
end is a library, and `driver::analyze` is what the server calls.

## What it answers

| Request | |
|---|---|
| `publishDiagnostics` | Every finding the front end has, for every file it looked at — including files you did not open, because a change in one module is usually reported in another. |
| `hover` | On a declaration's name, its signature and doc comment. Anywhere else, the type of the innermost expression under the cursor. |
| `definition` | Where the name under the cursor was declared. |
| `documentSymbol` | The outline, built from the syntax alone — so it still works in a file that does not typecheck, which is when an outline is worth most. |
| `formatting` | `buri format`, which refuses to emit anything that does not parse, so a file mid-edit is left alone rather than mangled. |
| `completion` | Inside a module path, the standard library plus the labels your target already declares. Inside an import's `{ … }`, what that module exports. |
| `codeAction` | The fix for a finding that has exactly one. |

**Diagnostics include the build-graph findings**, not only the type errors —
`missing-dep`, `unused-import`, and the rest of what `buri lint` reports. An
editor that showed half of what the toolchain knows would be showing the half
that is easier to notice at a terminal anyway.

**The fixes are the same ones `buri lint --fix` applies**, and they are applied
the same way: a finding about a build file is handed to `buri gen`, which
returns the whole file, so a `BUILD.buri` is never byte-edited and the three
paths cannot end up disagreeing. A finding with no mechanical answer — which
edge of a `dep-cycle` to cut — offers nothing rather than guessing.

Not answered, deliberately: rename, references, semantic tokens, inlay hints,
signature help, call hierarchy. Each is a real feature and none is worth
shipping half of.

## When it analyses

Stated here rather than tuned silently, because it is the difference between a
server that keeps up with typing and one that does not:

- **On a keystroke** — `didChange` — the server re-parses that one buffer and
  publishes the parse errors. No workspace, no standard library, no imports.
- **On open and on save** the server runs the whole front end over the target's
  closure and publishes everything, then runs the lint pass and publishes that
  too. The lint checks build their own analysis, so a save costs two — which is
  the other reason this does not happen on a keystroke.

The reason for the split is that the front end has no incremental mode:
`driver::analyze` is whole-closure. Analysing on every keystroke would mean
re-checking the standard library on every keystroke.

An unsaved buffer is not a problem — the server hands the editor's copy to the
loader in place of the file on disk, so what you see reported is what you are
looking at.

## Two things worth knowing

**Only the protocol goes to stdout.** Every log goes to stderr. A stray line on
stdout corrupts the stream in a way that presents as the editor being broken.

**Requests are handled one at a time**, in the order they arrive. That costs
some latency on a slow analysis and buys determinism: a session is reproducible,
which is what lets `cli/tests/repositories/lsp/` record one as a golden file.

## Editors

The Zed extension is in `editors/zed`. It is a separate crate, outside the cargo
workspace, because a Zed extension must depend on `zed_extension_api`, which
does not clear the toolchain's dependency bar (see the root `Cargo.toml`).

Any editor that speaks LSP can start `buri lsp` for files matching `*.buri`.
