---
title: An output names a variant its platform requires
message: '`{platform}` needs a variant'
note: 'available: {variants}'
fix: 'add `variant: "{example}"`'
reproduction: none
---
# An output names a variant its platform requires

```text
error: `native` needs a variant [missing-platform-variant]
```

```textproto schema=build
binary {
    outputs: [
        { platform: "native", variant: "linux-arm64" },
        { platform: "native", variant: "macos-arm64" },
    ]
}
```

A variant says how an output is built: for `native`, the operating system and
the architecture. An `entry` with `variant_required: true` makes every output
of its platform pick one. `buri test` builds the machine's own variant without
being told.
