---
title: A trait is a bound, not a type
message: '`{name}` is a trait, not a type'
note: there are no trait objects; use a bound on a type parameter
fix: use it as a bound — `<T: {name}>` — and name `T` here
---

```buri fail code=trait-used-as-a-type
trait Measurable { fn size(self): Int }

fn take(m: Measurable): Int { 0 }
```
