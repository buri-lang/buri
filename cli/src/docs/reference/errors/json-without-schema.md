---
title: A JSON file names its schema
message: this file has no `"$schema"`
note: a JSON file a build reads is checked against its schema before any generator reads it
fix: 'add `"$schema": "<schema path>"` as its first key: a path relative to this file, or a `//` path'
reproduction: none
---
# A JSON file names its schema

```text
error: this file has no `"$schema"` [json-without-schema]
 --> lib/deploy/regions.json:1:1
```

```json
{
    "$schema": "regions.schema.json",
    "regions": ["eu-west", "us-east"]
}
```

Without a schema, a typo in a key reaches the generator as data and shows up
as wrong generated code instead of an error at the typo. `buri format` never
adds the key: which schema a file follows is yours to say.
