---
title: A constant never refers back to itself
message: 'circular constant: {cycle}'
note: a constant is substituted wherever it's used, so one that reaches itself never finishes expanding
fix: 'break the cycle: write one of these as a value that names none of the others'
---

```buri fail code=circular-constant
let width: Int = height + 1;

let height: Int = width + 1;
```

A module-level `let` has no address. Each use is its initializer, written out
again, so a chain that comes back to where it started has no end. The
diagnostic prints the whole chain, since changing any name on it fixes it.
