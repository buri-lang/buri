---
title: A native output names its variant
message: 'a `{platform}` output names a variant'
note: 'a variant says how an output is built: here, the operating system and the architecture'
fix: 'add one of {variants}, as `variant: "{example}"`'
reproduction: none
---
# A native output names its variant

```textproto schema=build
binary {
    outputs: [
        { platform: "native", variant: "linux-arm64" },
        { platform: "native", variant: "macos-arm64" },
    ]
}
```

Each variant builds into its own directory, `.buri/out/native/<variant>/`.
`buri test` and `buri run` build the machine's own variant without being told.
