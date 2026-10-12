# Prices

A latte, priced by `sample`'s money library.

```buri file=lib/cafe/lib.buri repo=sample
from "//lib/money" import { latte };

export fn price(): I64 {
    latte()
}
```

```textproto file=lib/cafe/BUILD.buri repo=sample
library {
    dependencies: ["//lib/money"]
}
```
