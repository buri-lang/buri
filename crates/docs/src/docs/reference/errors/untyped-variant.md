---
title: A `.Variant` needs a known expected type
message: `.{variant}` needs a known expected type
fix: write the qualified form, as in `Option.{variant}(...)`, or annotate what this value is being used as
---
# A `.Variant` needs a known expected type

```text
error: `.Some` needs a known expected type [untyped-variant]
```

```buri fail code=untyped-variant
fn mystery(): Int {
    let v = .Some(3);
    match (v) {
        .Some(n) => n,
        .None => 0,
    }
}
```

`.Variant` means "that variant of whatever type is expected here", and a `let`
with no annotation expects nothing. Inference flows into the shorthand, never
out of it, so two enums can share a variant name.
