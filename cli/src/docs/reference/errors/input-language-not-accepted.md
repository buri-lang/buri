---
title: A tool with a contract reads the languages it lists
message: '`{input}` is in {language}, and `{tool}` accepts {accepted}'
note: 'a tool with a contract reads typed values, and only the languages its `accepts` lists have types'
fix: 'list a file in an accepted language, or add an `accepts` entry for this one to the tool'
reproduction: none
---
# A tool with a contract reads the languages it lists

```textproto schema=build
library {
    generators: [
        { tool: "//tools/database_schema_codegen", inputs: ["schema.json", "notes.jsonc"] },
    ]
}
```

`//tools/database_schema_codegen` accepts `json` only, so `notes.jsonc` has no
type to be read as.
