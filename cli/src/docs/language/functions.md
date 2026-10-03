## 9. Functions

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "platform/effect" import { Clock };
export fn slugify(s: Str): Str { ... }

fn quadratic(a: F64, b: F64, c: F64): Option<(F64, F64)> { ... }

fn retry<T, C: Clock>(
  ctx: C,
  attempts: Int,
  action: fn(C) => Result<T, Str>,
): Result<T, Str> { ... }
```

- Every top-level `fn` declares its parameter types and return type. Local
  bindings and lambdas are inferred.
- Trailing commas are allowed in parameter and argument lists.
- Functions are first-class values.
- There is no overloading and no default arguments.

Type inference is Hindley–Milner without row polymorphism: effects are trait
bounds, not rows. Because top-level signatures are mandatory, inference stays
inside one function body and type errors are reported against the signature you
wrote.

---
