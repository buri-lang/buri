---
title: A contract lists each language once
message: '`{language}` has two contracts on this tool'
note: 'the types for a language are one module, `<tool>/{language}`, so the tool has one schema for it'
fix: 'keep one `accepts` entry for `{language}`, or give `check` and `generate` the same `type_schema`'
reproduction: none
---
# A contract lists each language once

```textproto schema=build
tool {
    generate {
        accepts: [
            { language: "json", type_schema: "config.schema.json" },
            { language: "json", type_schema: "other.schema.json" },
        ]
    }
}
```

The build generates a language's types into the tool as `<tool>/<language>`,
so a tool has one schema per language, across `check` and `generate` too.
