---
title: A field is one its type declares
message: `{type}` has no field `{field}`
fix: check the spelling, or name a field the type declares
---
# A field is one its type declares

```text
error: `Rec` has no field `f1` [unknown-field]
```

```buri fail code=unknown-field
struct Rec {
    export f0: Int,
}

fn read(r: Rec): Int {
    r.f1
}
```

There's no structural typing and no inheritance. A value has exactly the fields
its declaration lists.
