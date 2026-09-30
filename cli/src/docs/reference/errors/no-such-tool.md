---
title: A tool name names a tool
message: '`{tool}` names no tool'
note: a tool is a `//label` naming a `tool` rule in this repository, or `std/json` or `std/proto`
fix: name a package whose BUILD.buri declares a `tool` rule
reproduction: none
---
# A tool name names a tool

```textproto schema=build
library {
    generators: [
        { tool: "//tools/routes", inputs: ["regions.json"] },
    ]
}
```

`//tools/routes` is a package whose `BUILD.buri` holds `tool { generate {} }`.
See [`tools.md`](../build/tools.md).
