# tree-sitter-buri

The Buri grammar for tree-sitter.

`grammar.js` is **generated** from
[`cli/src/docs/grammar.ebnf`](../../cli/src/docs/grammar.ebnf), which is the
normative grammar and the only place Buri's syntax is written down. Do not edit
it: edit the EBNF and run

```
BURI_BLESS=1 cargo test -p buri --test language corpus::the_tree_sitter_grammar
```

The EBNF carries what tree-sitter needs beyond a context-free grammar — node
names, hidden rules, field names, the external scanner's terminals, and the
precedence cascade — as directives in its own comments, and
`cli/src/documentation/grammar.rs` turns them into this file. The EBNF's header
documents them. A cargo test regenerates the grammar and compares it byte for
byte with the copy here, so the two cannot drift.

One file is hand-written, and stays that way:

- `src/scanner.c` — an external scanner, for the two things tree-sitter's lexer
  cannot express: string interpolation, where the `}` that closes a hole is not
  the `}` that closes a block, and nestable block comments. Both need a lexer
  with state, which no declarative grammar can describe.

Everything else in `src/` is produced by `tree-sitter generate`. Two of those
products are checked in anyway — `src/parser.c` and `src/tree_sitter/` — because
they are what Zed compiles; see [Publishing](#publishing). The rest is not.

## Checking it

```
./check.sh
```

The generator proves the grammar is what the EBNF says. `check.sh` proves the
EBNF is what the compiler does, which is the other half and the one that needs
the tree-sitter CLI — so it is not a `cargo test`, because the toolchain is not
allowed to depend on an external tool.

It asks the toolchain, live:

```
cargo run -q -p buri --example parse_verdicts < paths
```

prints `parses` or `rejects` for each path. Nothing is recorded in between —
a checked-in file of verdicts would be a readout of what the compiler does
rather than something to compare against, and it would go stale exactly when
the answer starts to matter. `check.sh` then holds the syntax tree to that
answer in **both** directions:

- a source the parser accepts must have zero `ERROR` and zero `MISSING` nodes;
- a source the parser rejects must have at least one.

The second direction is the one a corpus of working programs cannot check. It
is what says the grammar has not quietly become more permissive than the
language — and it found exactly that when it was first run, on `export fn`
inside an `impl ... for ...`, which the previous hand-written grammar accepted
and the compiler does not.

`check.sh` also compiles every query in
[`../zed/languages/buri`](../zed/languages/buri), because a highlight query
naming a node the grammar no longer has fails silently: an editor just stops
colouring.

## What it does not do

Five files, listed in `check.sh` with a reason each, are where the compiler and
the syntax tree are *meant* to disagree. All of them are the same argument: a
grammar that refuses the program the compiler's own error message is about
would replace a sentence with a red squiggle.

- **Reserved words.** `while` is not a keyword in Buri — it is a word the lexer
  refuses — and tree-sitter parses it as an identifier. The language server
  reports it instead.
- **Keywords where a name belongs.** `fn test(...)`, `fn f(x: Int, self: T)`.
  tree-sitter's keyword extraction reads the word as an identifier, which is
  what makes its error recovery work at all.
- **Chained comparison.** `a < b < c` is not derivable from the EBNF, which
  writes comparison as non-associative. tree-sitter has no word for that, so it
  is given left-associativity and the compiler's `chained-comparison` message
  is what a reader sees.

The list may only shrink: `check.sh` reports a file on it that has started to
agree, so it cannot become a place to put things.

## Publishing

`../zed/extension.toml` fetches the grammar from a git repository by commit —
this repository, at `path = "editors/tree-sitter-buri"`. Zed shallow-clones that
commit and compiles `src/parser.c` and `src/scanner.c` with clang. It never runs
`tree-sitter generate`, which is why the generated parser and the
`src/tree_sitter/` headers it includes are committed rather than ignored.

Two consequences. The pinned commit must be one that is **pushed**: Zed fetches
it from GitHub, not from the checkout you are sitting in. And a change to
`grammar.js` is not live for an editor until `tree-sitter generate` runs, the
regenerated parser is committed and pushed, and the `commit` in
`../zed/extension.toml` names it.
