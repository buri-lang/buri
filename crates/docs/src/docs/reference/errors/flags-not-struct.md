---
title: Only a struct derives `Flags`
message: '`{type}` cannot derive `Flags`: it is an enum'
note: a `Flags` type stores one bit per field, and an enum's value is one variant
fix: write a struct with a `Bool` field per flag, or drop `Flags` from this `derive`
---
# Only a struct derives `Flags`

```text
error: `Permission` cannot derive `Flags`: it is an enum [flags-not-struct]
```

```buri fail code=flags-not-struct
# from "core/flags" import { Flags };

derive Flags for Permission;
enum Permission {
    Read,
    Write,
}
```

A set of an enum's cases is a struct with a `Bool` per case:

```buri
# from "core/flags" import { Flags };

derive Flags for Permissions;
struct Permissions {
    read: Bool,
    write: Bool,
}
```
