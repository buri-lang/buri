---
title: A method called on a value takes `self`
message: '`{name}` takes no `self`, so it is not called on a value'
fix: give `{name}` a `self` parameter, or call it without a receiver
---

```buri fail code=method-without-self
trait Make {
    fn make(): Int;
}

fn go<T: Make>(t: T): Int {
    t.make()
}
```

`x.f()` hands `x` to `f` as `self`, so `f` needs a `self` parameter to take
it. A trait function with no `self`, such as `Bounded`'s `maxValue`, is
called without a receiver: `number.maxValue<U8>()`.
