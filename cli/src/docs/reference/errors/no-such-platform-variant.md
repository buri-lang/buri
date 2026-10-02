---
title: A variant is one its platform declares
message: '`{variant}` is not a variant of `{platform}`'
fix: '{choices}'
reproduction: none
---
# A variant is one its platform declares

```textproto schema=build
binary {
    outputs: [
        { platform: "native", variant: "linux-x86_64" },
    ]
}
```

`native`'s variants are `linux-arm64`, `linux-x86_64`, `macos-arm64` and
`macos-x86_64`. `node` and `web` have none, so an output of either names no
`variant`.
