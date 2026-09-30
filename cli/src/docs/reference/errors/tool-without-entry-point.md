---
title: A tool has the entry point it is asked for
message: '`{tool}` has no `{entry}` entry point'
note: a `generators` entry runs its tool's `generate`, and a language's `check`, `format` and `generate` each run the entry point of that name
fix: 'add `{entry} {{}}` to the `tool` rule and export `{entry}` from its `tool.buri`, or name a tool that has one'
reproduction: none
---
# A tool has the entry point it is asked for

```textproto schema=build
tool {
    check {}
    format {}
}
```

This tool has `check` and `format`, so a language may name it for either, and a
`generators` entry may not name it at all. `std/json` has `check` and `format`;
`std/proto` has `generate`.
