---
title: An output is the artifact for one platform
message: an output must name a platform
fix: 'add `platform: "native"`, `"node"`, `"web"` or a `//platform/` label'
reproduction: none
---
# An output is the artifact for one platform

```text
error: an output must name a platform [output-missing-platform]
```

```textproto schema=build
binary {
    outputs: [
        { platform: "node", artifact_name: "tool" },
    ]
}
```
