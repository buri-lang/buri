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
| `implementation` | For a trait, every `impl` and `derive` that conforms to it. For a type, every conformance it was given. For a trait method, the function each `impl` supplied for it. |
| `typeHierarchy` | Above a type, the traits it implements; below a trait, the types that implement it. The two directions the language has something in. |
| `moniker` | A name for the declaration under the cursor that an index somewhere else could resolve: `//lib/shop:catalog.Item.price`. |
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
| `documentColor` / `colorPresentation` | A swatch beside every `ui/style` `Color.Rgb` and `Color.Rgba` the file spells out, and the picker's choice written back as the same call. |
| `didChangeWatchedFiles` | Everything published again, when a `.buri` file changed on disk without the editor having done it. |
| `didChangeWorkspaceFolders` | A repository opened or closed while the server is running, and which one owns each file recomputed. |

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
about the file it was declared in says which targets those are. It is the most
expensive thing the server does, which is why it is kept — see "When it
analyses" below for what the key is.

A place the source never wrote the name is not a reference to it: an operator
standing for a trait method is a use of that method and is not in the list.

**Implementation and the type hierarchy are one table read three ways.** The
checker records every conformance as a `(trait, type)` pair carrying the span of
the `impl` or the `derive` that made it, so all three answers are a filter over
something already computed rather than a search. From a trait you get its
implementors, from a type its conformances, and from a trait method the function
each `impl` supplied for it — the method itself, not the block it sits in. A
`derive`d conformance has no function to point at, because what it stands for is
a fold over the type's shape, so the `derive` is the answer.

The whole repository is analysed for these, for the same reason `references` is:
an `impl` is written in the module that declares its type, and nothing about the
file under the cursor says which targets those modules are in.

The hierarchy has two empty directions, and they are answers rather than gaps. A
trait is under nothing: a Buri `trait` declares methods and names no other
trait. A type has nothing beneath it: Buri has no subtyping. A conformance to a
standard-library trait — everything a `derive Eq` gives you — is left out of the
hierarchy entirely, because those declarations are compiled into the binary and
have no file, which is the same silence `definition` answers with there.

Asking for the hierarchy of anything that is not a type or a trait answers
nothing at all, and the editor does not open a panel it would have nothing to
put in.

**A moniker is a name, not an index.** `//lib/shop:catalog.Item.price` is the two
things that already name a declaration in this language: what an import writes
to reach the module, and the dotted path to the declaration inside it. The colon
is where the package label ends, which nothing else in the string could tell
you — a package may hold a source in a subdirectory, so `//lib/shop/catalog`
alone does not say where the package stops and the module starts. A module
belonging to no package is the standard library, whose path is what an import
writes and so stands where the label would: `core/effect:Alloc`. A declaration
in the package's own root module leaves the module half empty, and the
separating dot goes with it: `//lib/shop:currency`.

The scheme is `buri` and the uniqueness level is `scheme`, which claims exactly
as much as is true: the identifier is unique among everything wearing this
scheme, and v0.3 has no external repositories for it to claim anything about.
The kind is `export` for a declaration the module exports and `local` for one it
does not. Two symbols get no moniker at all: a local, which has no name outside
the body that binds it, and a module, which is named by its path rather than by
a name — the same reason `rename` refuses one.

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

**The file the fix writes is one the editor is not holding**, and that used to
be the end of the story: you accepted the fix, the `BUILD.buri` changed, and the
squiggle it fixed stayed on screen until you typed in the buffer. So after
`initialized` the server registers a watcher for `**/*.buri` — one pattern
covers a source, a `BUILD.buri` and a `REPO.buri`, because all three wear the
one extension — and a `didChangeWatchedFiles` notification re-publishes for
every open buffer. `buri gen` at the terminal and a `git checkout` under the
editor arrive the same way and are answered the same way.

Nothing is invalidated by hand for it. An analysis is kept under a hash of the
bytes it read, so a file that changed on disk has already moved the key; the
notification says *when* to ask again, and the key decides what the answer is.

