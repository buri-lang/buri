---
title: `json` makes types for the JSON languages alone
message: `json` makes types for the JSON languages alone, so it can't be `{language}`'s `generate`
fix: give {extensions} to `json`, `jsonc` or `json5` instead, or name a tool of your own whose `generate` answers `typesOf`
reproduction: none
---
# `json` makes types for the JSON languages alone

```textproto schema=repo
language {
    name: "jsonc"
    extensions: [".conf"]
}
```

A contract's types come from its language's `generate`. The `json` tool makes
them from a JSON Schema, for files in `json`, `jsonc` or `json5`, so a language
of your own can `check` and `format` with `json` but can't `generate` with it.
Give its extensions to the JSON language they're written in, and a contract on
that language reads them typed.
