---
title: A schema is a generator's input
message: '`proto_sources` is retired'
note: the field was the one hard-wired generator in the build, and `.proto` was the one language it knew — `generators` runs any tool's `generate`, and `std/proto` is the tool for schemas
fix: move the schemas into `generators: [{{ tool: "std/proto", inputs: [...] }}]`
reproduction: none
---
# A schema is a generator's input

```text
error: `proto_sources` is retired [retired-proto-sources]
```

## What to do

Move every schema the field listed into a `generators` entry, and hand it to
`std/proto`:

```textproto schema=build
library {
    generators: [
        { tool: "std/proto", inputs: ["address.proto", "point.proto"] },
    ]
    visibility: ["//visibility:public"]
}
```

Nothing else moves. The modules are named as they were — `//lib/wire/point.proto`
is still what an import writes — they still belong to the rule that declared
them, and every `proto-*` diagnostic still comes from the same schema with the
same span.

One thing does change: `buri gen` no longer writes the field. `generators` is
hand-authored, like `visibility` and `outputs`, because nothing can work out
which generator owns a new file. A schema no entry lists is
[`unused-library`](../lints/unused-library.md).

## Why

`proto_sources` was a generator the build could not name. The rule ran one
program, on one language, and a repository that wanted a second kind of
generated code had no way to ask for one.

`generators` is the same idea with the program written down. `std/proto` is a
tool written in Buri, and its `generate` is what the field always ran — so the
schemas, the diagnostics and the generated modules are the ones you already
had, reached through a rule a tool of your own can also use. See
[`generators.md`](../build/generators.md).
