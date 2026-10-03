---
title: A toolchain tool is named for its language
message: '`{tool}` is retired'
note: 'a tool this toolchain ships is named for its language with no prefix, and a `//label` names one of your own'
fix: 'name it `{replacement}`'
reproduction: none
---
# A toolchain tool is named for its language

```textproto schema=build
library {
    generators: [
        { tool: "proto", inputs: ["point.proto"] },
    ]
}
```

A built-in tool has a bare name: `json`, `proto` or `textproto`. A `//` label
names your own tool, so the two never collide.

The old names `std/json`, `std/proto`, `std/textproto` and `std/codegen/proto`
are refused, not kept as second spellings.
