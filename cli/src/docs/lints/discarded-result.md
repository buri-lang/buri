---
title: Every deliberately dropped `Result` is reported
severity: warning
message: this discards a `Result`
note: "`ignore` is the one way to drop a `Result`, so every place a failure is deliberately unhandled is one of these"
fix: handle the error with `match`, propagate it with `?`, or keep `ignore` if dropping it is deliberate
---

Discarding a results hides a failure you have not understood or have not planned for. Instead, make sure you explicitly handle it.

```
# bad
let _ = io.println(ctx, "Hello world");

# better
let result = io.println(ctx, "Hello world");
match (result) {
  .Ok(_) => { ... },
  .Err(_) => { ... },
}
```
