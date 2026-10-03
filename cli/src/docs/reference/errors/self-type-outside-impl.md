---
title: `Self` names the implementing type
message: `Self` is legal only inside a `trait` or `impl`
note: `Self` stands for the implementing type, and there is none here
fix: name the type itself here
---
# `Self` names the implementing type

```text
error: `Self` is legal only inside a `trait` or `impl` [self-type-outside-impl]
```

```buri fail code=self-type-outside-impl
fn identity(x: Int): Self {
    x
}
```
