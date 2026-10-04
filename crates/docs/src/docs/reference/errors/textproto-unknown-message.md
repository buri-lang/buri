---
title: A text format file names a message its schema declares
message: '`{file}` declares no message `{message}`'
fix: name a message the schema declares, by its name or with its package as in `pkg.Config`
reproduction: none
---
# A text format file names a message its schema declares

```textproto fail code=textproto-unknown-message repo=cli/tests/docs/repositories/deploy file=lib/deploy/server.txtpb
# proto-file: server.proto
# proto-message: Sever
```

`proto-message` is read the way a field's type is: relative to the schema's
`package`, or whole with a leading `.`. A contract's `type_schema` names its
message after a `:`, as `server.proto:Server`.
