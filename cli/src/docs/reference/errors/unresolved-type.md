---
title: Every type name resolves to a declaration
message: there is no type `{name}`
fix: declare it, import it, or correct the spelling
---
# Every type name resolves to a declaration

```text
error: there is no type `Widgett` [unresolved-type]
```

```buri fail code=unresolved-type wrap=body
let n: Widgett = 1;
```

Types are nominal, with no structural fallback, so a misspelling can't quietly
become a different type that happens to fit.
