---
title: An output fills the entries its platform offers
message: '`{platform}` has no entry `{entry}`'
note: each key in `entries` is one of the platform's entries, and its value is the function that fills it
fix: 'its entries are {entries}'
reproduction: none
---
# An output fills the entries its platform offers

```textproto schema=build
binary {
    outputs: [
        { platform: "node", entries { main: "mainForNode" } },
    ]
}
```

Without `entries`, each entry is filled by the function of the same name in
`main.buri`. The bundled platforms each have one entry, `main`.
