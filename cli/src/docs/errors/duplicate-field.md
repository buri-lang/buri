# A field name is used once

```text
error: `side` is already a field of `Square` [duplicate-field]
```

## What to do

rename the method, or rename the field

## Why

a `.` resolves to a field before a method, so the two may not share a name

## A program that provokes it

```buri fail code=duplicate-field use=errors
impl Square {
  fn side(self: Square): Int {
    self.side * 2
  }
}
```
