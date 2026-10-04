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

```textproto fail code=schema-mismatch repo=cli/tests/docs/repositories/textproto-contract file=lib/routes/routes.txtpb
# proto-file: other.proto
# proto-message: Route
```

A tool with a contract checks the file against the contract's `type_schema`. A
different `"$schema"`, or a text format header naming another schema or message,
contradicts it.

Two tools with different contracts reading one file is the same mistake: a file
has one schema.
