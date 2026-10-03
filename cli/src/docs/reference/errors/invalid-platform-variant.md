---
title: A native variant names an operating system and an architecture
message: '`{variant}` is not a native variant'
note: 'a `NATIVE` entry builds for `<os>-<arch>`: `linux` or `macos`, then `arm64` or `x86_64`'
fix: 'write it as `linux-arm64`, `linux-x86_64`, `macos-arm64` or `macos-x86_64`'
reproduction: none
---
# A native variant names an operating system and an architecture

```text
error: `arm64` is not a native variant [invalid-platform-variant]
```

```textproto schema=build
# platform/lambda/BUILD.buri
platform {
    entry {
        name: "bootstrap"
        backend: NATIVE
        variants: ["linux-arm64", "linux-x86_64"]
    }
}
```

A `JS` entry's variants are free-form names.
