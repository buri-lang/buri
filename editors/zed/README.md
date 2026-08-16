# Buri for Zed

Syntax highlighting, outline, indentation, and the language server.

## Installing it while developing

Zed › Extensions › *Install Dev Extension*, and choose this directory.

The extension starts `buri lsp` from your `PATH`. It does not download a
toolchain: a repository pins its own in `REPO.buri`, and an extension that
fetched a different one would be answering questions about a different
compiler than the one `buri build` uses.

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
