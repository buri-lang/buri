## What it does

```
buri lsp
```

A Language Server Protocol server over stdin and stdout, which your editor
starts for you.

It runs the same analysis as `buri build`, so your editor shows what a build
would say. It serves diagnostics, hover, go-to-definition and the other
navigation requests, references, rename, completion, signature help,
formatting, the outline, inlay hints, code actions and code lenses, in `.buri`
sources, `BUILD.buri` and `REPO.buri` alike.

A JSON file a generator lists is checked against its schema as you type and
formatted like `buri format` would. A file in a language of your own is checked
and formatted by its tool's `check` and `format`. A save runs the whole
analysis.

Only the protocol goes to stdout; logging goes to stderr.

## Setting it up

Follow [set up your editor](../../guides/editor-setup.md). Any editor that
speaks the protocol can start `buri lsp` for `*.buri` files. A Zed extension
and the tree-sitter grammars ship in [`editors/`](../../../../../editors/).
