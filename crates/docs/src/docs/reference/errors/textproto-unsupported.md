---
title: The text format reader refuses what it cannot read
message: '{construct} is not supported'
note: '{reason}'
fix: '{remedy}'
reproduction: none
---
# The text format reader refuses what it cannot read

```textproto fail code=textproto-unsupported repo=cli/tests/docs/repositories/deploy file=lib/deploy/server.txtpb
# proto-file: server.proto
# proto-message: Server
# proto-import: other.proto
[pkg.ext]: 1
[type.googleapis.com/pkg.Point] { x: 1 }
```

- **`# proto-import:`.** The specification gives it no meaning. `proto-file`
  names the schema, and the schema's own `import`s bring in what it uses.
- **`[name]` fields.** An extension or an expanded `Any`. The schemas this
  toolchain reads declare no extensions, and an `Any` names a type its schema
  does not.
