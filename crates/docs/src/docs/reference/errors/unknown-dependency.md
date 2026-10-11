---
title: A dependency names a library
message: '`{dependency}` names no library'
label: no library
fix: name a package whose BUILD.buri declares a `library` rule, or drop it
reproduction: none
---
# A dependency names a library

```text
error: `//lib/mony` names no library [unknown-dependency]
```

```textproto schema=build
binary {
    dependencies: ["//lib/money"]
}
```

A `dependencies` entry is a `//` label naming a package with a `library` rule,
that library's `testing` surface as `//lib/money/testing`, or a repository
platform under `//platform/`. A binary or a tool can't be depended on: move
what both need into a library.
