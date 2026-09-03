## What it does

```
buri lsp
```

A language server, speaking the Language Server Protocol over stdin and stdout.
Editors start it; you do not run it by hand.

It serves the same analysis `buri build` runs — the front end is a library, and
`driver::analyze` is what the server calls — so what an editor shows you is what
a build would say. Diagnostics, hover, go-to-definition and the rest of the
navigation requests, references, rename, completion, signature help, formatting,
the outline, inlay hints, code actions and code lenses, in `.buri` sources and
in `BUILD.buri` and `REPO.buri` alike.

Only the protocol goes to stdout. Everything the server says out loud is written
to stderr as well, because a stray line on stdout corrupts the stream in a way
that presents as the editor being broken.

## Setting it up

[Set up your editor](../../guides/editor-setup.md) is the task. Any editor that
speaks the protocol can start `buri lsp` for files matching `*.buri`; a Zed
extension and the tree-sitter grammars ship in
[`editors/`](../../../../../editors/).