**A colour is a constructor call, not a string.** `ui/style` declares
`Color.Rgb(Int, Int, Int)` and `Color.Rgba(Int, Int, Int, Float)`, and a swatch
is drawn beside every one the file spells out. It is read from the tree the
checker built rather than scanned out of the text, so `#ff0000` inside a string
is not a colour — the language does not say it is one. A constructor with an
argument that is not a literal is skipped: a swatch beside `.Rgb(shade, 0, 0)`
would be a guess at what `shade` is worth at run time. `Token`, `Transparent`
and `Inherit` get none either, for the same reason from the other direction —
their value is decided by the theme or by the element above.

The picker writes back the spelling it found. `.Rgb(255, 0, 0)` is the shorthand
for a variant whose type context supplies and `Color.Rgb(255, 0, 0)` names the
enum; a replacement that dropped either would leave a file that no longer
resolves, so only the call itself changes. An opaque colour comes back as `Rgb`,
because a fourth argument would have nothing left to say; anything else is
`Rgba`, with its alpha rounded to three decimals — a picker's
`0.5019607843137255` is a rendering of one 8-bit step rather than a number
anybody meant to type.

**Two open folders are two repositories.** A Buri repository is rooted at a
`REPO.buri`, so a client holding two of them is holding two build graphs, two
closures and two sets of labels. The server keeps the roots the client named —
`workspaceFolders`, or `rootUri` from a client that only knows the one — and
resolves every request's file to the repository above it. A `workspace/symbol`
query is asked of all of them, because the query is about the workspace. A file
in no open folder is answered with nothing: it is a real file in some
repository, and answering out of a repository that is merely open would be
answering a question nobody asked.

Folders opened and closed while the server runs arrive as
`didChangeWorkspaceFolders`, and every open buffer is published again — a buffer
whose repository has just been closed has no findings to show, and leaving them
on screen would leave the editor asserting something the server can no longer
stand behind.

## The whole protocol, and what is left

Everything the 3.17 specification defines for a language, and where this server
stands on it. Three things a row can say, and all three are decisions with
reasons rather than gaps nobody noticed: **served**, **complete and empty** —
the honest answer for Buri is nothing, and a golden pins that it is nothing —
and **deferred**, which is work not yet done.

| Request | | |
|---|---|---|
| `publishDiagnostics` | served | |
| `hover` | served | |
| `definition` | served | |
| `declaration` | served | the same answer; Buri has one place |
| `typeDefinition` | served | |
| `implementation` | served | the conformance table, filtered |
| `prepareTypeHierarchy`, `typeHierarchy/supertypes`, `typeHierarchy/subtypes` | served | the same table, walked; a type's traits above it and a trait's implementors below |
| `moniker` | served | `//lib/shop:catalog.Item.price`, `unique: "scheme"` |
| `references` | served | |
| `documentHighlight` | served | every occurrence is `Text`: Buri has no assignment, so there is no write to distinguish from a read |
| `rename`, `prepareRename` | served | |
| `documentSymbol` | served | nested |
| `workspace/symbol` | served | |
| `workspaceSymbol/resolve` | complete and empty | the query already returns a full `Location`, computed from what the scan had in hand, so `resolveProvider` is `false` and there is nothing a resolve could add |
| `signatureHelp` | served | |
| `foldingRange` | served | |
| `selectionRange` | served | |
| `formatting` | served | whole file |
| `completion` | served | module paths and import clauses |
| `codeAction` | served | |
| `documentColor`, `colorPresentation` | served | `ui/style`'s `Color`, where every argument is a literal |
| `linkedEditingRange` | complete and empty | `null` at every position. The request is for syntax that spells one name at both ends of a construct; Buri writes each name once, and an `impl … for … { }` closes with a brace |
| `inlineValue` | complete and empty | `[]`. Only ever sent from a client stopped in a debug session, and there is no Buri debug adapter to stop in. It becomes real work the day one ships |
| `willSave` | complete and empty | ignored: nothing happens before a save that does not happen on `didSave`, and doing the analysis twice would only make the save slower |
| `didChangeConfiguration` | complete and empty | ignored: there is no setting to change. Every `Flags` the server builds is the default one, and the first real setting turns this into work |
| `notebookDocument/didOpen`, `didChange`, `didSave`, `didClose` | complete and empty | ignored, and `notebookDocumentSync` is not advertised: a Buri module belongs to a target declared in a `BUILD.buri`, and a notebook cell has no target for the toolchain to compile it in |
| `documentLink/resolve` | complete and empty | every link target is computed when the file is scanned — workspace path resolution is not lazy — so the resolve will be advertised `false` when `documentLink` itself lands, and is refused until then |
| `didChangeWatchedFiles`, `client/registerCapability` | served | the watcher is registered after `initialized`, and only for a client whose `initialize` said it accepts one |
| `didChangeWorkspaceFolders`, `workspace/workspaceFolders` | served | the server asks for the folders when a client that knows about them named none |
| `semanticTokens` | deferred | the tree-sitter grammar in `editors/` already colours a file, and a second answer to a coloured question is a way for the two to disagree |
| `inlayHint` | deferred | the types are there; what a hint needs is a judgement about where one is help and where it is clutter, which is a rendering decision rather than a lookup |
| `callHierarchy` | deferred | incoming calls are the references scan grouped by the body each is in, and outgoing calls are one body's calls listed; what is missing is only the request's own two-step protocol over an answer `references` already gives |
| `documentLink` | deferred | a link is a URL in a comment; an import path is already `definition` |
| `codeLens` | deferred | a lens spends a line of screen above a declaration, and nothing in the toolchain currently produces a count worth that line |
| `rangeFormatting`, `onTypeFormatting` | deferred | `buri format` is whole-file and canonical. A formatter with no partial mode has nothing to give a range, and formatting part of a file is how an editor and the command come to disagree about it |
| work-done progress | deferred | no request long enough to report progress on |

