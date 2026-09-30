---
title: A contract names a language the repository has
message: '`{language}` is not a language this repository has'
note: 'the languages are {known}'
fix: 'name one of them, or declare `{language}` in `REPO.buri`'
reproduction: none
---
# A contract names a language the repository has

```textproto schema=build
tool {
    generate {
        accepts: [{ language: "yaml", type_schema: "config.schema.yaml" }]
    }
}
```

The contract's types come from the language's `generate`, so the language has
to exist: a built-in one, or one `REPO.buri` declares.
