---
title: An output names a variant its platform requires
message: '`{platform}` needs a variant'
note: 'available: {variants}'
fix: 'add `variant: "{example}"`'
reproduction: none
---
# An output names a variant its platform requires

```textproto schema=build
binary {
    outputs: [
        { platform: "native", variant: "linux-arm64" },
        { platform: "native", variant: "macos-arm64" },
    ]
}
```

A variant says how an output is built: for `native`, the operating system and
the architecture. A platform's `entry` with `variant_required: true` makes
every output of that platform pick one.

Each variant builds into its own directory, `.buri/out/native/<variant>/`.
`buri test` and `buri run` build the machine's own variant without being told.
