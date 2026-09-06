# tree-sitter-buri

The Buri grammar for tree-sitter.

`grammar.js` is **generated** from
[`cli/src/docs/grammar.ebnf`](../../cli/src/docs/grammar.ebnf), which is the
normative grammar and the only place Buri's syntax is written down. Do not edit
it: edit the EBNF and run

```
BURI_BLESS=1 cargo test -p buri --test language corpus::the_tree_sitter_grammar
```

The EBNF carries everything tree-sitter needs beyond a context-free grammar —
node names, hidden rules, field names, the external scanner's terminals, and the
precedence cascade — as directives in its own comments. Its header documents
them, and `cli/src/documentation/grammar.rs` turns them into this file. A cargo
test regenerates the grammar and compares it byte for byte against the copy here,
so the two cannot drift.

One file is hand-written, and stays that way:

- `src/scanner.c` — an external scanner, for the two things tree-sitter's lexer
  cannot express: string interpolation, where the `}` closing a hole is not the
  `}` closing a block, and nestable block comments. Both need a lexer with state,
  which no declarative grammar can describe.

`tree-sitter generate` produces everything else in `src/`. Two of its products
are checked in anyway, `src/parser.c` and `src/tree_sitter/`, because Zed
compiles them — see [Publishing](#publishing). The rest stays out.

## Checking it

```
./check.sh
```

The generator proves the grammar says what the EBNF says. `check.sh` proves the
EBNF says what the compiler does. That is the other half, and it needs the
tree-sitter CLI, so it is a script rather than a `cargo test` — the toolchain may
not depend on an external tool.

It asks the toolchain, live:

```
cargo run -q -p buri --example parse_verdicts < paths
```

That prints `parses` or `rejects` for each path. Nothing gets recorded in
between. A checked-in file of verdicts would only read back what the compiler
does, and it would go stale exactly when the answer starts to matter. `check.sh`
then holds the syntax tree to that answer in **both** directions:

- a source the parser accepts must have zero `ERROR` and zero `MISSING` nodes;
- a source the parser rejects must have at least one.

A corpus of working programs cannot check that second direction, and it is what
says the grammar has not quietly become more permissive than the language. It
found exactly that on its first run: `export fn` inside an `impl ... for ...`,
which the previous hand-written grammar accepted and the compiler does not.

`check.sh` also compiles every query in
[`../zed/languages/buri`](../zed/languages/buri). A highlight query naming a node
the grammar no longer has fails silently: the editor just stops colouring.

## What it does not do

Four files, listed in `check.sh` with a reason each, are where the compiler and
the syntax tree *should* disagree. All four make the same argument: a grammar
that refuses the program the compiler's own error message is about would replace
a sentence with a red squiggle.

- **Reserved words.** `while` is not a keyword in Buri, it is a word the lexer
  refuses, and tree-sitter parses it as an identifier. The language server
  reports it instead.
- **Keywords where a name belongs.** `fn test(...)`. tree-sitter's keyword
  extraction reads the word as an identifier, which is what makes its error
  recovery work at all.
- **Chained comparison.** The EBNF writes comparison as non-associative, so
  `a < b < c` is not derivable from it. tree-sitter has no word for
  non-associative, so the grammar gives comparison left-associativity and lets
  the compiler's `chained-comparison` message be what a reader sees.

The list may only shrink. `check.sh` reports any file on it that has started to
agree, so nobody can quietly park a problem there.

## Publishing

`../zed/extension.toml` fetches the grammar from a git repository by commit —
this repository, at `path = "editors/tree-sitter-buri"`. Zed shallow-clones that
commit and compiles `src/parser.c` and `src/scanner.c` with clang. It never runs
`tree-sitter generate`, which is why we commit the generated parser and the
`src/tree_sitter/` headers it includes rather than ignoring them.

Two consequences. The pinned commit has to be **pushed**, because Zed fetches it
from GitHub and not from the checkout you are sitting in. And a change to
`grammar.js` reaches no editor until you run `tree-sitter generate`, commit and
push the regenerated parser, and point `commit` in `../zed/extension.toml` at
it.
