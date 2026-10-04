---
title: A tool declares every entry point it exports
message: '`{entry}` is exported, and the `tool` rule has no `{entry}` block'
note: the blocks in `BUILD.buri` are what a reader of the build sees, so each entry point is declared there too
fix: 'add `{entry} {{}}` to the `tool` rule, or stop exporting `{entry}`'
reproduction: none
---
# A tool declares every entry point it exports

`check`, `format` and `generate` are the entry point names. A tool that exports
one without its block would have an entry point nothing could reach, and one
that reads the `BUILD.buri` would not know it was there.
