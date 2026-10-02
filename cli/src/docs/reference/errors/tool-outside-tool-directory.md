---
title: A tool rule lives under the repository's tool/ directory
message: '{package} declares a `tool` rule outside //tool/'
label: a tool rule here
note: every tool a repository runs lives under its top-level `tool/` directory, so one place lists them all
fix: move this package under //tool/, as //tool/{name}, and rename every label that names it
reproduction: none
---
# A tool rule lives under the repository's tool/ directory

```textproto schema=build
# tool/routes/BUILD.buri
tool {
    generate {}
}
```

Any depth under `tool/` works, so `//tool/db/schema` is a tool too. A library
or a binary may live under `tool/` beside a tool, and anywhere else. See
[`tools.md`](../build/tools.md).
