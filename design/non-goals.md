# Non-goals and open questions

**What the language deliberately does not have, and the trade-offs that are
still open. A maintainer document: it was section 14 of the specification, and
any proposal to add one of these has to argue with what is written here.**

## Not in v0.3, and not planned

Mutation, references, lifetimes, classes, inheritance, dynamic dispatch, trait
objects, `null`, exceptions, implicit conversions (beyond `Str → Template`),
overloading, macros, reflection.

## Not in v0.3

Loops, and the `|>` pipe operator. Both were specified for this version, then
cut. Neither bought expressive power: `for (x in xs) with (acc = init) { body }`
desugars to the tail-recursive local function `xs.fold` already is, and a tail
call costs one frame (Section 8.3.1); method syntax (Section 6.7) chains
operations that belong to a type with no import, and cutting `|>` freed the
receiver to move to the front of the argument list (Section 10.7).

[`resolved-questions.md`](./resolved-questions.md) has the full argument for
both, and the case a future proposal has to beat.

## Deferred to a later version

Blanket implementations; associated types; `where` clauses; supertraits;
implementing a trait for a foreign type; dict and set literals; fixed-length
array types; `async`; ranges; a module-level effect summary in generated
documentation.

Every item in that first group turns trait resolution from a lookup into a
search (Section 5.12.5). That is why they are deferred together.

## Struct-of-arrays, and the `derive` that would generate a type

One request keeps reaching the standard library wearing three faces: a
`MultiArrayList<T>`, a columnar layout, a `derive Soa`. They are one language
gap. You cannot express it today, and on the JavaScript backend expressing it
would not pay:

1. **A struct is already an array** in the JavaScript representation, so
   `[Point]` is an array of arrays. A columnar `{ xs: [Float], ys: [Float] }`
   really is faster in a JIT, but you get that today by writing the two-field
   struct yourself.
2. **You cannot type a generic `MultiArrayList<T>`.** Exposing "column *i* of
   `T`, at `T`'s *i*-th field type" needs dependent or row types, and Section 5.5
   has no records.
3. **The version that would work is a type-generating `derive`**: `derive Soa
   for Point;` producing a `PointSoa` and its accessors. Today `derive` only
   attaches a conformance to a type that already exists. Generating a *new type*
   is a language change, and it belongs beside the native backend, where the
   layout would actually pay.

Point 3 carries weight well beyond layout, and `core/cli`'s own module comment
cites it: a typed command handler would need `derive` to build a struct out of a
value, so a handler takes an `Arguments` and asks it by name instead.

## Open questions, honestly flagged

1. *The capture rule (10.6) is strict.* It buys a clean purity theorem and
   charges for it in effectful higher-order code: every effectful traversal goes
   through a `*Ctx` combinator or hand-written recursion. The alternative encodes
   a captured-effect row in the function type, say
   `fn(Str) => Str uses { fs: Fs }` — more expressive, but it adds an effect
   system to a language whose selling point is not having one. This is the
   language's sharpest unresolved trade-off, and cutting loops put the full cost
   back on it. Traits do not help: a trait method that needs an effect must
   declare the context in its signature.
2. *`Allocator` granularity.* Demanding `Allocator` for every size-dependent result is
   principled and noisy. Only real code can say whether the noise is worth the
   guarantee.
3. *Indexing returns `Option`.* Correct, and occasionally miserable. A
   `list.getOr(default, i, xs)` helper and better pattern matching over arrays
   may absorb most of the pain.
4. *Trampolining higher-order tail calls.* Section 8.3.1's first two cases are
   exact and free. The third — a tail call through a value of function type —
   costs an allocation per bounce, and nobody knows how often that shape turns up
   in real Buri code. If it is common, the fix is probably call-site
   specialization rather than a language change.
5. *Methods are not extensible, and not available on type variables* (6.7.3).
   Resolving through the receiver's defining module keeps methods import-free and
   collision-free, and the same property stops you adding an operation to `Str`.
   Bounds cover calling a method on a bare `T` (5.10); free functions cover
   extending a foreign type. Neither gap has a fix that keeps resolution
   import-free and collision-free.
6. *Whether the compilation invariants survive contact with real features.*
   `cli/src/docs/guides/compile-speed.md` writes them down, and every one is the
   kind of property a reasonable-looking addition erodes. Interleaving name
   resolution with type inference is the fragile one.
7. *Holding the line on 5.12.5.* Restricted traits are cheap precisely because
   resolution is a lookup. Each deferred feature — blanket impls, associated
   types, `where` chains, foreign impls — looks reasonable on its own, and
   together they turn the lookup into a search. The risk is not what it cost to
   build; it is how hard it will be to refuse the next request.
8. *Must-use is hard-coded to `Result` (5.7.1).* A general `@mustUse` marker on
   user types would be more honest, but it would be the first attribute syntax in
   a language with none, and `Result` covers the case that actually bites.
   Revisit if a second must-use type shows up in practice.

An answered question leaves this list; `resolved-questions.md` keeps the ones
that did. A bare "Section N.M" above points at a section of the language
reference, under [`cli/src/docs/language/`](../cli/src/docs/language/).
