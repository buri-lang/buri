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
| `diagnostic` | The same findings, when the editor asks for them instead of waiting to be told. Quoting the result id of the last answer gets `unchanged` back for the price of a hash. |
| `workspace/diagnostic` | Every file of every open repository, whether or not anything has it open — which is the half a publish cannot reach, because a `BUILD.buri` that does not describe its package is a finding about a file nobody opens. |
| `hover` | On any name — a call, a type, a field, a variant, a local — the declaration's signature and its doc comment, wherever the declaration is. On something that is not a name, the type of the innermost expression under the cursor. |
| `definition` | Where the name under the cursor was declared, for every kind of name a file can write: functions and methods, types in expressions and in annotations, traits in bounds and in `impl` heads, module-level `let`s, locals and parameters, fields, variants, and the names in an import clause. Also the path of an import, and — in a `BUILD.buri` — a dependency label, a source entry and a tag. |
| `typeDefinition` | Where the *type* of the thing under the cursor was declared: a local's type, a field's type, a call's return type. |
| `declaration` | The same answer as `definition`. Buri declares and defines a thing in one place, so answering differently would mean inventing a difference. |
| `implementation` | For a trait, every `impl` and `derive` that conforms to it. For a type, every conformance it was given. For a trait method, the function each `impl` supplied for it. |
| `typeHierarchy` | Above a type, the traits it implements; below a trait, the types that implement it. The two directions the language has something in. |
| `callHierarchy` | Who calls a function or a trait method, and what it calls. A `test` is a caller under the sentence it is written with, and a module-level `let` is one too. |
| `documentLink` | The underlines: the path of every import and re-export, every `http(s)://` address written in a comment, and — in a `BUILD.buri` — every `sources` and `data` entry and every dependency label. |
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
| `rangeFormatting` / `onTypeFormatting` | The same canonical output with the rest withheld: the file is formatted whole, the result is diffed against what is on screen, and only the edits your range — or the declaration the `}` or `;` you just typed closes — actually touches come back. |
| `willSaveWaitUntil` | Format on save, with nothing to configure. Exactly what `formatting` returns, because it is the same function. |
| `completion` | Inside a module path, the standard library plus the labels your target already declares. Inside an import's `{ … }`, what that module exports. Each item carries the range it replaces, its kind, and the signature of what it names. |
| `completionItem/resolve` | The `///` prose for the one item you are on. Every doc comment in a module on the wire to show one of them is what this exists to avoid. |
| `codeAction` / `codeAction/resolve` | The fix for a finding that has exactly one, and the diagnostic it is under. The list is titles; the edit — which for a build file means running `buri gen` — is computed for the action you accept. A client that cannot resolve gets the edit in the list. |
| `codeLens` / `codeLens/resolve` | A run command above every `test`, and above every exported declaration how many places the repository uses it. The count is computed when the editor is about to draw that one lens, not when it scrolls past the file. |
| `executeCommand` | The three verbs the lenses invoke: run one test, regenerate a package's `BUILD.buri`, and show the places a count counted. |
| `documentColor` / `colorPresentation` | A swatch beside every `ui/style` `Color.Rgb` and `Color.Rgba` the file spells out, and the picker's choice written back as the same call. |
| `semanticTokens` | Colour, in two layers: the lexer's keywords, literals and comments, and then every identifier upgraded to what it actually names — a type, a trait, a field, a variant, a module, a method rather than a function. Whole file, one range, or a delta against the last answer. |
| `inlayHint` / `inlayHint/resolve` | The inferred type after every `let` and every closure parameter that wrote none, and the parameter's name before every argument that reads as an unlabelled one. Pointing at a hint resolves it: the declaration it is about, rendered as hover renders it, and a click that goes there. |
| `willCreateFiles` / `willRenameFiles` / `willDeleteFiles` | The edit that keeps the repository building when a file appears, moves or goes away: the `sources` entry in its package's `BUILD.buri`, and — for a rename — every import and re-export in the repository that named the module. |
| `didChangeWatchedFiles` | Everything published again, when a `.buri` file changed on disk without the editor having done it. |
| `didCreateFiles` / `didRenameFiles` / `didDeleteFiles` | The same, once it has happened. A deleted file's findings are cleared, and a renamed file's buffer follows it. |
| `didChangeWorkspaceFolders` | A repository opened or closed while the server is running, and which one owns each file recomputed. |

