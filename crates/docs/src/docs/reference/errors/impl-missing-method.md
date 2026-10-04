---
title: An `impl` supplies every method of its trait
message: `{type}`'s `impl {trait}` is missing {methods}
note: an `impl` supplies every method its trait declares
fix: add {methods} to the block, with the signature `{trait}` declares
---
# An `impl` supplies every method of its trait

```text
error: `Bag`'s `impl Measurable` is missing `isEmptyThing` [impl-missing-method]
```

```buri fail code=impl-missing-method
trait Measurable {
    fn size(self): Int;
    fn isEmptyThing(self): Bool;
}

struct Bag {
    export count: Int,
}

impl Measurable for Bag {
    fn size(self): Int {
        self.count
    }
}
```

Traits have no default method bodies.
