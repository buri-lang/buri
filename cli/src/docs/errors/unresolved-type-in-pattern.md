---
title: A pattern's path names a type or a variant
message: there is no type `{name}`
note: a bare identifier in a pattern is always a binding; a variant is written `.Variant` or `Enum.Variant`
fix: write `.Variant` for a variant, or a lowerCamelCase name to bind the value
---
```buri fail code=unresolved-type-in-pattern
fn describe(n: Int): Int {
  match (n) {
    Shape.Circle => 1,
    _ => 0,
  }
}
```
