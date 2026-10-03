---
title: A platform is bundled or a repository's `platform` rule
message: '`{platform}` is not a platform'
note: the platforms bundled with the toolchain are `"native"`, `"node"` and `"web"`, and a repository's own is a `//platform/` label
fix: name one of them
reproduction: none
---
# A platform is bundled or a repository's `platform` rule

```text
error: `//platform/workr` is not a platform [unknown-platform]
```

```textproto schema=build
binary {
    outputs: [
        { platform: "node" },
        { platform: "//platform/worker" },
    ]
}
```

`node` runs under bun or node, `web` is a page in a browser, and `native` is an
executable for the `variant` it names. A `//platform/` label names a package
under `platform/` holding a [`platform` rule](../build/platforms.md).
