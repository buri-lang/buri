## What's in v0.3

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
[SPEC.md §15.1](../SPEC.md) records the reasoning, since it constrains any future
attempt. Iteration is `fold` or explicit recursion, with tail calls guaranteed
eliminated.

**Deferred:** blanket impls, associated types, `where` clauses, supertraits,
foreign impls, dictionary literals, ranges, fixed-length array types, `async`.

### Where the toolchain is

It builds, tests, formats, lints, serves a language server, and answers
questions about a monorepo. It emits JavaScript and native code: `buri test`
compiles a suite to a native binary by default and falls back to JavaScript
where the native surface does not reach yet, while a binary that declares no
output still gets JavaScript.

The sharpest unresolved trade-off is the rule that a lambda may not capture an
effect ([SPEC.md §10.5](../SPEC.md)) — it is what makes the purity theorem hold
structurally, and it still forces any function that *stores* an effectful
callback to put the context in that callback's type. The runner-up is the
absence of `break`. [SPEC.md §15](../SPEC.md) lists those and five other
questions that want real programs before they can be settled.
