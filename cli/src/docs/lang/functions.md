## 9. Functions

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect/lib.buri" import { Clock };
export fn slugify(s: Str): Str { ... }

fn quadratic(a: F64, b: F64, c: F64): Option<(F64, F64)> { ... }

fn retry<T, C: Clock>(
  ctx: C,
  attempts: Int,
  action: fn(C) => Result<T, Str>,
): Result<T, Str> { ... }
```

- The return type annotation is **required** on every top-level `fn`. Local
  bindings and lambdas are inferred.
- Parameter types are required.
- Trailing commas are allowed in parameter and argument lists.
- Functions are first-class values and may be passed, returned, and stored.
- There is no overloading and no default arguments.

Type inference is Hindley–Milner. There is no row polymorphism: it went away
with the structural records of Section 5.5, and effects are trait bounds rather
than rows. Because top-level signatures are mandatory, inference is local to a function body, and
type errors are reported against the signature you wrote rather than one the
compiler guessed.

---
