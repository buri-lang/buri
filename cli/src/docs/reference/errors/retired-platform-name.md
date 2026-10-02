---
title: A platform is a string, and a suite names a backend
message: '`{name}` is retired'
note: an output names a platform as a string, `"native"`, `"node"` or `"web"`, and code that relies on one backend says so in `backends`
fix: '{replacement}'
reproduction: none
---
# A platform is a string, and a suite names a backend

```textproto schema=build
binary {
    outputs: [
        { platform: "native", variant: "linux-arm64" },
        { platform: "node", entries { main: "mainForNode" } },
        { platform: "web" },
    ]

    test {
        backends: [JS]
    }
}
```

| Was                                 | Now                                               |
| ----------------------------------- | ------------------------------------------------- |
| `platform: LINUX, arch: ARM64`      | `platform: "native", variant: "linux-arm64"`      |
| `platform: JS`                      | `platform: "node"`                                |
| `platform: WEB`                     | `platform: "web"`                                 |
| `entry: "run"`                      | `entries: { main: "run" }`                        |
| `js { module: ESM }`                | nothing: every JavaScript output is an ES module  |
| `test { platforms: [JS] }`          | `test { backends: [JS] }`                         |
| `platforms: [LINUX, MACOS]`         | `backends: [NATIVE]`                              |
| `platforms: [JS]`                   | `backends: [JS]`                                  |
| `platforms: [WEB]`                  | `platforms: ["web"]`                              |

A library or a tag's `requires` and `forbids` take both lists, and a platform
must satisfy every list written.