**Diagnostics include the build-graph findings**, not only the type errors —
`missing-dep`, `unused-import`, and the rest of what `buri lint` reports. An
editor that showed half of what the toolchain knows would be showing the half
that is easier to notice at a terminal anyway.

**Diagnostics arrive both ways, and the protocol allows both.** They are
*pushed* on an open and on a save, which is what every editor gets without
asking for anything. They can also be *pulled*: `textDocument/diagnostic` asks
about one file and `workspace/diagnostic` asks about the repository, and a
client that pulls decides for itself when to ask rather than waiting for a save.
The two producers are the same two either way — the front end and the lint
pass — so what a pull reports about a file is byte for byte what a publish about
it would have carried, `REPO.buri`'s `fail_on_finding` promotion included. There
is no second opinion to drift.

**Pull is what reaches the files nobody opened.** A publish is addressed to a
document, and most of what this toolchain knows is about a file the editor has
never had open: a package two directories away that nothing on screen imports, a
`BUILD.buri` that does not describe its own sources. `workspace/diagnostic`
answers with one report per `.buri` file in every open repository — sources,
every `BUILD.buri`, and `REPO.buri` — including the clean ones, because a report
saying "nothing here" is how a client is told the error it was showing is fixed.

**A result id is the state the answer was computed from.** Every report carries
one, and a client that quotes it back is asking "is anything my report was
computed from different now". That comparison is the analysis fingerprint — the
same hash the analysis cache is keyed on — so an unchanged answer costs a read
of the repository rather than a compilation of it. One id per repository rather
than per file: the front end is whole-closure, so there is no file whose answer
an edit elsewhere provably does not change, and a per-file id would be a promise
nothing can keep. Two open repositories keep their own, and an edit in one
leaves the other's current.

**`interFileDependencies: true`** is advertised for the same reason, and it is a
fact about the language rather than a preference: editing a library changes what
is wrong in the binary that imports it. A client reading `false` there would
re-pull only the file you touched and would go on showing an error somewhere
else that no longer exists.

**Related documents.** A client that claims `relatedDocumentSupport` gets, along
with the report it asked for, what the same two passes found in the *other*
files of that file's closure — a type error in the library, the finding about
the build file. A client that did not claim it is not left without them:
`workspace/diagnostic` is where they go.

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

**A call hierarchy lists calls, not references.** The scan is the calls each
checked body writes: a call of a function, a function named as a value, and a
call of a trait method. A name in an import clause is a reference to a function
and not a call of it, and `a + b` is a trait method the source never spelled —
neither is in the panel, which is what makes every row somewhere a reader can go
and look. The two directions are the same scan: incoming runs it over every body
and keeps the ones that mention the symbol, outgoing runs it over the one body
and keeps what it mentions. The whole repository is analysed, because a function
is called from wherever it is imported.

A **`test` is a caller with a name of its own.** The tables file one under a
generated name — nothing calls a test — so the item carries the sentence it was
written with instead, and points at that sentence. A **module-level `let`** is a
caller too: its value is checked on its own, with no body around it, and a list
of callers that quietly dropped one would be wrong rather than shorter. It comes
back as a `Constant` item and walks like any other.

Three answers are empty rather than absent. A **trait method calls nothing**: a
Buri `trait` declares signatures and writes no default body, so there is no body
here to walk and the impls' calls belong to the impls. A **function nobody
calls** answers with an empty list. And a call of a **standard-library**
function contributes no row at all, because those declarations are compiled into
the binary and have no file to point at — the same silence `definition` answers
with there. Asking for the hierarchy of anything that is not a function or a
trait method answers nothing at all: "nothing calls this struct" is not a
shorter answer than the list, it is not the question.

**A link is a different affordance from go-to-definition.** An editor draws every
link at once, without a cursor, and follows it on a click — so the answer covers
things no position request can reach. An address in a `///`, `//!` or `//`
comment is not a name, and `definition` will never have anything to say about
one. Which text is a comment is asked of the lexer rather than guessed at: every
token's span is code, so a run that is inside none of them is a comment — which
is why the address inside `"https://example.org/shop"` is *not* a link, where a
scan for `//` would have found an import path instead. A run ends at the first
byte that cannot continue an address, and trailing sentence punctuation is
dropped, because a full stop after a link is a full stop.

