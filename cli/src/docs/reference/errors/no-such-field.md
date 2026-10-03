---
title: A field is named by the type that declares it
message: `{type}` has no field `{field}`
fix: check the spelling, or name a field the type declares
---
# A field is named by the type that declares it

```text
error: `Rec` has no field `f1` [no-such-field]
```

```buri fail code=no-such-field
struct Rec {
    export f0: Int,
}

fn read(r: Rec): Int {
    r.f1
}
```

There's no structural typing and no inheritance. A value has exactly the fields
its declaration lists.
