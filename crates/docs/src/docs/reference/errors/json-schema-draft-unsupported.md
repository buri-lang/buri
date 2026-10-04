---
title: A schema is JSON Schema 2020-12
message: '`{draft}` is not JSON Schema 2020-12'
note: this toolchain reads one draft of JSON Schema, so every keyword has one meaning
fix: 'write `"$schema": "https://json-schema.org/draft/2020-12/schema"` in the schema, and update its keywords to 2020-12'
reproduction: none
---
# A schema is JSON Schema 2020-12

```text
error: `http://json-schema.org/draft-07/schema#` is not JSON Schema 2020-12 [json-schema-draft-unsupported]
 --> lib/deploy/regions.schema.json:2:16
```

Drafts disagree on keywords: `items` holding a list is a tuple in draft 7 and
an error in 2020-12. Moving a draft-07 schema is mostly renaming:

| Draft 7                   | 2020-12                                    |
|---|---|
| `definitions`             | `$defs`                                    |
| `items: [...]`            | `prefixItems: [...]`                       |
| `additionalItems`         | `items`                                    |
| `dependencies`            | `dependentRequired` or `dependentSchemas`  |
