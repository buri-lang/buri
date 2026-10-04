---
title: Every type name resolves to a declaration
message: there is no type `{name}`
fix: declare it, import it, or correct the spelling
---
# Every type name resolves to a declaration

```text
error: there is no type `Widgett` [unknown-type]
```

```buri fail code=unknown-type wrap=body
let n: Widgett = 1;
```

```buri fail code=unknown-type
fn describe(n: Int): Int {
    match (n) {
        Shape.Circle => 1,
        _ => 0,
    }
}
```

Types are nominal, with no structural fallback, so a misspelling can't quietly
become a different type that happens to fit.
