---
title: A struct literal is headed by a type
message: the head of a struct literal must be a type
note: the grammar permits `f(x) {{ a: 1 }}`; the checker does not
fix: name the type, as in `Point {{ x: 1, y: 2 }}`, or `.Variant {{ ... }}` where the expected type is known
---
# A struct literal is headed by a type

```text
error: the head of a struct literal must be a type [struct-literal-head]
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

The grammar decides shape without name resolution, so it accepts any head. The
checker rejects it, one phase later than you might expect.
