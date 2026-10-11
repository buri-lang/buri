---
title: Every arm must be reachable
message: this arm is unreachable
note: {why}
fix: {fix}
---
# Every arm must be reachable

```text
error: this arm is unreachable [unreachable-arm]
```

```buri fail code=unreachable-arm
fn describe(o: Option<Int>): Int {
    match (o) {
        anything => 1,
        .None => 0,
    }
}
```

```buri fail code=unreachable-arm
enum Never {}

fn describe(o: Option<Never>): Int {
    match (o) {
        .None => 0,
        .Some(_) => 1,
    }
}
```

An arm is unreachable when the arms before it cover it, or when no value fits
it at all, like `.Some(_)` on an `Option<Never>`. A guarded arm covers nothing,
because its guard may fail. The usual cause is an arm in the wrong place, and a
dead arm reads as handled.
