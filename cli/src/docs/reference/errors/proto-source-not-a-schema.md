---
title: '`proto_sources` holds schemas'
message: {source} is not a .proto file
fix: list `.buri` files under `sources`; `proto_sources` holds schemas
reproduction: none
---
# `proto_sources` holds schemas

**Retired.** No build says this any more. `proto_sources` is gone, and a build
file that still writes it is
[`retired-proto-sources`](./retired-proto-sources.md).

The rule this page carried was that the field held schemas and nothing else. A
`generators` entry has no such rule: a generator reads whatever it likes, and
what a file has to be is the tool's question rather than the build's. Hand
`std/codegen/proto` something that is not a schema and the reader says so, at
the line it could not read.

The page is kept because a code that has ever been printed is a code somebody
can still find in a log.
