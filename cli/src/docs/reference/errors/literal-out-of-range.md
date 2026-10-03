---
title: A literal must fit the type it is pinned to
message: {literal} is not representable in `{type}`
---
# A literal must fit the type it is pinned to

```text
error: 18_446_744_073_709_551_616 is not representable in `U64` [literal-out-of-range]
```

```buri fail code=literal-out-of-range wrap=body
let w: U64 = 18_446_744_073_709_551_616;
```

Write a value inside the type's range, or annotate a wider type. A literal
never widens to fit.
