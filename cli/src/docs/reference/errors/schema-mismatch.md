---
title: A file under a contract has the contract's schema
message: '`"$schema": "{schema}"` is not `{contract}`, the schema the tool reading this file checks it against'
note: 'under a contract a file may leave out `"$schema"`, or name the same schema'
fix: 'delete `"$schema"`, or name `{contract}`'
reproduction: none
---
# A file under a contract has the contract's schema

```json
{ "$schema": "orders.schema.json", "tables": [] }
```

A tool with a contract reads a typed value, so the file is checked against the
contract's `type_schema`. A different `"$schema"` would say the file is
something else.

Two tools with different contracts reading one file is the same mistake: one
file cannot have two schemas.
