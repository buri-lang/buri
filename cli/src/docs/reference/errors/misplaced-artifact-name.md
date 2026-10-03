---
title: A page keeps its entry's file name
message: '`{platform}` names its own artifacts'
label: renamed here
note: '`{platform}` ships assets, such as an `index.html` that loads `/main.mjs`, which find each entry''s file by its name'
fix: remove `artifact_name`
reproduction: none
---
# A page keeps its entry's file name

```text
error: `web` names its own artifacts [misplaced-artifact-name]
```

```textproto schema=build
binary {
    outputs: [
        { platform: "web" },
    ]
}
```

A platform with `assets` names each artifact after its entry, because the
assets refer to it by that name.