The other two producers are the paths that already resolve. An import's or a
re-export's path string is underlined to the file it names, by exactly the
resolution `definition` performs on the one under the cursor — so a `core/…`
path, which is compiled into the binary, gets no underline. In a `BUILD.buri` a
`sources` or `data` entry is the file beside it and a dependency label is a
package, which is its `BUILD.buri`; `//visibility:public` is in a label field
and names no package, so it gets nothing. A `tags` entry is deliberately not a
link: what a tag names is a block inside `REPO.buri`, a line rather than a file,
and a `DocumentLink` has nowhere to put a line. `definition` still answers for
it.

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

- a module, which is named by its path: renaming one means moving its file, and
  the editor's own rename of that file is what asks for it — see *Renaming a
  file is renaming a module* below;
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

**A completion item says what it replaces.** A module path has a `/` in it and a
client's idea of a word does not, so an item that carried only a label left the
editor guessing which characters an accepted `core/order` was meant to stand in
for — and it guessed `order`. Every item now carries the range: the path typed
so far inside the quotes, or the partial name inside the braces. `detail` says
the thing the label cannot: for a path, whether it is this package, one your
target already declares, or the standard library; for an exported name, the
signature the formatter would print. Sorting is by kind before spelling, because
alphabetical order puts every capitalized name above every lowercase one, which
is a fact about ASCII rather than about what you are looking for. The `///`
prose is the one thing left out, and `completionItem/resolve` supplies it for
the row you are on — every doc comment in a module is a page of text on the wire
to show one line of it.

**Three requests read a parse and nothing else** — the outline, the folds and
the selection chain. No workspace, no standard library, no analysis: they are
questions about shape, and asking them of the buffer alone is what makes them
work in a file that does not typecheck. A fold ends on the last line of the
body rather than on the closing brace's own line, so collapsing a function
leaves its `}` visible; that is exact rather than a guess, because `buri format`
is canonical and puts a closing brace on a line of its own.

**A range is the whole-file answer with the rest withheld.** `buri format` has
no partial mode and is never asked for one: the file is formatted whole, the
result is diffed against the buffer line by line, and the edits handed back are
the ones the requested range touches. So "format the selection" cannot disagree
with "format the file", because it *is* that answer minus what you did not ask
for — and a formatter that could reindent a region differently from the file
around it is a formatter a repository cannot check in. `onTypeFormatting` is the
same computation with the range chosen for you: a `}` closes something, the
something it closes is inside exactly one declaration, and that declaration is
the scope. A `;` works the same way. A file mid-edit that does not parse is left
alone, whole or by the range, because the formatter refuses to emit anything it
could not read back.

**Format on save is the same function.** `willSaveWaitUntil` returns exactly
what `textDocument/formatting` returns, so a file written by a save and a file
written by the format command cannot differ, and there is nothing to configure
to make them agree. `willSave` — the notification — is deliberately not
advertised: the server has nothing to do before a save that it does not do on
`didSave`.

**The fixes are the same ones `buri lint --fix` applies**, and they are applied
the same way: a finding about a build file is handed to `buri gen`, which
returns the whole file, so a `BUILD.buri` is never byte-edited and the three
paths cannot end up disagreeing. A finding with no mechanical answer — which
edge of a `dep-cycle` to cut — offers nothing rather than guessing.

**A code action is offered before it is computed.** The request arrives every
time the cursor lands on a squiggle, and the fix for a build file costs a
`buri gen` — a writable session and a package walk that nothing caches. So the
list is a title, the kind, and the `diagnostics` array that tells the editor
which squiggle to hang the lightbulb on; `codeAction/resolve` computes the edit,
once, for the action somebody accepted. A client whose `initialize` did not name
`edit` in its `codeAction.resolveSupport` will never send that second request,
so it gets the edit in the list — deferring it would be offering a fix that does
nothing.

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

**Renaming a file is renaming a module.** A Buri module is not a file the
compiler goes looking for: it is a file a `BUILD.buri` lists in `sources`, and a
path other modules write in an import. So dragging `lib/shop/catalog.buri` to
`inventory.buri` in the file tree breaks the repository twice over, and neither
break is in the file that moved. Before the editor performs the move it asks
`workspace/willRenameFiles`, and the answer is both rewrites: the `sources`
entry, and every `from "//lib/shop/catalog"` in every open repository. Creating
a file adds its entry; deleting one removes it.

