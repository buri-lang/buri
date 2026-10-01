---
title: The text format reader refuses what it cannot read
message: '{construct} is not supported'
note: '{reason}'
fix: '{remedy}'
reproduction: none
---
# The text format reader refuses what it cannot read

```textproto ignore why="a data file, not a build file"
# proto-import: other.proto
[pkg.ext]: 1
[type.googleapis.com/pkg.Point] { x: 1 }
```

- **`# proto-import:`.** The specification gives it no meaning. `proto-file`
  names the schema, and the schema's own `import`s bring in what it uses.
- **`[name]` fields.** An extension or an expanded `Any`. The schemas this
  toolchain reads declare no extensions, and an `Any` names a type its schema
  does not.
