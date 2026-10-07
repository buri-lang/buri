---
title: Shared code reaches no app
message: '{from} {reaches} {to}, which belongs to an app'
label: an app's package
note: an app's packages are for that app alone, and {from} lives outside apps/
fix: move the code {from} needs out of {to} into a library under libs/, and depend on that instead
reproduction: none
---
# Shared code reaches no app

```text
error: //libs/bridge depends on //apps/api, which belongs to an app [shared-depends-on-app]
```

```textproto schema=build
# libs/bridge/BUILD.buri
library {
    dependencies: ["//apps/api"]
    visibility: ["//visibility:public"]
}
```

A package outside `apps/`, such as a library in `libs/`, a tool in `tools/` or
a platform, is shared. Shared code may not depend on or import any app's
package, whatever its `visibility` says. Move what both need into `libs/`:

```textproto schema=build
# libs/bridge/BUILD.buri
library {
    dependencies: ["//libs/greeting"]
    visibility: ["//visibility:public"]
}
```

A platform's `dependencies` are read with the build graph, so naming an app
there stops every command until it's fixed.
