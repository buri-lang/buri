---
title: A tool or platform rule lives in its own directory
message: '{package} declares a `{rule}` rule outside {directory}'
label: a {rule} rule here
note: every tool lives under the repository's top-level `tool/` directory, and every platform under `platform/`, so one place lists each
fix: move this package to {destination}, and rename every label that names it
reproduction: none
---
# A tool or platform rule lives in its own directory

```textproto schema=build
# tool/routes/BUILD.buri
tool {
    generate {}
}
```

```textproto schema=build
# platform/lambda/BUILD.buri
platform {
    entry {
        name: "bootstrap"
        backend: NATIVE
        variants: ["linux-arm64", "linux-x86_64"]
        variant_required: true
    }
}
```

Any depth works, so `//tool/db/schema` is a tool too. `platform/effect/` holds
effects, so no platform lives there. A library or a binary may live beside
either rule, and anywhere else. See [`tools.md`](../build/tools.md) and
[`build-files.md`](../build/build-files.md).
