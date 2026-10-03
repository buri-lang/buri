---
title: A list separates its elements with `,`
message: {construct} ends with `,`
fix: write `,` here
---
# A list separates its elements with `,`

```text
error: a match arm ends with `,` [missing-comma]
```

```buri fail code=missing-comma
fn pick(n: Int): Int {
  match (n) {
    1 => 1
    _ => 0,
  }
}
```

An editor's quick fix and `buri lint --fix` both write the `,` for you.

Every comma-separated list works the same way, even when an element ends in a
brace: a match arm whose body is a block still ends with `,`. Without it,
`A => x` followed by `-1 => y` reads as `x - 1` (design/grammar-rationale.md
12.12).
