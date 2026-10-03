---
title: A schema is a generator's input
message: '`proto_sources` is retired'
note: the field was the one hard-wired generator in the build, and `.proto` was the one language it knew — `generators` runs any tool's `generate`, and `proto` is the tool for schemas
fix: move the schemas into `generators: [{{ tool: "proto", inputs: [...] }}]`
reproduction: none
---
# A schema is a generator's input

```text
error: `proto_sources` is retired [retired-proto-sources]
```

```textproto schema=build
library {
    generators: [
        { tool: "proto", inputs: ["address.proto", "point.proto"] },
    ]
    visibility: ["//visibility:public"]
}
```

Nothing else moves. Imports still write `//lib/wire/point.proto`, the modules
still belong to the rule that declared them, and `proto-*` diagnostics keep
their spans.

`buri gen` doesn't write `generators`: it's hand-authored, like `visibility` and
`outputs`, because nothing can tell which generator owns a new file. A schema no
entry lists is [`unused-source`](../lints/unused-source.md).

`proto` is a tool written in Buri, and its `generate` is what the old field ran.
See [`generators.md`](../build/generators.md).