The imports are found by *resolving* each path rather than by matching the
string, because a module path need not contain the file's name — `//lib/shop`
is `lib.buri`, and renaming that file changes a path that never mentioned it.
A build file is skipped: moving a `BUILD.buri` is not moving a module, it is
deleting a package, and that is a decision rather than a restatement.

**What the answer leaves alone is left alone on purpose.** `dependencies` is
not touched, and a rename that crosses a package boundary is where that shows:
the libraries a package uses are derived from the repository *after* the move,
so they are `buri gen`'s to write and `missing-dep`'s to ask for — the finding
and its code action are already there. The imports a delete leaves dangling are
not rewritten either. What they should have said instead is a judgement, and a
server that guessed would be editing code nobody asked it to; a
`module-not-found` on the line that named it is the honest answer, and it says
where the decision is.

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

**Semantic tokens are two layers, and the first one always answers.** Layer one
is the lexer: every keyword, every literal, every comment, coloured from the
token stream with no analysis of any kind behind it. That layer survives a file
that does not parse, does not typecheck and names a module the build graph has
never heard of — which is the state the file you are editing is in most of the
time. Layer two hands each identifier to the same resolver `hover` uses and
colours it by what it names: `interface` for a trait or an effect, `enumMember`
for a variant, `property` for a field, `namespace` for a module alias, `method`
rather than `function` for anything reached through a value. That is the whole
reason for a second answer beside the tree-sitter grammar in `editors/`: a
grammar cannot tell a type name from a value name, and this can. The two do not
disagree, because one is the floor the other builds on.

A name carries the `declaration` and `definition` modifiers where it is written
and neither where it is used — Buri declares and defines in one place, so the
two travel together. A `BUILD.buri` gets no tokens at all: it is textproto, and
the kinds this colours by are the Buri lexer's.

The whole-file answer carries a `resultId`, and the next request may quote it to
get a delta — the edits between what the client is holding and what it would get
now, spliced on whole-token boundaries. A quoted id the server no longer holds
is answered in full rather than with edits against a buffer nobody has, and
closing the document throws the result away with it. A range answer carries no
`resultId`, because a partial result is not a base a delta could be taken
against. Nothing here is incremental underneath: the tokens are recomputed
either way, and what a delta saves is the wire and the client's re-render.

