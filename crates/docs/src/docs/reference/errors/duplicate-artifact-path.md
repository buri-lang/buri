---
title: Each artifact has its own path
message: 'two artifacts of {target} write `{path}`'
note: '{note}'
fix: '{fix}'
reproduction: none
---
# Each artifact has its own path

```text
error: two artifacts of //cmd/app write `.buri/out/web/cmd/app/main.mjs` [duplicate-artifact-path]
```

One `web` output is enough:

```textproto schema=build
binary {
    outputs: [
        { platform: "web" },
    ]
}
```

A platform with several entries names each artifact after its entry, so an
output of it takes no `artifact_name`. Two outputs of one platform with no
entries of their own need different `artifact_name`s.
