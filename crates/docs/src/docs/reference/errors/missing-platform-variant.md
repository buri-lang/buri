---
title: An output names a variant its platform requires
message: '`{platform}` needs a variant'
note: 'available: {variants}'
fix: 'add `variant: "{example}"`'
reproduction: none
---
# An output names a variant its platform requires

```text
error: `//platform/lambda` needs a variant [missing-platform-variant]
```

```textproto schema=build
binary {
    outputs: [
        { platform: "//platform/lambda", variant: "linux-arm64" },
    ]
}
```

A variant says how an output is built: for a native entry, the operating system
and the architecture. A repository platform's entry with `variant_required:
true` makes every output of that platform pick one. The bundled `native`
platform requires none, and builds the host's when an output names none.
