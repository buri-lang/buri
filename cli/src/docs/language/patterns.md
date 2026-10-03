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

`User { id, name }` binds `id` and `name`. Without a trailing `..`, a struct
pattern must mention every field.

A rest pattern goes last: `[first, ..rest]` is legal, `[..init, last]` is not.

Every alternative of an or-pattern binds the same names at the same types.

### 7.2 Why variants must be qualified

A bare identifier pattern is **always** a binding. `None` binds a variable named
`None`; write `.None` or `Option.None` to match the variant.

That keeps name resolution out of the parser: the token after `Foo` decides
between `Foo`, `Foo(x)` and `Foo { .. }`, never what `Foo` means
(`design/grammar-rationale.md` 12.7).

### 7.3 Exhaustiveness

Every `match` must cover its scrutinee's type. The checker understands enum
variants, `Bool`, tuples, structs, and array lengths, but not integer or string
ranges; those need a `_` arm.

An alternation counts toward coverage anywhere in a pattern: `.Some(true | false)`
covers `.Some` exactly as `.Some(true) | .Some(false)` does.

Unreachable arms are a compile error. So is an unreachable *alternative*, checked
against the arms above it and the alternatives to its left: `.Now(_) | .World`
below an arm that handles `.Now` reports the `.Now(_)` half. An arm with no
reachable alternative gets the arm-level error.

---
