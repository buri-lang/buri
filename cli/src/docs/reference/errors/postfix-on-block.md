---
title: A block expression is parenthesised before `.`, `(` or `[`
message: {construct} is an operand, not the head of a `{token}`
fix: parenthesise it, or bind it with `let` and go on from the name
---
# A block expression is parenthesised before `.`, `(` or `[`

```text
error: a `match` is an operand, not the head of a `.` [postfix-on-block]
```

```buri fail code=postfix-on-block
struct Point { export x: Int, export y: Int }

fn xOf(p: Point): Int {
  match (p) {
    Point { x, y: _ } => Point { x, y: 0 },
  }.x
}
```

Binding it with `let` usually reads better than parentheses.

`if`, `match`, `context` and a bare block end with `}`. If `.`, `(` or `[` could
follow that `}`, then `if (c) { a } else { b } { x: 1 }` would be ambiguous: a
struct literal headed by the `if`, or an `if` followed by a block
(design/grammar-rationale.md 12.13).
