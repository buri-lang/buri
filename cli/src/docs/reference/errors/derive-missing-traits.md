---
title: A `derive` clause names at least one trait
message: a `derive` clause names no traits
note: `derive` generates one implementation per trait it names, so a clause naming none would generate nothing; delete it, or name what the type should derive
fix: name the traits between `derive` and `for`, as in `derive Equal, Show for Meters;`
---
# A `derive` clause names at least one trait

```text
error: a `derive` clause names no traits [derive-missing-traits]
```

```buri fail code=derive-missing-traits
struct Meters(export Float);

derive for Meters;

from "native" import { NativeHost };

export fn main(host: NativeHost): Result<(), Str> {
  .Ok(())
}
```
