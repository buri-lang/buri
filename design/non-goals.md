# Non-goals and open questions

**What the language deliberately does not have, and the trade-offs that are still
open. A maintainer document: it was section 14 of the specification, and a
proposal to add any of it has to argue with what is written here.**

## Not in v0.3, and not planned

Mutation, references, lifetimes, classes, inheritance, dynamic dispatch, trait
objects, `null`, exceptions, implicit conversions (beyond `Str → Template`),
overloading, macros, reflection.

## Not in v0.3

Loops, and the `|>` pipe operator. Both were specified for this version and then
cut. There is no `for` and no `while`: the sugar that was specified — `for (x in
xs) with (acc = init) { body }` — desugared to a tail-recursive local function,
which is what `xs.fold(fn(n, x) => n + x, 0)` already is, so it bought familiar
syntax and no new expressive power in exchange for a `Range` type, two new
operators, and a body typing rule that changed shape with its `with` clause.
Write the fold, or write the recursion; a tail call costs one frame (Section
8.3.1). There is no `x |> f(a)` either: method syntax (Section 6.7) covers
chaining operations that belong to a type, and covers it with resolution that
needs no import, and cutting the operator freed the receiver to move to the front
of the argument list (Section 10.7).

`resolved-questions.md` has the full argument for both, and the case a future
proposal has to beat.

## Deferred to a later version

Blanket implementations; associated types; `where` clauses; supertraits;
implementing a trait for a foreign type; dict and set literals; fixed-length
array types; `async`; ranges; a module-level effect summary in generated
documentation.

Each item in that first group turns trait resolution from a lookup into a search
(Section 5.12.5). They are deferred together, deliberately.

## Open questions, honestly flagged

1. *The capture rule (10.6) is strict.* It buys a clean purity theorem at the cost
   of ergonomic effectful higher-order code: every effectful traversal goes
   through a `*Ctx` combinator or hand-written recursion. The alternative —
   encoding a captured-effect row in the function type, for example
   `fn(Str) => Str uses { fs: Fs }` — is more expressive but adds an effect system
   to a language whose selling point is not having one. This is the language's
   sharpest unresolved trade-off, and cutting loops put the full cost back on it.
   Traits do not help: a trait method that needs an effect must declare the
   context in its signature, which is the honest encoding but not a convenient
   one.
2. *`Alloc` granularity.* Requiring `Alloc` for every size-dependent result is
   principled and noisy. Whether the noise is worth the guarantee is an empirical
   question.
3. *Indexing returns `Option`.* Correct, and occasionally miserable. A
   `list.getOr(default, i, xs)` helper and better pattern-matching over arrays may
   absorb most of the pain.
4. *Trampolining higher-order tail calls.* Section 8.3.1 specifies how tail-call
   elimination is achieved on a target without native support, and the first two
   cases are exact and free. The third — a tail call through a value of function
   type — costs an allocation per bounce, and how often that shape occurs in real
   Buri code is unknown. If it turns out to be common, the answer is probably
   call-site specialization rather than a language change.
5. *Methods are not extensible, and not available on type variables* (6.7.3).
   Resolving through the receiver's defining module is what makes them
   import-free and collision-free, and it is the same property that stops you
   adding an operation to `Str`. Calling a method on a bare `T` is what bounds are
   for (5.10); extending a foreign type is what free functions are for. Neither
   gap has a fix that preserves import-free, collision-free resolution.
6. *Whether the compilation invariants survive contact with real features.* They
   are written down — `cli/src/docs/guides/compile-speed.md` is the page — which
   is the point, but every one of them is the kind of property that a
   reasonable-looking addition erodes. The interleaving of name resolution and
   type inference is the fragile one.
7. *Holding the line on 5.12.5.* Restricted traits are cheap precisely because
   resolution is a lookup. Every deferred feature — blanket impls, associated
   types, `where` chains, foreign impls — individually looks reasonable and
   collectively turns the lookup into a search. By then the compiler's
   architecture will assume constant-time resolution. The risk is not the cost of
   what was built; it is the difficulty of refusing the next request.
8. *Must-use is hard-coded to `Result` (5.7.1).* A general `@mustUse` marker on
   user types would be more honest than a compiler that knows one type by name,
   but it is the first piece of attribute syntax in a language with none, and
   `Result` covers the case that actually bites. Revisit if a second must-use
   type shows up in practice.

A question that has been answered leaves this list. `resolved-questions.md` keeps
the ones that did, with what the answer cost.

A bare "Section N.M" above is a section of the language reference, under
[`cli/src/docs/language/`](../cli/src/docs/language/).
