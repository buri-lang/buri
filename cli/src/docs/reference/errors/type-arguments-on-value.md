---
title: Type arguments qualify a function, not a value
message: explicit type arguments qualify a function or a call
fix: attach the type arguments to the call, as in `{function}<Str>(x)`
---
# Type arguments qualify a function, not a value

```text
error: explicit type arguments qualify a function or a call [type-arguments-on-value]
```

```buri fail code=type-arguments-on-value
fn f(a: Int, c: Int): Bool {
    a<Int>(c)
}
```

If you meant a comparison, comparisons don't chain: write `a < b && b > c`.

`a < Int > (c)` parses as type arguments on `a`, because `x < y > z` means
nothing as a comparison. That's what lets Buri write `f<T>(x)` without `::`.
Type arguments pick an instantiation of a generic function, so what sits to
their left must be a function.
