# Set up your editor

`buri lsp` is a language server speaking the Language Server Protocol over stdin
and stdout. It serves the same analysis `buri build` runs, so what an editor
shows you is what a build would say: diagnostics, hover, go-to-definition,
references, rename, completion, formatting, the outline, inlay hints, signature
help, code actions and code lenses — in `.buri` sources and in `BUILD.buri` and
`REPO.buri` alike.

## Zed

Zed › Extensions › *Install Dev Extension*, and choose `editors/zed` from a
checkout. Zed compiles the extension itself, which needs the wasm target:

```text
rustup target add wasm32-wasip2
```

The extension starts `buri lsp` from your `PATH`. It downloads no toolchain, so
the server answering is the compiler `buri build` runs.

Two languages are registered, because a `.buri` file is not always Buri:
**Buri** for source, **Buri Build** for `BUILD.buri` and `REPO.buri`, which are
textproto.

Zed reads semantic tokens only when asked, and they are the layer that colours a
local against a parameter and a method against a function. In `settings.json`:

```json
{
  "languages": {
    "Buri": {
      "semantic_tokens": "combined"
    }
  }
}
```

`"combined"` puts the server's answers over the grammar's, which is what the
extension is written for. `editors/zed/README.md` covers the three colour
layers, styling individual token types, and what to do when the extension fails
to build.

## Any other editor

Point the client at `buri lsp` for `*.buri`, with the directory holding
`REPO.buri` as the workspace root. The shape of it, in whatever the client's
configuration language happens to be:

```json
{
  "command": ["buri", "lsp"],
  "filetypes": ["buri"],
  "root_markers": ["REPO.buri"]
}
```

Nothing is passed on the command line and there is no settings file to write:
the server takes its configuration from the `initialize` handshake and finds its
repository from the root the client sends.

Two things a client has to get right. Only the protocol goes to stdout, so a
wrapper that echoes anything into that stream presents as the editor being
broken — the server's own log lines go to stderr as well for exactly this
reason. And requests are served one at a time in arrival order, so a client that
pipelines gets its answers back in the order it asked.

## Syntax highlighting without the server

`editors/tree-sitter-buri` and `editors/tree-sitter-buri-build` are tree-sitter
grammars — the first for `.buri` sources, the second for the two build files —
and any editor with a tree-sitter integration can build them the usual way.
The Buri one's `grammar.js` is generated from `cli/src/docs/grammar.ebnf`, the
normative grammar, and a cargo test compares the two byte for byte — so it
cannot drift from the language.

Highlighting from a grammar knows where a name is *written* and not what it
means. That is the gap semantic tokens close, and it is why the server is worth
connecting even in an editor that already colours the file.