**Everything not in that table is refused**, with the protocol's own
`-32601 MethodNotFound` and the method's name in the message. A refusal is still
a reply, which is the part that matters: a client that got nothing back waits
forever, and that presents as the server having hung. It used to be
`result: null`, which is worse than an error rather than safer — several
requests have no legal null result at all, so a client asking for one was handed
a shape it could not read. An unknown *notification* is dropped, because a
notification has no reply to give.

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
  signature help, a moniker, a `documentColor` or a `prepareRename`** the server
  analyses the target owning the file — everything those have to decide is
  inside its closure. A moniker is built from where the declaration is, and a
  file's own closure is where the declarations it names are. A colour is a
  constructor call written in the file, and the file's own closure is what
  checked it.
- **On a references, a rename, a workspace symbol query, an implementation or
  either half of a type hierarchy** it analyses every target in the repository,
  as one compilation, because a name is used — and implemented — wherever it is
  imported, and nothing about the file it was declared in says which files those
  are.
- **On an outline, a fold or a selection range** it analyses nothing: those read
  a parse of the one buffer. A definition in a build file analyses nothing
  either — it loads the repository and asks the graph. Nor does a
  `colorPresentation`: the range came from the `documentColor` answer, and what
  goes there is decided by the colour and by how the source spelled the call.
- **On a change to a watched file or to the open folders** it asks every open
  buffer's own target again, because a build file decides what every file in its
  package can see and there is no one buffer that changed.

The reason for the split is that the front end has no incremental mode:
`driver::analyze` is whole-closure. Analysing on every keystroke would mean
re-checking the standard library on every keystroke.

An unsaved buffer is not a problem — the server hands the editor's copy to the
loader in place of the file on disk, so what you see reported is what you are
looking at.

**An analysis is kept until something changes.** The key is a hash of every byte
that fed it: every `.buri` file in that repository — sources, every
`BUILD.buri`, and `REPO.buri` — plus the buffers the editor has open in it,
because those are what the loader actually reads. Reading and hashing bytes is
not parsing them, so the key costs a fraction of the answer it stands for, and a
second question about a repository nobody has touched is a lookup. Any change
anywhere in that repository invalidates all of its answers: the front end is
whole-closure, so there is no smaller unit whose answer an edit elsewhere
provably does not change. The root is in the key too, so a keystroke in one open
repository does not invalidate the other's. Nothing invalidates by hand — a
keystroke, a save, a close and a file written behind the editor's back all move
the hash on their own.

## Two things worth knowing

**Only the protocol goes to stdout.** Every log goes to stderr. A stray line on
stdout corrupts the stream in a way that presents as the editor being broken.

**Requests are handled one at a time**, in the order they arrive. That costs
some latency on a slow analysis and buys determinism: a session is reproducible,
which is what lets `cli/tests/repositories/lsp/` record one as a golden file.

## Editors

Any editor that speaks that protocol can start `buri lsp` for files matching
`*.buri`. A Zed extension ships in `editors/zed`.
