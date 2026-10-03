---
title: An output fills the entries its platform offers
message: '`{platform}` has no entry `{entry}`'
note: each `name` in `entries` is one of the platform's entries, and its `function` fills it
fix: 'its entries are {entries}'
reproduction: none
---
# An output fills the entries its platform offers

```textproto schema=build
binary {
    outputs: [
        { platform: "node", entries: [
            { name: "main", function: "mainForNode" },
        ] },
    ]
}
```

Without `entries`, each entry is filled by the function of the same name in
`main.buri`. The bundled platforms each have one entry, `main`.
