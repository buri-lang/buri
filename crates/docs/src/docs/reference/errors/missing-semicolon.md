---
title: A declaration ends with `;`
message: {construct} ends with `;`
fix: write `;` here
---
# A declaration ends with `;`

```text
error: a type alias ends with `;` [missing-semicolon]
```

```buri fail code=missing-semicolon
type Meters = Float

fn zero(): Meters {
  0.0
}
```

An editor's quick fix and `buri lint --fix` both write the `;` for you.

A declaration ends with `;` unless its last token is a `}`. So `let`, `type`,
`derive`, an import and a tuple struct end with `;`, while a `fn`, a `struct`
with fields, an `enum`, a `trait` and an `impl` end with `}`.
