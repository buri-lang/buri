---
title: A `Flags` type has a field
message: '`{type}` cannot derive `Flags`: it has no fields'
note: a `Flags` type stores one bit per field, and a set of no flags holds nothing
fix: add a `Bool` field, or drop `Flags` from this `derive`
---
# A `Flags` type has a field

```text
error: `Nothing` cannot derive `Flags`: it has no fields [flags-empty]
```

```buri fail code=flags-empty
# from "core/flags" import { Flags };

derive Flags for Nothing;
struct Nothing {}
```
