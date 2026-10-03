---
title: A binding is given its value once
message: there is no assignment; a binding is given its value once, where it is declared
fix: bind the new value to a new name with `let`, or return it from the expression that computes it
---
# A binding is given its value once

```text
error: there is no assignment; a binding is given its value once, where it is declared [reassignment]
```

```buri fail code=reassignment
fn total(): Int {
  let n = 1;
  n = 2;
  n
}
```

Build a value in steps with one `let` per step. Build it in a loop with
recursion or a fold, and use what that returns.

There's no assignment, no `mut` and no interior mutability, so a name means one
value everywhere it's in scope.
