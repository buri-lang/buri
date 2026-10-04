---
title: A match arm is `pattern => expression`
message: a match arm is `pattern => expression`
fix: write `=>` here
---
# A match arm is `pattern => expression`

```text
error: a match arm is `pattern => expression` [missing-arrow]
```

```buri fail code=missing-arrow
fn pick(n: Int): Int {
  match (n) {
    1 1,
    _ => 0,
  }
}
```

An editor's quick fix writes the `=>` for you.

Patterns and expressions can look identical: `1` on the left matches one, `1`
on the right is one. The arrow is how the parser knows where the pattern stops.
