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

## What to do

Rename the method, or rename the field.

## Why

A `.` resolves to a field before a method, so the two sharing a name would make
`sq.side` mean one thing and `sq.side()` another, decided by a rule nobody
should have to remember.

## A program that provokes it

```buri fail code=duplicate-field use=errors
impl Square {
  fn side(self): Int {
    self.side * 2
  }
}
```
