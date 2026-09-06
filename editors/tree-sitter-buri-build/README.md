# tree-sitter-buri-build

The grammar for `BUILD.buri` and `REPO.buri`.

Those files end in `.buri` and are not Buri. They are textproto, read by
[`cli/src/build/textproto.rs`](../../cli/src/build/textproto.rs), and the Buri
grammar next door turns every one of them into one long parse error. Two of the
consequences are worse than no colour at all: `#` reads as a syntax error, and an
editor asked to comment a line writes `//`, which the reader refuses and
`buri format` exits on.

Hence a second grammar, and a second language in
[`../zed/languages/buri-build`](../zed/languages/buri-build). Zed scores a
matched `path_suffixes` entry by the length of what it matched, so the whole name
`BUILD.buri` beats the `buri` the other language claims.

`grammar.js` is **hand-written**, unlike the one in `../tree-sitter-buri`. No
EBNF exists to generate it from, because the dialect is not the language — it is
whatever `textproto.rs` reads, and that reader is small enough to mirror rule for
rule.

- a file is a list of fields;
- a field is `name: value`, or `name { ... }` with no colon;
- a value is a string, an integer, a bare word, a `[list]` or a `{ block }`;
- separators are optional — a comma between two fields or two list elements is
  stepped over if it is there and not missed if it is not;
- `#` starts a comment, anywhere.

A bare word is an enum constant or a bool. The reader spells both the same way,
so the tree gives them one node, named `constant`.

`tree-sitter generate` produces everything in `src/`. Two of its products are
checked in anyway, `src/parser.c` and `src/tree_sitter/`, because Zed compiles
them — see [Publishing](#publishing). The rest stays out.

## Checking it

```
./check.sh
```

Every `BUILD.buri` and `REPO.buri` in the repository has to parse with zero
`ERROR` and zero `MISSING` nodes. This corpus needs no recorded list of verdicts
the way the Buri grammar's does: a build file in this repository is one the
toolchain already reads, or the tests around it would not run.

The `REFUSED` list in `check.sh` covers the other direction, the one a corpus of
working files cannot see. It holds inputs `textproto.rs` turns away, and the
syntax tree has to turn them away too. That is what says the grammar has not
quietly become more permissive than the reader. `// not a comment` is on the
list, because that is the mistake this whole language exists to stop.

`check.sh` also compiles every query in
[`../zed/languages/buri-build`](../zed/languages/buri-build). A highlight query
naming a node the grammar no longer has fails silently: the editor just stops
colouring.

## What it does not do

Three rules the reader has that a context-free grammar cannot state. Each stays
with the reader, whose message about it beats a red squiggle:

- **An integer an `i64` does not hold.** `99999999999999999999999` is a
  well-formed number and a rejected one.
- **Nesting past the depth bound.** Thirty-three `{` is a shape like any other.
  The bound exists so a pathological file cannot exhaust the reader's stack.
- **A field name that begins with a digit.** The reader takes one; no field in
  the schema is spelled that way, and an identifier here is a word.

`check.sh` skips one file rather than parsing it:
`cli/tests/repositories/cli/output_selection/repo/cmd/native/BUILD.buri` is a
template whose platform the test harness fills in at run time, because it has to
name a platform that is not the host's.

## Publishing

`../zed/extension.toml` fetches this grammar from a git repository by commit —
this repository, at `path = "editors/tree-sitter-buri-build"`. Zed shallow-clones
that commit and compiles `src/parser.c` with clang. It never runs
`tree-sitter generate`, which is why we commit the generated parser and the
`src/tree_sitter/` headers it includes rather than ignoring them.

Two consequences. The pinned commit has to be **pushed**, because Zed fetches it
from GitHub and not from the checkout you are sitting in. And a change to
`grammar.js` reaches no editor until you run `tree-sitter generate`, commit and
push the regenerated parser, and point `commit` in `../zed/extension.toml` at
it.
