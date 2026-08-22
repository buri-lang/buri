## 13. Compilation invariants

Section 12 explains why parsing is cheap. This section states the invariants that
make *checking* cheap, because they are the ones a future feature is most likely
to break quietly. A conforming implementation may rely on all of them, and any
proposed addition to the language should be measured against them.

**13.1 Parsing depends on nothing.** No production consults name resolution or
types (Section 12). Parsing is one pass, and files parse in parallel with no
coordination.

**13.2 Name resolution and type inference interleave, in a single traversal.**
This is the one place Buri gives something up. Resolving `x.f()` requires knowing
what `x` is, so the two cannot be separate passes.

What keeps it a single traversal rather than a fixpoint:

- Method resolution needs only the receiver's **head type constructor**, not its
  full type. `xs.first()` resolves in `core/list` whether `xs` is `[Int]` or
  `[T]` for an unresolved `T`.
- Type information flows **outside-in and left-to-right**. A lambda's parameter
  types come from the expected type at its call site, which is known before the
  body is visited.
- There is no overloading, so a name plus one type constructor selects exactly
  one definition.
- Conformance is nominal (Section 5.12.1), so a bound is a table lookup rather
  than a search that could need the rest of the program.

What would break it: overloading resolved by argument types, return-type-directed
dispatch, structural conformance, or any construct where a method call can appear
before its receiver's type constructor is determined.

**13.3 Function bodies check independently.** Top-level signatures are mandatory
(Section 9), so no inference crosses a function boundary. Bodies check in
parallel, and editing one body can never invalidate the check of another.

A consequence: a type variable still unconstrained when a body finishes checking
is unconstrained for good, because no other body and no signature can reach it.
It becomes `()`. This is not the literal defaulting of Section 5.1.1, which picks
between the types a numeric class admits; here nothing in the program picks at
all, so the type with one value and no structure is the answer. `assert.some(o)`
on an `Option` whose payload the program never names is the shape that reaches
it, and the value it renders is the `Option`, not the payload.

**13.4 A module's inter-module surface is exactly its exported declarations.**
Because conformance is declared rather than inferred from shape, adding or
removing a private function cannot change what any other module sees. Incremental
invalidation is therefore precise: a dependent is rechecked only when a
declaration it names actually changes.

**13.5 Monomorphization is a codegen concern, not a checking one.** A generic body
is checked once, polymorphically, with bounds verified at each call site. Checking
is O(code), not O(code × instantiations).

**13.6 Nothing in the checker requires a fixpoint.** No recursive trait solving
(no blanket impls, no associated types, no supertraits — Section 5.12.5), no
variance inference (no subtyping), no effect inference (effects are declared, not
deduced), and no cross-module exhaustiveness.

---
