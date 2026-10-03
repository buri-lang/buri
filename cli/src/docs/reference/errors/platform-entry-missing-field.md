---
title: A platform's entry has a name and a backend
message: 'this `entry` has no `{field}`'
note: an entry is named by `name`, which the function filling it matches, and built by `backend`
fix: 'add `{field}: {example}`'
reproduction: none
---
# A platform's entry has a name and a backend

```text
error: this `entry` has no `backend` [platform-entry-missing-field]
```

```textproto schema=build
# platform/cloudflare_worker/BUILD.buri
platform {
    entry {
        name: "fetch"
        backend: JS
        js: "fetch.mjs"
    }
}
```
