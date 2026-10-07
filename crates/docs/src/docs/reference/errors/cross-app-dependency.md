---
title: An app reaches only its own packages and shared ones
message: '{from} {reaches} {to}, which belongs to another app'
label: another app
note: code under apps/{app}/ reaches only that app and packages outside apps/
fix: move the code both apps need out of {to} into a library under libs/, and depend on that instead
reproduction: none
---
# An app reaches only its own packages and shared ones

```text
error: //apps/web depends on //apps/api, which belongs to another app [cross-app-dependency]
```

```textproto schema=build
# apps/web/BUILD.buri
binary {
    dependencies: ["//apps/api"]
    outputs: [
        { platform: "node" },
    ]
}
```

Everything under `apps/<name>/` is one app. An app's packages may depend on
each other, and on anything outside `apps/`, such as `libs/` and `tools/`.
They may not reach another app's packages, whatever its `visibility` says.

A path through a shared library counts too. `//apps/web -> //libs/bridge ->
//apps/api` is refused at `//apps/web`'s edge, and a note prints the path. An
import of another app's module is refused at the import.

Code two apps share belongs in a library under `libs/`:

```textproto schema=build
# apps/web/BUILD.buri
binary {
    dependencies: ["//libs/greeting"]
    outputs: [
        { platform: "node" },
    ]
}
```
