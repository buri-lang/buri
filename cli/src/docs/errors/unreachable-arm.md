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

## What to do

Delete it, or move it above the arm that subsumes it.

## Why

Arms are tried in order, so an arm the ones above it already cover can never
run. Reported rather than ignored because the usual cause is an arm in the
wrong place, and a silently dead arm reads as handled.

## A program that provokes it

```buri fail code=unreachable-arm
fn describe(o: Option<Int>): Int {
    match (o) {
        anything => 1,
        .None => 0,
    }
}
```
