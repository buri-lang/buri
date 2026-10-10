---
title: Every field of a `Flags` type is a `Bool`
message: '`{type}` cannot derive `Flags`: `{field}` has type `{field_type}`'
note: a `Flags` type stores one bit per field, so every field is a `Bool`
fix: make `{field}` a `Bool`, or drop `Flags` from this `derive`
---
# Every field of a `Flags` type is a `Bool`

```text
error: `Access` cannot derive `Flags`: `level` has type `I64` [flags-field-not-bool]
```

```buri fail code=flags-field-not-bool
# from "core/flags" import { Flags };

derive Flags for Access;
struct Access {
    read: Bool,
    level: Int,
}
```

Keep the flags in a struct of their own and hold it beside the other fields:

```buri
# from "core/flags" import { Flags };

derive Flags for Access;
struct Access {
    read: Bool,
    write: Bool,
}

struct Grant {
    access: Access,
    level: Int,
}
```
