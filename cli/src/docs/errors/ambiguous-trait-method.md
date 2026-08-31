---
title: Two bounds declaring one method name need disambiguating
message: '`{method}` is declared by both `{first_trait}` and `{second_trait}`'
---

```buri fail code=ambiguous-trait-method
trait Left {
    fn size(self): Int;
}

trait Right {
    fn size(self): Int;
}

fn go<T: Left + Right>(x: T): Int {
    x.size()
}
```
