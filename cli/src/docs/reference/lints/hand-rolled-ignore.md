---
title: A `match` that drops a `Result` is `ignore` written out
severity: warning
message: this drops a `Result` by hand
note: "every arm answers `()`, so nothing here handles anything — this is `core/result.ignore` written out in four lines"
fix: handle the error with `match`, propagate it with `?`, or say `ignore()` if dropping it is deliberate
---
```
match (io.println(ctx, line)) {
    .Ok(_written) => (),
    .Err(_error) => (),
}
```

Both arms answer `()`, so this handles nothing. It's `core/result.ignore`,
whose body is these same four lines.

**The real fix is to handle the error.** Do something in the `.Err` arm (count
it, report it, fall back), or use `?` to hand it to a caller who can.

If the drop is deliberate, like a cache write whose failure no caller could act
on, say `ignore()`. It's one greppable call, and `ignored-result` collects
every one into a single report.

This rule exists so the four-line form can't dodge `ignored-result` in a
repository gated on a clean lint run. Both forms are reported.

It fires only on exactly this shape: two arms, `.Ok` and `.Err`, no guards,
nothing read from either payload, and both bodies `()`. A `match` that does
anything in either arm is not a finding.
