---
title: A contract's language has a `generate`
message: '`{language}` has no `generate`, so nothing turns its schema into types'
fix: 'add `generate` to the `{language}` block in `REPO.buri`, or drop the contract'
reproduction: none
---
# A contract's language has a `generate`

```textproto schema=repo
language {
    name: "yaml"
    extensions: [".yaml"]
    check: "//tools/yaml"
    generate: "//tools/yaml_types"
}
```

A contract means typed values, and the types come from the language's
`generate`, asked with `typesOf`. A language without one is read as text.
