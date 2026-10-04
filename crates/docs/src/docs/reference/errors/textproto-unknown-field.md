---
title: A field is one its message declares
message: '`{message}` has no field `{field}`'
fix: write one of the fields the message declares
reproduction: none
---
# A field is one its message declares

```textproto ignore why="a data file, not a build file"
# proto-file: server.proto
# proto-message: Server
name: "api"
prot: 80
# `Server` has no field `prot`
```

A field is written by the name the schema gives it, not its JSON name.
