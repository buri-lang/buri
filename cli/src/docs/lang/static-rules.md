## 14. Static rules not expressed in the grammar

The grammar accepts a superset of well-formed programs. These are checked
afterward. Each is one line here and is argued where it is cited; this section is
the index, not the explanation.

1. The head of a struct literal (`Expr { ... }`) must be a type path — optionally
   with type arguments, or the inferred-type dot form `.Variant` — not an
   arbitrary expression.
2. `let` patterns must be irrefutable (Section 6.3).
3. `match` must be exhaustive, and no arm may be unreachable (Section 7.3).
4. Or-pattern alternatives must bind identical names at identical types
   (Section 7.1).
5. Array rest patterns appear only in final position; at most one per pattern
   (Section 7.1).
6. Struct field names, enum variant names, context bindings, and match-arm
   pattern bindings must each be unique within their scope.
7. Every arm of a `match` produces a value of the arm type, and no arm may opt
   out (Section 6.10).
8. A lambda may not capture an effect-carrying value, nor one whose type could be
   a context at some instantiation (Section 10.6).
9. Private fields may not be read, written, or matched outside the module that
   declares them (Section 5.6).
10. Numeric conversions are ordinary methods, declared per source-and-target pair
    in `core/num` (Section 6.2.1).
11. A numeric literal must be representable in the type it resolves to
    (Section 5.1.1).
12. The dot form (`.Variant`) requires a known expected type (Section 5.7).
13. `?` requires the enclosing function's return type to be a compatible `Result`
    or `Option` (Section 6.8).
14. Recursive type definitions must be productive: a recursive enum must have at
    least one variant that does not recurse.
15. The head of a struct literal may not be a block-like expression; neither may
    the head of any postfix chain (Section 12.13).
16. A value of type `Result<T, E>` may not be discarded by a `_` pattern
    (Section 5.7.1).

Methods and traits:

17. `self` may appear only as the first parameter of a function inside an `impl`
    block, and is written without a type. Every function in an `impl` block must
    take one, and no function outside an `impl` block may (Section 6.7.1).
18. A method may not share a name with a field of its `self` type
    (Section 12.16).
19. A method call `x.f(...)` requires the receiver's type to be known and to have
    a defining module, or to be a type parameter whose bounds declare `f`
    (Section 6.7.3).
20. A method is not a value: `x.f` must be immediately called, and a method's
    name in module scope resolves for a re-export only, never as an expression
    (Section 6.7.3).
21. `Self` is legal only inside a `trait` or `impl` body (Section 5.12).
22. An `impl` may appear only in the defining module of its type. With a `for`
    clause it must supply every method the trait declares, with matching
    signatures; a primitive's methods belong to the `core` module named for it
    (Section 5.12.2).
23. `derive` requires every field type, or payload type, to satisfy the derived
    trait (Section 5.12.3).
24. A generic parameter's bounds must name declared traits, and inside the
    function only those traits' methods are callable on that parameter
    (Section 5.10).

Effects and contexts:

25. `ctx` may appear only as a function's first parameter, the parameter
    immediately after `self`, or a `let` binding name where a context may be
    constructed (Section 11.3).
26. An effect-carrying parameter must be `self` or `ctx`, at most one of each,
    and a `context` expression is the only construct in which more than one
    effect-carrying value may appear (Section 10.2).
27. `effect` declarations may appear only in platform modules, and no type may
    implement both an effect and a trait — so an effect-carrying type satisfies
    no ordinary trait bound, however it is composed (Section 10.1).
28. `impl` and `derive` may not be exported, and may appear only in the defining
    module of the type they name. A method inside an inherent `impl` may be
    exported; a method supplied to a trait may not (Section 6.7.1).
29. There is no cast operator: `as` is legal only in an import specifier
    (Section 12.5).
30. `main` has the signature Section 11 requires: no parameters, no generic
    parameters, returning `Result<(), Str>`.

Contexts (Section 11.3):

31. A `context` declaration may appear only in the module exporting `main`, in a
    test source, or in a test-only module, and may be exported only from a
    test-only module.
32. A context may be *constructed* — by a `context` expression, or by calling a
    named context — only inside `main`'s body, in a test source, or in a
    test-only module. Never inside a lambda.
33. Each binding's left side names a declared effect, bound at most once across
    the spread and the explicit bindings; each right side's type must implement
    that effect. The result satisfies exactly the effects bound.
34. `"core/host"` is importable only from the module that exports `main`, and
    what it exports is **what the output's platform grants** — so the check is
    per output, and binding a name a platform withholds is an ordinary
    unresolved name (`host-not-granted`, Section 10.3).

Modules and tests:

35. A module path names the standard library — `"core/..."` or `"ui/..."` — or
    this repository, `"//..."`; there are no relative paths, and a `//` path the
    build system does not make visible to the importing target is an error. A
    path containing a `testing` segment is importable only from a test source
    (Section 4.1.1).
36. A re-export may name only what its module path exports, and `export *` is
    not derivable (Section 4.2.1).
37. `test`, and imports of test-only paths, may appear only in a test source. A
    test source may not `export`, and may not be imported (Section 11.2).
38. An expression statement is legal only in a test source, and only when its
    type is `()`. Any expression qualifies — a call, a `match`, an `if`, a
    block — and every one of them is terminated by `;` (Section 11.2.1).
