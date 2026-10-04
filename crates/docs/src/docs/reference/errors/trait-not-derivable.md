---
title: Only some traits can be derived
message: '`{trait}` cannot be derived'
fix: write `impl {trait} for ... {{ ... }}` by hand
---

```buri fail code=trait-not-derivable
struct Point { export x: Int }

trait Measurable { fn size(self): Int }

derive Measurable for Point;
```
