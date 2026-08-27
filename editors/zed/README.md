# Buri for Zed

Syntax highlighting, outline, indentation, and the language server.

## Installing it

Zed › Extensions › *Install Dev Extension*, and choose this directory.

Zed builds the extension itself, and needs two things to do it:

- **The wasm target.** Zed compiles `src/lib.rs` for `wasm32-wasip2`. If your
  Rust toolchain does not have that target it looks for `rustup` to add it, and
  fails if there is no `rustup` either — which is the case in a Nix shell built
  from `pkgs.cargo`. `rustup target add wasm32-wasip2`, or a toolchain declared
  with that target, is the fix.
- **A pushed grammar.** The grammar is fetched from GitHub at the commit
  `extension.toml` pins, not read from the directory next door. A local edit to
  `../tree-sitter-buri` is invisible until it is committed, pushed, and repinned.

The extension starts `buri lsp` from your `PATH`. It does not download a
toolchain: an extension that fetched its own would be answering questions about
a different compiler than the one `buri build` runs.

## What the server provides

Diagnostics, hover, go-to-definition, the outline, formatting, and completion
inside a module path and inside an import clause. See `buri docs cli lsp`.

## Layout

```
extension.toml            id, version, the grammar, the language server
Cargo.toml                zed_extension_api — see the note in the file
src/lib.rs                language_server_command
languages/buri/
  config.toml             suffixes, comments, brackets
  highlights.scm          the one copy; ../tree-sitter-buri/check.sh compiles it
  indents.scm
  outline.scm
```

The grammar is in `../tree-sitter-buri`. Its `check.sh` parses every `.buri`
file in the repository and compiles every query in this directory — run it
after changing either.
