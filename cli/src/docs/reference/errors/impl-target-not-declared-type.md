---
title: Only a declared type has methods
message: "`{type}` has no methods"
note: tuples, function types, and `Template` have no defining module
fix: write a free function instead, and call it as one
---
```buri fail code=impl-target-not-declared-type
impl (Int, Int) {
    fn total(self): Int {
        1
    }
}
```
