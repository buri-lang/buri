---
title: A tool name names a declared tool
message: '`{tool}` names no tool'
note: a tool is a `//label` naming a `tool` rule in this repository, or a built-in: `json`, `proto` or `textproto`
fix: name a package whose BUILD.buri declares a `tool` rule
reproduction: none
---
# A tool name names a declared tool

```textproto schema=build
library {
    generators: [
        { tool: "//tools/routes", inputs: ["regions.json"] },
    ]
}
```

`//tools/routes` is a package whose `BUILD.buri` holds `tool { generate {} }`.
See [`tools.md`](../build/tools.md).
