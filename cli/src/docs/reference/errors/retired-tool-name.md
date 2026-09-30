---
title: A toolchain tool is named for its language
message: '`{tool}` is retired'
note: 'a tool this toolchain ships is `std/<language>`'
fix: 'name it `{replacement}`'
reproduction: none
---
# A toolchain tool is named for its language

```textproto schema=build
library {
    generators: [
        { tool: "std/proto", inputs: ["point.proto"] },
    ]
}
```

`std/codegen/proto` was named for what it did. It is `std/proto` now, beside
`std/json`, and the old name is refused rather than kept as a second spelling.
