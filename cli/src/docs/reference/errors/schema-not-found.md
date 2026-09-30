---
title: A schema path names a file
message: there is no schema at `{path}`
fix: create the schema, or correct the path
reproduction: none
---
# A schema path names a file

```text
error: there is no schema at `lib/deploy/regions.schema.json` [schema-not-found]
 --> lib/deploy/regions.json:2:16
```

A relative path is read from the directory of the file that holds it, the way
a browser resolves a link. A path starting `//` is read from the repository
root.
