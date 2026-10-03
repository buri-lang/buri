---
title: A platform names each entry once
message: 'the platform declares `{entry}` twice'
label: declared again here
note: an output fills each entry by its name, so two entries can't share one
fix: rename one of them, or delete it
reproduction: none
---
# A platform names each entry once

```text
error: the platform declares `main` twice [duplicate-platform-entry]
```

```textproto schema=build
# platform/desktop/BUILD.buri
platform {
    entry {
        name: "main"
        backend: NATIVE
    }
    entry {
        name: "window"
        backend: JS
        js: "window.mjs"
    }
}
```
