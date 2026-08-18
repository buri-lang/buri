## What's in v0.2

Primitives with explicit widths, arrays, tuples, structs (with per-field
visibility), enums, functions, methods, traits, and `effect` declarations.
Generics with trait bounds. The type system is nominal throughout — no records,
no structural conformance. Pattern matching with exhaustiveness checking. `Option`,
`Result`, `?`, `??`.

Methods and traits, neither of which introduces a runtime mechanism.

**Not present, deliberately:** classes, inheritance, dynamic dispatch, trait
objects, records, row polymorphism, cast operators, mutation, `null`, exceptions,
loops, the `|>` pipe operator, `return`, overloading, macros.

A `for`/`while` sugar was specified for this version and cut;
[SPEC.md §15.1](./cli/src/docs/SPEC.md) records the reasoning, since it constrains any future
attempt. Iteration is `fold` or explicit recursion, with tail calls guaranteed
eliminated.

**Deferred:** blanket impls, associated types, `where` clauses, supertraits,
foreign impls, dictionary literals, ranges, fixed-length array types, `async`.
