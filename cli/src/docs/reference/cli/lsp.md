## What it does

```
buri lsp
```

A language server, speaking the Language Server Protocol over stdin and stdout.
Your editor starts it; you never run it by hand.

It runs the same analysis `buri build` runs — the front end is a library, and
the server calls `driver::analyze` — so your editor shows you what a build would
say. That covers diagnostics, hover, go-to-definition and the rest of the
navigation requests, references, rename, completion, signature help, formatting,
the outline, inlay hints, code actions and code lenses. It serves all of them in
`.buri` sources and in `BUILD.buri` and `REPO.buri` alike.

Only the protocol goes to stdout. The server writes everything it says out loud
to stderr as well, because a stray line on stdout corrupts the stream, and that
looks like a broken editor.

## Setting it up

Follow [set up your editor](../../guides/editor-setup.md). Any editor that
speaks the protocol can start `buri lsp` for files matching `*.buri`. A Zed
extension and the tree-sitter grammars ship in
[`editors/`](../../../../../editors/).
