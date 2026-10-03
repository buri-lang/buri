---
title: An output fills the entries its platform offers
message: '`{platform}` has no entry `{entry}`'
note: each `name` in `entries` is one of the platform's entries, and its `function` fills it
fix: 'its entries are {entries}'
reproduction: none
---
# An output fills the entries its platform offers

```text
error: `node` has no entry `start` [unknown-entry]
```

```textproto schema=build
binary {
    outputs: [
        { platform: "node", entries: [
            { name: "main", function: "mainForNode" },
        ] },
    ]
}
```

The bundled platforms each have one entry, `main`. A repository platform's are
its `entry` blocks.
