# Questions the specification stopped asking

**The arguments behind decisions [`non-goals.md`](./non-goals.md) now states in
one line: two features that were specified and cut, and one open question that
got an answer.**

This is a maintainer document, and so is its neighbour: what a user needs is the
outcome, which the language reference states where the feature would have been.
What is kept here is the reasoning, because each of these constrains the next
proposal that asks for the same thing.

## Considered and cut: `for` and `while`

A `for`/`while` sugar was fully specified for v0.3 and then removed.

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

## Considered and cut: the `|>` pipe operator

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

## Answered: `I64` on a JavaScript target

This was an open question, and it was answered — not the way the entry that
raised it expected.

`Int` is `I64` on every target, and the question was whether "undefined above
2^53" is a rule programmers internalize or one they discover. It is one they
discover: buri-lang/buri#8 and #4 are the same person finding it twice, from two
directions, porting nanosecond timestamps. So `I64`, `U64`, `I128` and `U128`
are `BigInt`s on that backend now.

The objection the entry raised to that — it taxes every loop counter for a case
most never reach — is real and was paid rather than argued away: the narrow
widths keep the `number` representation, and a loop counter that does not need
the range can say `I32`. What the tax actually is, measured on the conformance
corpus rather than guessed, is in [`native/VALUE-MODEL.md`](./native/VALUE-MODEL.md)
§12. The alternative that stays refused is a target-dependent `Int` width, which
trades a performance problem for a portability one.
