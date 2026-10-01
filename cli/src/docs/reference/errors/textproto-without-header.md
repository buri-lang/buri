---
title: A text format file names its message
message: this file has no {missing} header
note: a text format file a build reads is checked against its message before any generator reads it
fix: 'start the file with `# proto-file: <schema path>` and `# proto-message: <Message>`: a path relative to this file, or a `//` path'
reproduction: none
---
# A text format file names its message

```textproto ignore why="a data file, not a build file"
# proto-file: server.proto
# proto-message: Server

name: "api"
```

The two comments above the first field are the text format's
[header](https://protobuf.dev/reference/protobuf/textformat-spec/#header).
`proto-file` is the schema: a path relative to the file, or a `//` path from
the repository root. `proto-message` is the message, by its name or with its
package, as `deploy.v1.Server`.

Without them, a typo in a field name reaches the generator as data. A file read
by a tool with a [contract](../build/tools.md#input-contracts) may leave both
out, because the contract names the message.
