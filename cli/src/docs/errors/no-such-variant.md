---
title: A variant is named by the enum that declares it
message: '`{type}` has no variant `{variant}`'
fix: name a variant the enum declares
---

```buri fail code=no-such-variant
enum Colour { Red, Green }

fn go(): Colour { .Blue }
```
