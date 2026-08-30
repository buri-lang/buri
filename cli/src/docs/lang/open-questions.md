## 15. Non-goals and open questions

**Not in v0.3, and not planned:** mutation, references, lifetimes, classes,
inheritance, dynamic dispatch, trait objects, `null`, exceptions, implicit
conversions (beyond `Str → Template`), overloading, macros, reflection.

**Not in v0.3:** loops, and the `|>` pipe operator. See below.

**Deferred to a later version:** blanket implementations; associated types;
`where` clauses; supertraits; implementing a trait for a foreign type; dict and
set literals; fixed-length array types; `async`; ranges; a module-level effect
summary in generated documentation.

Each item in that first deferred group turns trait resolution from a lookup into
a search (Section 5.12.5). They are deferred together, deliberately.

### 15.1 Considered and cut: `for` and `while`

A `for`/`while` sugar was fully specified for this version and then removed. The
reasoning is recorded here because it constrains any future attempt.

The design was: `for (x in xs) with (acc = init) { body }` desugaring to a
tail-recursive local function, where the body evaluates to the next accumulator;
`while (cond) with (...)` likewise; plus a `Range` type and `a..b` / `a..=b`
operators so that counting loops would not have to allocate an array.

What it bought: familiar syntax for folds, and — the strongest argument — an
exemption from the capture rule of Section 10.6, since a loop body is inlined
control flow rather than a value of function type. Effectful iteration could be
written directly instead of through a `*Ctx` combinator.

What it cost, and why it lost:

- **Two ways to say one thing.** `for (x in xs) with (n = 0) { n + x }` and
  `xs.fold(fn(n, x) => n + x, 0)` are the same program. A small language
  that offers both has to teach both, and every codebase splits on which to use.
- **The sugar was not simple.** A `with` clause whose scope differs between
  `for` and `while`, an optional index binding, a body typing rule that changes
  with the presence of `with`, a special termination check for effect-free
  `while` conditions, plus a `Range` type, a `core/range` module, and two new
  operators with their own ambiguity argument. That is a lot of specification
  for zero new expressive power.
- **It made the capture rule inconsistent.** Exempting loop bodies is sound, but
  it means "can this construct see an effect?" stops having one answer. Better
  to keep the rule absolute and treat its cost as the open question it is.

If loops return, the case to beat is: they must earn their keep on something
other than familiarity, and the capture-rule exemption should be solved directly
rather than routed around.

### 15.2 Considered and cut: the `|>` pipe operator

`x |> f(a)` meant `f(a, x)`, and it is why the standard library originally put
its data *last*. Method syntax (Section 6.7) covers the case that mattered —
chaining operations that belong to a type — and covers it with resolution that
needs no import. What remained for `|>` was chaining a function that is not a
method of the receiver's type, which reads at least as well as a `let` sequence
in a language that already has no expression statements.

By the same standard that cut loops, it did not earn its keep. Removing it also
freed the argument convention: with `|>` gone, the receiver could move to the
front (Section 10.7), where it reads correctly for methods and for direct calls
alike.

### 15.3 Open questions, honestly flagged

1. *The capture rule (10.6) is strict.* It buys a clean purity theorem at the cost
   of ergonomic effectful higher-order code: every effectful traversal goes
   through a `*Ctx` combinator or hand-written recursion. The alternative —
   encoding a captured-effect row in the function type, for example
   `fn(Str) => Str uses { fs: Fs }` — is more expressive but adds an effect system
   to a language whose selling point is not having one. This is the language's
   sharpest unresolved trade-off, and cutting loops (15.1) put the full cost back
   on it. Traits do not help: a trait method that needs an effect must declare
   the context in its signature, which is the honest encoding but not a
   convenient one.
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
6. *Whether the Section 13 invariants survive contact with real features.* They
   are now written down, which is the point — but every one of them is the kind
   of property that a reasonable-looking addition erodes. 13.2 is the fragile
   one.
7. *Holding the line on 5.12.5.* Restricted traits are cheap precisely because
   resolution is a lookup. Every deferred feature — blanket impls, associated
   types, `where` chains, foreign impls — individually looks reasonable and
   collectively turns the lookup into a search. By then the compiler's
   architecture will assume constant-time resolution. The risk is not the cost of
   what was built; it is the difficulty of refusing the next request.
8. *`I64` on a JavaScript target.* **Answered, and not the way this entry
   expected.** `Int` is `I64` on every target, and the question was whether
   "undefined above 2^53" is a rule programmers internalize or one they
   discover. It is one they discover: buri-lang/buri#8 and #4 are the same
   person finding it twice, from two directions, porting nanosecond timestamps.
   So `I64`, `U64`, `I128` and `U128` are `BigInt`s on that backend now. The
   objection this entry raised to that — it taxes every loop counter for a case
   most never reach — is real and was paid rather than argued away: the
   narrow widths keep the `number` representation, and a loop counter that does
   not need the range can say `I32`. What the tax actually is, measured on the
   conformance corpus rather than guessed, is in
   `design/native/VALUE-MODEL.md` §12. The alternative that stays refused is a
   target-dependent `Int` width, which trades a performance problem for a
   portability one.
9. *Must-use is hard-coded to `Result` (5.7.1).* A general `@mustUse` marker on
   user types would be more honest than a compiler that knows one type by name,
   but it is the first piece of attribute syntax in a language with none, and
   `Result` covers the case that actually bites. Revisit if a second must-use
   type shows up in practice.
