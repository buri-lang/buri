# Set up your editor

`buri lsp` is a language server speaking the Language Server Protocol over stdin
and stdout. It serves the same analysis `buri build` runs, so your editor shows
what a build would say: diagnostics, hover, go-to-definition, references,
rename, completion, formatting, the outline, inlay hints, signature help, code
actions and code lenses. All of it in `.buri` sources and in `BUILD.buri` and
`REPO.buri` alike.

## Zed

Zed › Extensions › *Install Dev Extension*, and choose `editors/zed` from a
checkout. Zed compiles the extension itself, which needs the wasm target:

```text
rustup target add wasm32-wasip2
```

The extension starts `buri lsp` from your `PATH` and downloads no toolchain.

It registers two languages, because a `.buri` file is not always Buri: **Buri**
for source, and **Buri Build** for `BUILD.buri` and `REPO.buri`, which are
textproto.

Zed reads semantic tokens only when asked, and they are what colours a local
against a parameter and a method against a function. In `settings.json`:

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
layers, styling token types, and what to do when the extension fails to
build.

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

You pass nothing on the command line, and there is no settings file to write.
The server takes its configuration from the `initialize` handshake and finds its
repository from the root the client sends.

Two things a client has to get right. Only the protocol goes to stdout, so a
wrapper that echoes into that stream looks like a broken editor — the server's
own log lines go to stderr. And the server answers requests one at a time in
arrival order, so a client that pipelines gets its answers back in the order it
asked.

## Syntax highlighting without the server

`editors/tree-sitter-buri` and `editors/tree-sitter-buri-build` are tree-sitter
grammars: the first for `.buri` sources, the second for the two build files. Any
editor with a tree-sitter integration builds them the usual way. The Buri one's
`grammar.js` is generated from the normative `cli/src/docs/grammar.ebnf`, and a
cargo test compares the two byte for byte, so it cannot drift from the language.

A grammar knows where a name is *written*, not what it means. That is the gap
semantic tokens close.
