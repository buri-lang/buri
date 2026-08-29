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
| `typeDefinition` | Where the *type* of the thing under the cursor was declared: a local's type, a field's type, a call's return type. |
| `declaration` | The same answer as `definition`. Buri declares and defines a thing in one place, so answering differently would mean inventing a difference. |
| `references` | Every place the repository names the symbol under the cursor, asked from the declaration or from any use. The declaration itself is included when the client asks for it. |
| `documentHighlight` | The same question narrowed to the file you are in, which is the file the highlights are painted on. |
| `rename` / `prepareRename` | Every reference rewritten as one edit, with the check an editor runs before it prompts you. |
| `documentSymbol` | The outline, nested: an `impl` holds its methods, a struct its fields, an enum its variants. Built from the syntax alone — so it still works in a file that does not typecheck, which is when an outline is worth most. |
| `workspace/symbol` | Every declaration in the repository whose name contains the query, case-insensitively. |
| `signatureHelp` | The callee's signature while the call is being typed, with the parameter the cursor is in marked. Triggered on `(` and `,`. |
| `foldingRange` | What an editor may collapse: every declaration taller than a line, the methods inside an `impl` or a `trait`, and the run of imports at the top as one region. |
| `selectionRange` | Expand-selection: the word, the expression around it, the declaration, the file. |
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

**Rename is that list written back.** The one rule it adds is about spelling. An
alias refers to a declaration without spelling it — in
`import { listing as shown }` both names refer to the same function, and only
the first is the one being renamed — so a rename rewrites the places that spell
the old name and leaves the rest alone. Renaming `listing` to `catalogue` gives
`import { catalogue as shown }`, and every use of `shown` keeps working.

Four things it refuses, each with the reason, because a rename that quietly did
nothing would look like the server had hung:

- a module, which is named by its path: renaming one means moving its file;
- anything declared in the standard library, which is compiled into the binary
  and has no file to edit;
- a tuple field, which is named by its position rather than by a name;
- a replacement that is not a name — asked of the lexer rather than of a
  character rule here, so `fn`, `match` and the words v0.3 reserves are refused
  for the same reason the compiler would refuse them.

`prepareRename` answers the range and the current name, or nothing at all where
one of those four applies, so the editor never prompts for a rename it is about
to be told it cannot have.

A rename edits the *name*; a definition points at the *declaration*. For a
field, a variant and a trait method those differ — a field's declaration is
`export price: I64` and its name is the five characters inside it — and each
request takes the one it needs: what you want to read is the declaration, and
what you need to write is the name.

**Signature help reads the text, not the typed tree.** It is asked for at the
one moment the file does not check: `listing(` has no arguments yet, so the
checker has already reported the arity and replaced the call with poison.
Finding the enclosing `(` in the buffer and resolving only the callee is what
makes the answer arrive while the call is still being written, which is the
only time it is any use. A cursor inside a string or a comment answers nothing
rather than answering about the call that string is an argument to.

**Three requests read a parse and nothing else** — the outline, the folds and
the selection chain. No workspace, no standard library, no analysis: they are
questions about shape, and asking them of the buffer alone is what makes them
work in a file that does not typecheck. A fold ends on the last line of the
body rather than on the closing brace's own line, so collapsing a function
leaves its `}` visible; that is exact rather than a guess, because `buri format`
is canonical and puts a closing brace on a line of its own.

**The fixes are the same ones `buri lint --fix` applies**, and they are applied
the same way: a finding about a build file is handed to `buri gen`, which
returns the whole file, so a `BUILD.buri` is never byte-edited and the three
paths cannot end up disagreeing. A finding with no mechanical answer — which
edge of a `dep-cycle` to cut — offers nothing rather than guessing.

## The whole protocol, and what is left

Everything the 3.17 specification defines for a language, and where this server
stands on it. A deferral is a decision with a reason, not a gap nobody noticed.

| Request | | |
|---|---|---|
| `publishDiagnostics` | served | |
| `hover` | served | |
| `definition` | served | |
| `declaration` | served | the same answer; Buri has one place |
| `typeDefinition` | served | |
| `references` | served | |
| `documentHighlight` | served | every occurrence is `Text`: Buri has no assignment, so there is no write to distinguish from a read |
| `rename`, `prepareRename` | served | |
| `documentSymbol` | served | nested |
| `workspace/symbol` | served | |
| `signatureHelp` | served | |
| `foldingRange` | served | |
| `selectionRange` | served | |
| `formatting` | served | whole file |
| `completion` | served | module paths and import clauses |
| `codeAction` | served | |
| `implementation` | deferred | the implementors of a trait are a real relation and a small scan of the impl table; it is the next one to add, not one this server has a reason to leave out |
| `semanticTokens` | deferred | the tree-sitter grammar in `editors/` already colours a file, and a second answer to a coloured question is a way for the two to disagree |
| `inlayHint` | deferred | the types are there; what a hint needs is a judgement about where one is help and where it is clutter, which is a rendering decision rather than a lookup |
| `callHierarchy` | deferred | incoming calls are the references scan grouped by the body each is in, and outgoing calls are one body's calls listed; what is missing is only the request's own two-step protocol over an answer `references` already gives |
| `typeHierarchy` | deferred | Buri has no subtyping, so the hierarchy has one level and is not one |
| `moniker` | deferred | a name for a symbol that another repository's index could resolve; there is no such index |
| `linkedEditingRange` | deferred | for syntax that writes one name at both ends of a construct. Buri writes each name once |
| `documentLink` | deferred | a link is a URL in a comment; an import path is already `definition` |
| `codeLens` | deferred | a lens spends a line of screen above a declaration, and nothing in the toolchain currently produces a count worth that line |
| `rangeFormatting`, `onTypeFormatting` | deferred | `buri format` is whole-file and canonical. A formatter with no partial mode has nothing to give a range, and formatting part of a file is how an editor and the command come to disagree about it |
| `didChangeWatchedFiles`, work-done progress | deferred | there is no cache to invalidate and no request long enough to report progress on |

## When it analyses

Stated here rather than tuned silently, because it is the difference between a
server that keeps up with typing and one that does not:

- **On a keystroke** — `didChange` — the server re-parses that one buffer. No
  workspace, no standard library, no imports. A keystroke may *add* to what the
  editor is showing and may never take anything away: a buffer that parses
  publishes nothing at all, so the type errors the last analysis found stay on
  screen with the editor moving them as the text moves. A buffer that does not
  parse publishes its parse errors instead, and when it parses again what the
  analysis last said goes back.
- **On open and on save** the server runs the whole front end over the target's
  closure and publishes everything, then runs the lint pass and publishes that
  too. The lint checks build their own analysis, so a save costs two — which is
  the other reason this does not happen on a keystroke. A finding's severity is
  the one the terminal prints, `REPO.buri`'s `fail_on_finding` included.
- **On a hover, a definition, a type definition, a highlight, a completion, a
  signature help or a `prepareRename`** the server analyses the target owning
  the file — everything those have to decide is inside its closure.
- **On a references, a rename or a workspace symbol query** it analyses every
  target in the repository, as one compilation, because a name is used wherever
  it is imported and nothing about the file it was declared in says which files
  those are.
- **On an outline, a fold or a selection range** it analyses nothing: those read
  a parse of the one buffer. A definition in a build file analyses nothing
  either — it loads the repository and asks the graph.

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
