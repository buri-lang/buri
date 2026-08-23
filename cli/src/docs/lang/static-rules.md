## 14. Static rules not expressed in the grammar

The grammar accepts a superset of well-formed programs. These are checked
afterward:

1. The head of a struct literal (`Expr { ... }`) must be a type path — optionally
   with type arguments, or the inferred-type dot form `.Variant` — not an arbitrary
   expression. The grammar permits `f(x) { a: 1 }`; the checker does not.
2. `let` patterns must be irrefutable.
3. `match` must be exhaustive, and no arm may be unreachable.
4. Or-pattern alternatives must bind identical names at identical types.
5. Array rest patterns appear only in final position; at most one per pattern.
6. Struct field names, enum variant names, context bindings, and match-arm
   pattern bindings must each be unique within their scope.
7. Every arm of a `match` produces a value of the arm type. There is no bottom
   type and no way to declare a branch unreachable, so an arm cannot opt out
   (Section 6.10).
8. A lambda may not capture a effect-carrying value, nor one whose type could be
   a context at some instantiation — that is, one mentioning a type parameter
   that carries no ordinary trait bound, anywhere `is effect-carrying` would
   have looked (Section 10.6). Function types are exempt: a closure holds only
   what this rule let it capture.
9. Opaque types may not be constructed or destructured outside their defining
   module, and private fields may not be read, written, or matched outside it.
10. Numeric conversion methods are declared per source-and-target pair in
    `core/num` (Section 6.2.1); there is no cast operator.
11. A numeric literal must be representable in the type it resolves to.
12. The dot form (`.Variant`) requires a known expected type.
13. `?` requires the enclosing function's return type to be a compatible `Result`
    or `Option`.
14. Recursive type definitions must be productive (a recursive enum must have at
    least one variant that does not recurse).
15. The head of a struct literal may not be a block-like expression; neither may
    the head of any postfix chain.
16. A value of type `Result<T, E>` may not be discarded by a `_` pattern
    (Section 5.7.1). Use `?`, `match`, `result.withDefault`, or the explicit
    `result.ignore`.

Methods and traits:

17. `self` may appear only as the first parameter of a function inside an `impl`
    block. Every function in an `impl` block must take one, and no function
    outside an `impl` block may.
18. A method may not share a name with a field of its `self` type.
19. A method call `x.f(...)` requires the receiver's type to be known and to have
    a defining module (Section 6.7.3), or to be a type parameter whose bounds
    declare `f`.
20. A method is not a value: `x.f` must be immediately called, and a method's
    name in module scope resolves for a re-export only, never as an expression.
21. `Self` is legal only inside a `trait` or `impl` body.
22. An `impl` may appear only in the defining module of its type. With a `for`
    clause it must supply every method the trait declares, with matching
    signatures; a primitive's methods belong to the `core` module named for it.
23. `derive` requires every field type (for a struct) or payload type (for an
    enum) to satisfy the derived trait.
24. A generic parameter's bounds must name declared traits. Inside the function,
    only the methods those traits declare are callable on that parameter.

Capabilities:

25. `ctx` may appear only as a function's first parameter, the parameter
    immediately after `self`, or a `let` binding name where a context may be
    constructed (Section 11.3).
26. A effect-carrying parameter must be `self` or `ctx`, at most one of each
    (Section 10.2). A type is effect-carrying if it is a type variable with a
    effect bound, a type that implements an effect, or any type that can hand
    one of those back — a type argument counts only in a position the
    constructor can hand back, which is why `fn(C, A) => B` and a constructor
    storing only such functions are data. A `context` expression is the only
    construct in which more than one effect-carrying value may appear.
27. `effect` declarations may appear only in platform modules, and no type may
    implement both an effect and a trait. An effect-carrying type satisfies no
    ordinary trait bound, so the separation survives composition: a
    `Holder<C>` that stores a context does not reach a `T: Eq` even where
    `Holder` implements `Eq` (Section 10.1).
28. `impl` and `derive` may not be exported, and may appear only in the defining
    module of the type they name. A method inside an inherent `impl` may be
    exported; a method supplied to a trait may not.
29. Numeric literals, conversions, and comparisons are ordinary methods; there is
    no cast operator.
30. `main` has the signature required by Section 11: no parameters, no generic
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
34. `"core/host"` is importable only from the module that exports `main`.

Modules and tests:

35. A module path names the standard library — `"core/..."` or `"ui/..."` — or
    this repository, `"//..."`. A relative path is an error, as is a `//` path
    the build system does not make visible to the importing target
    (Section 4.1.1). A path containing a `testing` segment is importable only
    from a test source.
36. A re-export may name only what its module path exports, and `export *` is
    not derivable.
37. `test`, and imports of test-only paths, may appear only in a test source. A
    test source may not `export`, and may not be imported.
38. An expression statement is legal only in a test source, and only when its
    type is `()`.
