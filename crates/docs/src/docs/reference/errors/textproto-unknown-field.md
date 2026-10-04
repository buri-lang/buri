---
title: A field is one its message declares
message: '`{message}` has no field `{field}`'
fix: write one of the fields the message declares
reproduction: none
---
# A field is one its message declares

```textproto fail code=textproto-unknown-field repo=cli/tests/docs/repositories/deploy file=lib/deploy/server.txtpb
# proto-file: server.proto
# proto-message: Server
name: "api"
prot: 80
# `Server` has no field `prot`
```

A field is written by the name the schema gives it, not its JSON name.
