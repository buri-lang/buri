---
title: An `accepts` entry names a language and a schema
message: 'this `accepts` entry has no `{field}`'
fix: 'write both `language` and `type_schema`'
reproduction: none
---
# An `accepts` entry names a language and a schema

```textproto schema=build
tool {
    generate {
        accepts: [{ language: "json", type_schema: "config.schema.json" }]
    }
}
```

`language` says which inputs the contract covers, and `type_schema` is what
that language's `generate` turns into the types the tool reads.
