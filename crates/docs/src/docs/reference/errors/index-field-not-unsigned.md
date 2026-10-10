---
title: An `Index` type wraps an unsigned integer
message: '`{type}` cannot derive `Index`: it wraps `{field_type}`'
note: a position is never negative, so an `Index` type wraps a `U8`, `U16`, `U32` or `U64`
fix: wrap an unsigned integer, or drop `Index` from this `derive`
---
# An `Index` type wraps an unsigned integer

```text
error: `Row` cannot derive `Index`: it wraps `I64` [index-field-not-unsigned]
```

```buri fail code=index-field-not-unsigned
# from "core/indexset" import { Index };

derive Index for Row;
struct Row(Int);
```

Pick the unsigned width that holds every row:

```buri
# from "core/indexset" import { Index };

derive Index for Row;
struct Row(U32);
```
