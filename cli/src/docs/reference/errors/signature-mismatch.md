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

The code generator also rebuilds the `impl` method's type arguments from the
trait's. An `impl` with an extra parameter or a different type would break at
some later call site, if at all.
