---
title: A tool or platform rule lives in its own directory
message: '{package} declares a `{rule}` rule {place}'
label: a {rule} rule here
note: every tool lives under the repository's top-level `tools/` directory, and every platform under `platform/`, so one place lists each
fix: move this package to {destination}, and rename every label that names it
reproduction: none
---
# A tool or platform rule lives in its own directory

```text
error: //lib/routes declares a `tool` rule outside //tools/ [misplaced-rule]
```

```textproto schema=build
# tools/routes/BUILD.buri
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

Any depth works, so `//tools/db/schema` is a tool too. `platform/effect/` holds
effect packages, so no platform lives there. A library or a binary may live
beside either rule, and anywhere else.
