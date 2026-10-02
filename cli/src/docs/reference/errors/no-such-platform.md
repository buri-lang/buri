---
title: A platform is one the toolchain bundles
message: '`{platform}` is not a platform'
note: the platforms bundled with the toolchain are `"native"`, `"node"` and `"web"`
fix: name one of them
reproduction: none
---
# A platform is one the toolchain bundles

```textproto schema=build
binary {
    outputs: [
        { platform: "node" },
    ]
}
```

A platform name is a string. `node` runs under bun or Node, `web` is a page in
a browser, and `native` is an executable for the `variant` it names.
