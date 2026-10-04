---
title: A JSON file follows its schema
message: '{problem}'
note: 'the schema says so at `{location}`'
fix: '{remedy}'
reproduction: none
---
# A JSON file follows its schema

```text
error: expected an integer, found a string [json-schema-violation]
 --> lib/deploy/regions.json:3:13
```

Change the value, or, if the value is right, change the schema. The generator
that lists the file doesn't run until it passes.

`"$schema"` is a property like any other: a schema with
`"additionalProperties": false` lists it in `properties`.
