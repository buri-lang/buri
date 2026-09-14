---
title: An `impl` supplies the signature its trait declares
message: "`{method}` does not have the signature `{trait}` declares"
label: expected {expected}, found {found}
note: a call through a bound is checked against the trait's declaration and dispatched to the `impl`'s method, so the two are one signature
fix: give `{method}` the signature `{trait}` declares
---
# An `impl` supplies the signature its trait declares

```text
error: `size` does not have the signature `Measurable` declares [signature-mismatch]
```

## Why

A caller reaching the method through a bound is typechecked against the
*trait's* declaration, and the code generator reconstructs the `impl` function's
type arguments from the trait's. So an `impl` that took one more parameter, or a
`Str` where the trait said `Int`, would break its promise at some later call
site, if at all.

## A program that provokes it

```buri fail code=signature-mismatch
trait Measurable {
    fn size(self): Int;
}

struct Bag {
    export count: Int,
}

impl Measurable for Bag {
    fn size(self, scale: Int): Int {
        self.count * scale
    }
}
```
