---
title: A platform has an entry
message: this `platform` rule declares no entries
note: an output builds one artifact per entry, so a platform without one has nothing to build
fix: 'add one, as `entry {{ name: "main"  backend: JS }}`'
reproduction: none
---
# A platform has an entry

```text
error: this `platform` rule declares no entries [platform-missing-entry]
```

```textproto schema=build
# platform/cli/BUILD.buri
platform {
    entry {
        name: "main"
        backend: JS
    }
}
```
