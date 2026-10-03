---
title: A schema's import is written from the repository root
message: '"{path}" names no schema in this repository'
note: an import inside a schema is written from the repository root, the way protoc resolves one against `-I.`
fix: write the path from the repository root, as in `import "proto/address.proto";`
reproduction: none
---
