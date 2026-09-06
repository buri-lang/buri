## 7. Patterns

### 7.1 Forms

| Form | Example |
|---|---|
| Wildcard | `_` |
| Binding | `n` |
| Named subpattern | `whole @ .Circle(r)` |
| Literal | `0`, `-1`, `"yes"`, `'x'`, `true` |
| Qualified variant | `Option.Some(x)`, `Shape.Empty` |
| Inferred variant | `.Some(x)`, `.Empty` |
| Struct | `User { id, name: n }`, `User { id, .. }` |
| Tuple struct | `Meters(m)` |
| Tuple | `(a, b)` |
| Array | `[]`, `[x]`, `[first, ..rest]` |
| Or | `.Circle(_) | .Empty` |

Struct patterns support field shorthand: `User { id, name }` binds `id` and
`name`. A `..` at the end ignores remaining fields; without it, a struct pattern
must mention every field.

Array rest patterns bind only at the end: `[first, ..rest]` is legal,
`[..init, last]` is not.

Or-patterns must bind the same names at the same types in every alternative.

### 7.2 Why variants must be qualified

A bare identifier pattern is **always** a binding. `None` as a pattern binds a
variable named `None`; it does not match the `None` variant. Write `.None` or
`Option.None`.

This is a real ergonomic cost, and it is what takes name resolution out of the
parser. The token after `Foo` decides between `Foo`, `Foo(x)` and `Foo { .. }`,
never what `Foo` means. `design/grammar-rationale.md` 12.7.

### 7.3 Exhaustiveness

Every `match` must cover its scrutinee's type. The checker reasons about enum
variants, `Bool`, tuples, structs, and array lengths. It does not try
exhaustiveness over integer or string ranges; those need a `_` arm.

An alternation counts toward coverage wherever it appears, not only at the top
of a pattern: `.Some(true | false)` covers `.Some` completely, exactly as
`.Some(true) | .Some(false)` does.

Unreachable arms are a compile error, not a warning. So is an unreachable
*alternative*. The checker asks about each `|` alternative on its own, against
the arms above it and the alternatives to its left. So `.Now(_) | .World` below
an arm that already handles `.Now` reports the `.Now(_)` half rather than passing
because its other half is live. An arm with no live alternative at all gets the
arm-level error.

---
