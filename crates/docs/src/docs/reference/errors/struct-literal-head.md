---
title: A struct literal starts with a type or variant
message: the head of a struct literal must be a struct or an enum variant
fix: name the type, as in `Point {{ x: 1, y: 2 }}`, or a variant that has fields, as in `.Variant {{ ... }}` where the expected type is known
---
# A struct literal starts with a type or variant

```text
error: the head of a struct literal must be a struct or an enum variant [struct-literal-head]
```

```buri fail code=struct-literal-head
struct Holder {
    export a: Int,
}

fn identity(n: Int): Int {
    n
}

fn build(): Int {
    let h = identity(1) { a: 1 };
    h.a
}
```

The grammar decides shape without name resolution, so it accepts any head, such
as `f(x) { a: 1 }`. The checker rejects it, one phase later than you might
expect.
