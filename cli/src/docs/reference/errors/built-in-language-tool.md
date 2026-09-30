---
title: A built-in language keeps its own tools
message: '`{field}` cannot be set on `{language}`, a built-in language'
note: a built-in language means the same thing in every repository, so a block for one may only add extensions
fix: delete the field
reproduction: none
---
# A built-in language keeps its own tools

```textproto schema=repo
language {
    name: "jsonc"
    extensions: [".code-workspace"]
}
```

If `json` could be checked by a tool of your own, a `.json` file would mean
something different in each repository, and a schema that passed here could
fail next door.
