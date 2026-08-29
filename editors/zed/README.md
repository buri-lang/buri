# Buri for Zed

Syntax highlighting, outline, indentation, and the language server.

Two languages, because a `.buri` file is not always Buri: **Buri** for source,
and **Buri Build** for `BUILD.buri` and `REPO.buri`, which are textproto. Zed
scores a matched suffix by its length, so the whole file name wins over the
extension and a build file lands on the build grammar.

## Installing it

Zed › Extensions › *Install Dev Extension*, and choose this directory.

Zed builds the extension itself, and needs two things to do it:

- **The wasm target.** Zed compiles `src/lib.rs` for `wasm32-wasip2`. If your
  Rust toolchain does not have that target it looks for `rustup` to add it, and
  fails if there is no `rustup` either — which is the case in a Nix shell built
  from `pkgs.cargo`. `rustup target add wasm32-wasip2`, or a toolchain declared
  with that target, is the fix.
- **A pushed grammar.** Each grammar is fetched from GitHub at the commit
  `extension.toml` pins, not read from the directory next door. A local edit to
  `../tree-sitter-buri` or `../tree-sitter-buri-build` is invisible until it is
  committed, pushed, and repinned.

The extension starts `buri lsp` from your `PATH`. It does not download a
toolchain: an extension that fetched its own would be answering questions about
a different compiler than the one `buri build` runs.

## What the server provides

Diagnostics, hover, go-to-definition, the outline, formatting, and completion
inside a module path and inside an import clause. See `buri docs cli lsp`.

## Colour, in three layers

Nothing here is a fallback for anything else. Each layer answers a question the
one below it cannot.

1. **The lexer.** Keywords, literals and comments, in a file that does not
   parse. The server's `semanticTokens` always has this much to say.
2. **The grammar** — `languages/buri/highlights.scm`, tree-sitter. It knows
   where a name is *written*: a declaration's name, a field label, a call's
   callee, a struct literal's type. It cannot know what a name *means*, so
   where a bare word could be a local, a parameter or a module alias, it leaves
   the word alone rather than guessing. Buri's naming conventions
   (`buri docs lang lexical`) cover the rest: a capitalized word is coloured as
   a type.
3. **The resolver**, over LSP semantic tokens. It knows what each identifier
   resolves to — a trait rather than a type, a variant rather than a field, a
   method rather than a function, a module alias rather than a local — and it
   is the only layer that colours a local or a parameter at its use.

`editors/tree-sitter-buri/check_highlighting.sh` holds layers two and three to
a named colour for every token of one fixture file, so neither can quietly stop
answering.

### Turning on the third layer

Zed reads semantic tokens only when asked. It is **off by default**, so a fresh
install shows layers one and two and none of the third. In Zed's `settings.json`:

```json
{
  "languages": {
    "Buri": {
      "semantic_tokens": "combined"
    }
  }
}
```

`"combined"` puts the server's answers over the grammar's, which is the mode
this extension is written for — the queries are a complete colouring on their
own, and the server upgrades what it can. `"full"` turns tree-sitter off
entirely and leaves a file with no colour at all while the server is starting
or while the repository fails to load. `"off"` is the default.

The server's legend uses the protocol's own type names — `namespace`, `type`,
`interface`, `enumMember`, `property`, `function`, `method`, `variable`,
`keyword`, `comment`, `string`, `number`, `operator` — so Zed maps every one of
them to a theme style with no configuration. To style them differently, or to
give the `declaration` modifier a look of its own:

```json
{
  "global_lsp_settings": {
    "semantic_token_rules": [
      { "token_type": "interface", "style": ["type.interface", "type"] },
      { "token_type": "enumMember", "style": ["constructor"] },
      { "token_type": "variable", "token_modifiers": ["declaration"], "font_weight": "bold" }
    ]
  }
}
```

`dev: open highlights tree view` in the command palette shows which layer won
for the token under the cursor, and `editor: restart language server` is what
picks up a settings change the server has to be re-asked about.

## Layout

```
extension.toml            id, version, the two grammars, the language server
Cargo.toml                zed_extension_api — see the note in the file
src/lib.rs                language_server_command
languages/buri/
  config.toml             suffixes, comments, brackets
  highlights.scm          the one copy; ../tree-sitter-buri/check.sh compiles it
  indents.scm
  outline.scm
languages/buri-build/
  config.toml             BUILD.buri and REPO.buri
  highlights.scm          compiled by ../tree-sitter-buri-build/check.sh
  indents.scm
  outline.scm
```

The grammars are in `../tree-sitter-buri` and `../tree-sitter-buri-build`. Each
has a `check.sh` that parses its half of the repository and compiles the queries
in the language directory beside it — run the one whose grammar you touched.
`../tree-sitter-buri/check.sh` also runs `check_highlighting.sh`, which is the
one that says what colour each token comes out.
