## What it does

```
buri lsp
```

A language server, speaking the Language Server Protocol over stdin and stdout. Editors start it; you do
not run it by hand. It serves the same analysis `buri build` runs — the front
end is a library, and `driver::analyze` is what the server calls.

## What it answers

| Request | |
|---|---|
| `publishDiagnostics` | Every finding the front end has, for every file it looked at — including files you did not open, because a change in one module is usually reported in another. |
| `hover` | On any name — a call, a type, a field, a variant, a local — the declaration's signature and its doc comment, wherever the declaration is. On something that is not a name, the type of the innermost expression under the cursor. |
| `definition` | Where the name under the cursor was declared, for every kind of name a file can write: functions and methods, types in expressions and in annotations, traits in bounds and in `impl` heads, module-level `let`s, locals and parameters, fields, variants, and the names in an import clause. Also the path of an import, and — in a `BUILD.buri` — a dependency label, a source entry and a tag. |
| `references` | Every place the repository names the symbol under the cursor, asked from the declaration or from any use. The declaration itself is included when the client asks for it. |
| `documentSymbol` | The outline, built from the syntax alone — so it still works in a file that does not typecheck, which is when an outline is worth most. |
| `formatting` | `buri format`, which refuses to emit anything that does not parse, so a file mid-edit is left alone rather than mangled. |
| `completion` | Inside a module path, the standard library plus the labels your target already declares. Inside an import's `{ … }`, what that module exports. |
| `codeAction` | The fix for a finding that has exactly one. |

**Diagnostics include the build-graph findings**, not only the type errors —
`missing-dep`, `unused-import`, and the rest of what `buri lint` reports. An
editor that showed half of what the toolchain knows would be showing the half
that is easier to notice at a terminal anyway.

**Hover and go-to-definition ask one question**, in one place: what does the
name under the cursor refer to? Hover renders the answer and definition returns
its span, so the two can never disagree about what you are pointing at. The
standard library is compiled into the binary and has no file, so a definition
inside it answers with nothing rather than sending the editor somewhere wrong —
hover still shows its signature and its docs.

**Definition also follows the paths that are not names.** The path string of an
import or a re-export opens the file that path resolves to — the workspace
resolves it, so it works whether or not that module loaded, which is exactly the
state a file is in when the dependency it needs has not been declared yet. A
`core/...` path resolves to a module compiled into the binary, so it answers with
nothing.

Inside a `BUILD.buri` or a `REPO.buri` there is no analysis to consult — these
are textproto, and the front end never reads one — so the build graph answers
instead. A dependency label opens that package's build file, `//lib/money/testing`
along with `//lib/money`, because a testing surface is declared in the same file
its library is. A `sources`, `proto_sources` or `data` entry opens the file
itself, beside the build file. A `tags` entry opens that tag's block in
`REPO.buri`. A string that names none of those — `//visibility:public`, a label
in no package of this repository — answers with nothing.

Every one of those jumps lands at the top of the file it names, tag blocks
apart. A label names a package rather than a line, and picking a rule inside its
build file would be answering a question the label does not ask.

**References is that same question, inverted.** The cursor names a symbol, and
the answer is every other place the repository names it: calls and function
values, method calls, types in annotations, in bounds, in `impl` heads and in
`derive` lists, struct and enum literals, field accesses and the field names in
a literal or a pattern, variants in both spellings, and the names in an import
clause — importing something is a use of it. A local is matched by where it was
bound rather than by how it is spelled, so it never escapes the body that
declares it.

The whole repository is analysed for a references request, not the one target
that owns the file, because a name is used wherever it is imported and nothing
about the file it was declared in says which targets those are. That is paid per
request and it is the most expensive thing the server does. There is no cache,
for the same reason there is none anywhere else here: a cache needs a key saying
which files and which revisions produced it.

A place the source never wrote the name is not a reference to it: an operator
standing for a trait method is a use of that method and is not in the list.

**The fixes are the same ones `buri lint --fix` applies**, and they are applied
the same way: a finding about a build file is handed to `buri gen`, which
returns the whole file, so a `BUILD.buri` is never byte-edited and the three
paths cannot end up disagreeing. A finding with no mechanical answer — which
edge of a `dep-cycle` to cut — offers nothing rather than guessing.

Not answered, deliberately: rename, semantic tokens, inlay hints, signature
help, call hierarchy. Each is a real feature and none is worth shipping half of.

## When it analyses

Stated here rather than tuned silently, because it is the difference between a
server that keeps up with typing and one that does not:

- **On a keystroke** — `didChange` — the server re-parses that one buffer and
  publishes the parse errors. No workspace, no standard library, no imports.
- **On open and on save** the server runs the whole front end over the target's
  closure and publishes everything, then runs the lint pass and publishes that
  too. The lint checks build their own analysis, so a save costs two — which is
  the other reason this does not happen on a keystroke.
- **On a hover, a definition or a completion** the server analyses the target
  owning the file. On a **references** request it analyses every target in the
  repository, as one compilation. A definition in a build file analyses nothing
  at all: it loads the repository and asks the graph.

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

Any editor that speaks that protocol can start `buri lsp` for files matching
`*.buri`. A Zed extension ships in `editors/zed`.
