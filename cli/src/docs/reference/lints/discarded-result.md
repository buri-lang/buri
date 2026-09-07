---
title: Every deliberately dropped `Result` is reported
severity: warning
message: this discards a `Result`
note: "`ignore` is the one way to drop a `Result`, so every place a failure is deliberately unhandled is one of these"
fix: handle the error with `match`, propagate it with `?`, or keep `ignore` if dropping it is deliberate
---
A dropped `Result` is a failure nobody has read. Sometimes that is the right
call, and this rule does not say otherwise. It says every place you made that
call belongs in one report.

**The real solution is to handle the error.** `match` on the `Result` and answer
the `.Err` arm — count it, report it, fall back — or `?` to hand it to a caller
who can.

**Writing the drop out by hand is the anti-pattern, not the way around this
rule.** The four-line form

```
match (io.println(ctx, line)) {
    .Ok(_written) => (),
    .Err(_error) => (),
}
```

handles nothing. It is `ignore()` spelled out, and it throws away the one
advantage `ignore()` had, that a reviewer can grep for it.
`discarded-result-by-hand` reports it too, so it is no route to a quiet report.

This rule cannot be about `let _ = someResult()`. That is already a hard type
error, `result-discarded`, and so is leaving the `Result` standing as a
statement. What is left is `ignore`, the deliberate, greppable drop.

**A dropped print is no exception**, and that is the whole of what a total
must-use costs. `Stdout` and `Stderr` answer `Result<(), IoError>` because a
closed pipe is a thing that happens, so `io.println(ctx, x).ignore()` is a
deliberate drop like any other. The program that cares says so: `match` on the
print and answer, which is what `buri init`'s template and the example monorepo
do.

If the failure genuinely does not matter, leaving the `ignore` where it is and
letting this warning stand is a legitimate outcome. The point of the rule is
that somebody decided, not that the count reaches zero.
