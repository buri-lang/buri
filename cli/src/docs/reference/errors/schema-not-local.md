---
title: A schema is checked into the repository
message: '`{schema}` is not a file in this repository'
note: a schema is read from the repository and never fetched, so every machine checks against the same one
fix: check the schema in, and name it by a path relative to this file or a `//` path
reproduction: none
---
# A schema is checked into the repository

```text
error: `https://example.com/regions.schema.json` is not a file in this repository [schema-not-local]
 --> lib/deploy/regions.json:2:16
```

`"$schema"`, and every `$ref` to another file, names a file in this repository:

```json
{ "$schema": "regions.schema.json" }
{ "$schema": "//schemas/regions.schema.json" }
```

A URL, an absolute path, or a relative path that climbs out of the repository
is refused. Fetching one would put the network in the build, and two machines
could check one file against two different schemas.

The one URL known by name is the JSON Schema 2020-12 meta-schema,
`https://json-schema.org/draft/2020-12/schema`, which marks a file as a schema.
