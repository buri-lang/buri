---
title: An output's entry names the entry and its function
message: 'this item in `entries` has no `{field}`'
note: each item names one of the platform's entries in `name`, and the function filling it in `function`
fix: 'add `{field}: "{example}"`'
reproduction: none
---
# An output's entry names the entry and its function

```textproto schema=build
binary {
    outputs: [
        { platform: "node", entries: [{ name: "main", function: "mainForNode" }] },
    ]
}
```
