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

A built-in tool has a bare name: `json`, `proto` or `textproto`. A label
starting with `//` names a tool of your own, so the two never collide.

The built-ins used to be `std/json`, `std/proto` and `std/textproto`, and
before that the proto generator was `std/codegen/proto`, named for what it did.
Each old name is refused rather than kept as a second spelling.
