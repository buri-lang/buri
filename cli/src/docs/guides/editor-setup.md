# Set up your editor

`buri lsp` is a language server over stdin and stdout. It runs the same analysis
as `buri build`, so your editor shows what a build would say: diagnostics,
hover, go-to-definition, references, rename, completion, formatting, the
outline, inlay hints, signature help, code actions and code lenses. It works in
`.buri` sources and in `BUILD.buri` and `REPO.buri`.

## Zed

Open Zed › Extensions › *Install Dev Extension* and choose `editors/zed` from a
checkout. Zed compiles the extension itself, which needs the wasm target:

```text
rustup target add wasm32-wasip2
```

The extension runs `buri lsp` from your `PATH`. It registers two languages:
**Buri** for source, and **Buri Build** for `BUILD.buri` and `REPO.buri`, which
are textproto.

Turn on semantic tokens to colour locals apart from parameters and methods apart
from functions. In `settings.json`:

```json
{
  "languages": {
    "Buri": {
      "semantic_tokens": "combined"
    }
  }
}
```

`"combined"` layers the server's tokens over the grammar's, which is what the
extension expects. `editors/zed/README.md` covers colour layers, styling token
types, and fixing a failed extension build.

## Any other editor

Point the client at `buri lsp` for `*.buri`, with the directory holding
`REPO.buri` as the workspace root:

```json
{
  "command": ["buri", "lsp"],
  "filetypes": ["buri"],
  "root_markers": ["REPO.buri"]
}
```

There are no flags and no settings file. The server reads its configuration
from the `initialize` handshake and finds the repository from the client's root.

Two things a client must get right:

- Only the protocol goes to stdout, and the server logs to stderr. A wrapper
  that echoes into stdout breaks the editor.
- The server answers requests one at a time, in arrival order.

## Syntax highlighting without the server

`editors/tree-sitter-buri` (for `.buri` sources) and
`editors/tree-sitter-buri-build` (for the build files) are tree-sitter grammars;
build them the usual way for your editor. The Buri one's `grammar.js` is
generated from `cli/src/docs/grammar.ebnf`, and a test keeps the two in sync.

A grammar knows where a name is *written*, not what it means. Semantic tokens
close that gap.
