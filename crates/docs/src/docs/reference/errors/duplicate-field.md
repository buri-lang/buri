---
title: A field name is used once
message: `{name}` is already a field of `{type}`
note: a `.` resolves to a field before a method, so the two may not share a name
fix: rename the method, or rename the field
---
# A field name is used once

```text
error: `side` is already a field of `Square` [duplicate-field]
```

```buri fail code=duplicate-field use=errors
impl Square {
    fn side(self): Int {
        self.side * 2
    }
}
```

Otherwise `x.side` and `x.side()` would differ by a lookup rule nobody should
have to remember.
