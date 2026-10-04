---
title: A literal fits its type
message: {literal} is not representable in `{type}`
---
# A literal fits its type

```text
error: 18_446_744_073_709_551_616 is not representable in `U64` [literal-out-of-range]
```

```buri fail code=literal-out-of-range wrap=body
let w: U64 = 18_446_744_073_709_551_616;
```

Write a value inside the type's range, or annotate a wider type. A literal
never widens to fit.
