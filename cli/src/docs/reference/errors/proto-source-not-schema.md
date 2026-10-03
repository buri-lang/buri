---
title: '`proto_sources` holds schemas'
message: {source} is not a .proto file
fix: list `.buri` files under `sources`; `proto_sources` holds schemas
reproduction: none
---
# `proto_sources` holds schemas

**Retired.** No build reports this any more. `proto_sources` is gone, and a
build file that still writes it gets
[`retired-proto-sources`](./retired-proto-sources.md).

A `generators` entry has no such rule: the tool decides what its files have to
be. Hand `proto` something that isn't a schema and the reader reports the line
it couldn't read.

The page stays so a code found in an old log still has an explanation.
