---
title: An enum has a variant
message: `{name}` has no variants
note: an enum with no variants has no values, so nothing could ever build one
fix: give `{name}` a variant, or use `()` where a type needs no information
---
# An enum has a variant

```text
error: `Never` has no variants [empty-enum]
```

```buri fail code=empty-enum
enum Never {}
```

Buri has no type without values. Where a `Result` can't fail, return the value
itself rather than a `Result` whose error type has no values.
