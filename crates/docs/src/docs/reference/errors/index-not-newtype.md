---
title: An `Index` type is a tuple struct of one field
message: '`{type}` cannot derive `Index`: {reason}'
note: an `Index` type's position is the one number it wraps
fix: 'wrap exactly one unsigned integer, `struct {type}(U32);`, or drop `Index` from this `derive`'
---
# An `Index` type is a tuple struct of one field

```text
error: `Slot` cannot derive `Index`: its fields are named [index-not-newtype]
```

```buri fail code=index-not-newtype
# from "core/indexset" import { Index };

derive Index for Slot;
struct Slot {
    number: U32,
}
```

The same goes for an enum, and for a tuple struct of two fields or none. Wrap
the one number:

```buri
# from "core/indexset" import { Index };

derive Index for Slot;
struct Slot(U32);
```
