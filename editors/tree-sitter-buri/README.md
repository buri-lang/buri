# tree-sitter-buri

The Buri grammar for tree-sitter, transliterated from
[`cli/src/docs/grammar.ebnf`](../../cli/src/docs/grammar.ebnf), which is
normative.

Two files are hand-written and everything else is generated:

- `grammar.js` — the grammar.
- `src/scanner.c` — an external scanner, for the two things tree-sitter's
  lexer cannot express: string interpolation, and nestable block comments.

## Checking it

```
./check.sh
```

Parses every `.buri` source in the repository and asserts zero `ERROR` and zero
`MISSING` nodes, then compiles every query in
[`../zed/languages/buri`](../zed/languages/buri). This is the guarantee that the
grammar and the compiler agree — there is no generator between them, because
two parts of the grammar need shapes a transliteration cannot produce (see the
header of `grammar.js`).

It is not a `cargo test`: it needs the tree-sitter CLI, and the toolchain is not
allowed to depend on an external tool.

## What it does not do

Reserved words. `while` is not a keyword in Buri — it is a word the lexer
refuses — and tree-sitter will happily parse it as an identifier. Encoding that
here would mean a grammar that rejects the program the compiler's own error
message is about, so the language server reports it instead.

## Publishing

`../zed/extension.toml` fetches the grammar from a git repository by commit,
the same way `REPO.buri` pins the toolchain by hash. Until this grammar is
published to one, that entry holds a placeholder commit and the extension can
only be used as a dev extension, which builds the grammar from a local path.
