---
title: Every arm must be reachable
message: this arm is unreachable
note: the arms before it already cover everything it matches
fix: delete it, or move it above the arm that subsumes it
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

A guarded arm covers nothing, because its guard may fail. The usual cause is an
arm in the wrong place, and a dead arm reads as handled.
