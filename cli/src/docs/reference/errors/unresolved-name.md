---
title: Every name resolves to a declaration
message: there is nothing named `{name}` in scope
---
# Every name resolves to a declaration

```text
error: there is nothing named `duoble` in scope [unresolved-name]
```

```buri fail code=unresolved-name
fn twice(n: Int): Int {
    duoble(n)
}
```

```buri fail code=unresolved-name
fn root(x: Float): Float {
    sqrt(x)
}
```

There is no prelude. A module sees only what it declares and what its own
imports name, which is why the suggestion is trustworthy.