**Inlay hints say the two things the source left out.** A `let` or a closure
parameter that wrote no type gets the inferred one after its name, rendered by
the renderer hover uses, so the hint and the hover cannot disagree about what
something is. An annotated binding gets nothing — the source already said it.
At a call, an argument gets its parameter's name before it when the argument is
a literal, or a bare name that differs from the parameter's: `line(count: 1,
price: each)`, and no `count:` in front of an argument already spelled `count`.
Anything with structure in it — a nested call, an arithmetic expression — is
long enough to read on its own and gets nothing. An operator is a trait call in
the tables and is *not* a call site in the source, so `running + n` is never
labelled: an argument is text that follows a `(` or a `,`.

A hint carries a position, a label in parts, a kind and its paddings, and
nothing else. Everything expensive is in `inlayHint/resolve`, which the client
asks only for the hint you are pointing at: the tooltip is the declaration's
signature and its doc comment, and the part of the label that names something
gets a location, so `Money` in `: Money` and `count` in `count:` are places you
can go to. A hint about a declaration compiled into the binary — `I64`, a
primitive's method — carries no `data` and resolves to itself, because the
label already says everything there is to say about it.

**A code lens is a line of screen, and it is spent on two things.** Above every
`test`, a command that runs that one test, titled with the sentence the test was
written with — a column of lenses all reading "Run test" says nothing about
which line each one belongs to. Above every declaration the module **exports**,
how many places the repository uses it. A declaration the module does not export
gets none: the count would be that one file's uses, which is the question
`documentHighlight` already answers by painting them.

The full pass reads a parse and nothing else, and that is the whole design. A
client asks for the lenses of every file it scrolls through, so anything the
pass does is paid per file scrolled — and counting references means analysing
the entire repository. So the count is not in it. A reference lens leaves with a
range and a `data` and *no command*, and `codeLens/resolve` does the counting
for the one lens an editor is about to draw; a run lens leaves complete, because
all it needed was the sentence and the parse has that. Being a parse also means
the lenses arrive in a file that does not typecheck, and in one that does not
parse the declarations above the break still get theirs.

The count leaves the declaration itself out. The lens is drawn on that line, and
a "3 references" that included the line the reader is looking at would be
counting the reader's own cursor — the same `includeDeclaration: false` a client
sends when it asks "where else is this used". Zero is an answer worth having on
screen: an export nothing reaches is the finding `buri lint` reports as dead
code, said before you have to run it. A lens whose `data` names a file in no
open repository comes back exactly as it went in, because the protocol's result
for that request is a code lens and an unresolved one is still one.

**Three commands, and they are the verbs behind the lenses.** Each is a call
into an entry point `buri` already has at a terminal, so a command and the
command line cannot end up doing different things.

- `buri.runTest` takes the file and the sentence. The file says which repository
  and which target — the same rule every other request follows — and the
  sentence is what `--filter` takes, so a name that is a substring of another
  test's runs both, exactly as it does at the terminal. This compiles and links
  a test binary from a language-server request, which is the most expensive
  thing any of them does and is precisely what was asked for by clicking the
  lens. The transcript goes back as a `window/logMessage`, because that is where
  a client puts several lines, and its last line as a `window/showMessage`,
  because that is where a client puts one. The compiler's own diagnostics are in
  neither: those are already on screen as squiggles from the analysis.
- `buri.regenerateBuildFile` takes a file in the repository and a package label.
  Two arguments rather than the label alone, because a label is
  repository-relative and a client holding two repositories open would be naming
  a package in both.
- `buri.showReferences` takes what a resolved reference lens carries — the uri,
  the position and the `Location[]` — and does nothing. Showing a list of places
  is the client's affordance, not something a server can do to an editor; the
  command exists so that the lens is a well-formed one, because a `CodeLens`
  with a title and no command is not one and naming a command the server does
  not implement would be naming one it would refuse.

**A command that edits writes through the client, and waits.** The server has no
business writing a file an editor may be holding unsaved, so the whole file
`buri gen` produces goes out as a `workspace/applyEdit` with an id of its own —
and the `executeCommand` is *not answered yet*. It is answered when the client
says what it did with the edit, and with that answer: "regenerated" means the
editor wrote the file rather than that the server asked. A client that refused
the edit gets `applied: false` rather than a silence its caller cannot tell from
success. A package whose build file already says what `buri gen` would write is
told so in a `window/showMessage` — nothing to do is not a failure, and saying
so out loud is what a command invoked from a palette owes whoever invoked it.

**A save can tell the client its colours, its hints, its lenses and its
diagnostics are stale.** After a `didSave` or a `didChangeWatchedFiles` that
moved the analysis fingerprint — the same key the cached analyses are filed
under, so "something changed" is a comparison rather than a guess — the server
sends `workspace/semanticTokens/refresh`, `workspace/inlayHint/refresh`,
`workspace/codeLens/refresh` and `workspace/diagnostic/refresh`. An import that
newly resolves turns a name that had no colour into a type, a binding that had
no inferred type into one, a count into a different count, and a file two
packages away into one with an error in it; nothing else would tell the editor
to ask again. The fingerprint is computed once for all four, because it reads
every byte under every open folder. A save that changed nothing is silent, and
so is a client that did not say in its `initialize` that it accepts the
request — each of the four is gated on its own `refreshSupport`, which the
protocol spells `workspace.diagnostics` for the last of them and in the
singular for the other three.

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
| `textDocument/diagnostic` | served | the pull half, over the same two producers. `previousResultId` is answered `unchanged` when the analysis fingerprint has not moved; a request with no document to answer about is a `-32602 InvalidParams`, because a `DocumentDiagnosticReport` has no null among its shapes |
| `workspace/diagnostic` | served | one report per `.buri` file in every open repository, clean ones included — which is how a client is told a finding is gone, and the only shape that reaches a file no editor has open |
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
| `formatting` | served | whole file. `FormattingOptions` — `tabSize`, `insertSpaces` — is accepted and ignored: the formatter is canonical, and a repository whose indentation depended on the editor that saved it would be one nobody could review |
| `rangeFormatting`, `onTypeFormatting` | served | the whole-file answer, diffed against the buffer and filtered to the hunks the range touches. Nothing partial is computed, so the two cannot disagree with `buri format`. `onTypeFormatting` triggers on `}` and `;` and scopes to the declaration around them; a build file has no such scope and gets nothing |
| `willSaveWaitUntil` | served | the same edits `formatting` returns, from the same function |
| `completion` | served | module paths and import clauses, each item with its own replacement range |
| `completionItem/resolve` | served | the doc comment, which is the half worth withholding |
| `codeAction`, `codeAction/resolve` | served | `resolveProvider: true`. The list carries the standard `diagnostics` array — this used to be a `diagnosticCode` string, which is not a field the protocol has |
| `documentColor`, `colorPresentation` | served | `ui/style`'s `Color`, where every argument is a literal |
| `linkedEditingRange` | complete and empty | `null` at every position. The request is for syntax that spells one name at both ends of a construct; Buri writes each name once, and an `impl … for … { }` closes with a brace |
| `inlineValue` | complete and empty | `[]`. Only ever sent from a client stopped in a debug session, and there is no Buri debug adapter to stop in. It becomes real work the day one ships |
| `willSave` | complete and empty | ignored, and **not** advertised beside `willSaveWaitUntil`: nothing happens before a save that does not happen on `didSave`, and asking a client to send a notification this server drops would be asking for traffic to throw away |
| `didChangeConfiguration` | complete and empty | ignored: there is no setting to change. Every `Flags` the server builds is the default one, and the first real setting turns this into work |
| `notebookDocument/didOpen`, `didChange`, `didSave`, `didClose` | complete and empty | ignored, and `notebookDocumentSync` is not advertised: a Buri module belongs to a target declared in a `BUILD.buri`, and a notebook cell has no target for the toolchain to compile it in |
| `documentLink/resolve` | complete and empty | every link target is computed when the file is scanned — resolving a module path or a package label is a lookup in the workspace the answer came from, not a lazier second question — so `resolveProvider` is `false` and a client that sends it anyway is refused |
| `didChangeWatchedFiles`, `client/registerCapability` | served | the watcher is registered after `initialized`, and only for a client whose `initialize` said it accepts one |
| `didChangeWorkspaceFolders`, `workspace/workspaceFolders` | served | the server asks for the folders when a client that knows about them named none |
| `willCreateFiles`, `willRenameFiles`, `willDeleteFiles` | served | registered for `**/*.{buri,proto}` and files rather than folders. A rename rewrites the `sources` entry and every import that resolves to the moved file; a create adds an entry, a delete drops one. Placing a new file in a package that declares both a library and a binary is the one refusal, and it is `buri gen`'s: which rule owns a file is which entry point reaches it, and nothing reaches a file that does not exist yet |
| `didCreateFiles`, `didRenameFiles`, `didDeleteFiles` | served | the same re-publish a watched change gets. A renamed file's buffer moves with it, because a client is not required to close and reopen one; a deleted file's findings are published empty rather than left on screen |
| `semanticTokens/full`, `/range`, `/full/delta` | served | two layers — the lexer's, which cannot fail, and the resolver's upgrade of every identifier. The grammar in `editors/` is the floor this builds on rather than a second opinion it can contradict |
| `inlayHint`, `inlayHint/resolve` | served | the inferred type after a binding that wrote none, and a parameter's name before an argument that is a literal or a differently-spelled bare name. The judgement is a rule and not a taste: an annotated binding, an argument spelled like its parameter, and an argument with structure in it all get nothing |
| `workspace/semanticTokens/refresh`, `workspace/inlayHint/refresh`, `workspace/codeLens/refresh`, `workspace/diagnostic/refresh` | served | sent after a save or a watched change that moved the analysis fingerprint, each only to a client whose `initialize` claimed that family's `refreshSupport` — spelled `workspace.diagnostics`, plural, for the last of the four |
| `prepareCallHierarchy`, `callHierarchy/incomingCalls`, `callHierarchy/outgoingCalls` | served | both directions are one scan of the calls each checked body writes; a trait method has no body of its own and answers `[]` outgoing |
| `documentLink` | served | import and re-export paths, addresses in comments, and a build file's `sources`, `data` and dependency entries |
| `codeLens`, `codeLens/resolve` | served | a run command above every `test` and a use count above every export. The full pass is a parse, and the count — which costs a whole-repository analysis — waits for `resolve`, which is what `resolveProvider: true` claims |
| `workspace/executeCommand` | served | exactly three commands, each a call into an entry point `buri` already has: `buri.runTest`, `buri.regenerateBuildFile`, `buri.showReferences` |
| `workspace/applyEdit` | served | how a command that edits writes: the server sends the file `buri gen` produced and the command's own answer is what the client says it did with it |
| `window/showMessage`, `window/logMessage` | served | what a command has to report: the transcript of a test run in the log, its verdict on screen |
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
  signature help, a moniker, a `documentColor`, a semantic-token request or a
  `prepareRename`** the server analyses the target owning the file — everything
  those have to decide is inside its closure. A moniker is built from where the
  declaration is, and a file's own closure is where the declarations it names
  are. A colour is a constructor call written in the file, and the file's own
  closure is what checked it. Semantic tokens ask what each identifier in one
  file names, which is the same question `hover` asks — once per identifier
  rather than once, which is a real cost and is why layer one is written to need
  no analysis at all. Inlay hints are the other side of that: they read the same
  analysis and ask the resolver *nothing*, because one scan of the syntax and
  one walk of each typed body answer for the whole file at once.
- **On a `textDocument/diagnostic`** it does exactly what an open or a save
  does — the target's closure through the front end, and the lint pass over the
  same target — unless the client quoted the result id it was last given and
  nothing has moved, in which case it reads the repository's bytes and answers
  `unchanged` without compiling anything. **On a `workspace/diagnostic`** it
  analyses every target in every open repository as one compilation, and runs
  the lint catalogue over every target too; a repository whose id the client
  quoted and whose files have not moved costs the hash and nothing else.
- **On an `inlayHint/resolve`** it analyses the file the hint's `data` names,
  which is where the declaration is rather than where the hint is painted, and
  asks the resolver once — for the one hint under the pointer. **On a
  `completionItem/resolve`** it analyses the target owning the file the item
  came from, which is the same analysis the list itself was built from.
- **On any of the three formatting requests and on a `willSaveWaitUntil`** it
  analyses **nothing**. The formatter reads a parse of the one buffer, and a
  range answer is that same output diffed against the buffer — so the cost of
  formatting a selection is the cost of formatting the file, which is a parse.
- **On a `textDocument/codeAction`** it runs the lint pass over the target
  owning the file, which is what knows the findings and their fixes; the answer
  is titles. **On a `codeAction/resolve`** it does that again and then, for a
  fix that rewrites a build file, opens a writable session of its own and makes
  the package walk `buri gen` makes. That last part is not cached by anything,
  which is exactly why it waits for the one action somebody accepted rather
  than running on every cursor move onto a squiggle. A client that did not
  claim `resolveSupport` pays it in the list, because it has no second request
  to pay it in.
- **On a `codeLens`** it analyses **nothing**: the lenses are a parse of the one
  buffer, which is why scrolling through a repository costs a parse per file and
  not a compilation per file. **On a `codeLens/resolve`** it analyses every
  target in the repository, because the count is the `references` answer and
  that is what `references` needs — for the one lens the editor is about to
  draw.
- **On a references, a rename, a workspace symbol query, an implementation,
  either half of a type hierarchy or any of the three call-hierarchy requests**
  it analyses every target in the repository, as one compilation, because a name
  is used — and implemented, and called — wherever it is imported, and nothing
  about the file it was declared in says which files those are.
- **On a `documentLink`** it analyses the target owning the file, which is what
  resolves an import path; a build file's links are the graph's and need no
  analysis at all. The addresses in the comments need neither, so a file the
  server cannot analyse still gets those.
- **On an outline, a fold or a selection range** it analyses nothing: those read
  a parse of the one buffer. A definition in a build file analyses nothing
  either — it loads the repository and asks the graph. Nor does a
  `colorPresentation`: the range came from the `documentColor` answer, and what
  goes there is decided by the colour and by how the source spelled the call.
- **On a change to a watched file or to the open folders** it asks every open
  buffer's own target again, because a build file decides what every file in its
  package can see and there is no one buffer that changed.
- **On a `workspace/executeCommand`** it does whatever the command does, which
  for `buri.runTest` is a compilation and a link and a process, and for
  `buri.regenerateBuildFile` is the same package walk `buri gen` makes. These
  are the two requests that are not cheap and are not meant to be: each is a
  thing somebody clicked.

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
