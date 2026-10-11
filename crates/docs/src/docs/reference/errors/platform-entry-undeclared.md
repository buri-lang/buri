---
title: A platform declares every entry its rule names
message: "`{entry}` fills `{platform}`'s entry `{point}`, which its `platform.buri` doesn't declare"
note: an entry's declaration is the signature the platform calls, and the function filling it is held to that signature
reproduction: none
---
# A platform declares every entry its rule names

```text
error: `go` fills `//platform/ghost`'s entry `go`, which its `platform.buri` doesn't declare [platform-entry-undeclared]
```

Each `entry` in the platform rule has a declaration without a body in
`platform.buri`:

```text
platform {
    entry {
        name: "go"
        backend: JS
        js: "go.mjs"
    }
}
```

```text
export fn go(host: GhostHost, name: Str): Str;
```
