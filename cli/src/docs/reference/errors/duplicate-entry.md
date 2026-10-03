---
title: An output fills each entry once
message: '`{entry}` is filled twice'
note: an entry has one function, so a second item in `entries` naming it would contradict the first
fix: 'delete the second `{entry}`'
reproduction: none
---
# An output fills each entry once

```textproto schema=build
binary {
    outputs: [
        { platform: "node", entries: [
            { name: "main", function: "mainForNode" },
        ] },
    ]
}
```
