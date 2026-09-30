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

Each failure is reported at the value that fails, and the note names the
keyword in the schema that it fails. Change the value, or, if the value is
right, change the schema.

A file with a failure is not handed to the generator that lists it.

`"$schema"` is a property like any other: a schema with
`"additionalProperties": false` lists it in `properties`.
