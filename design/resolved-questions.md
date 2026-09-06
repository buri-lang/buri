# Questions the specification stopped asking

**The arguments behind decisions [`non-goals.md`](./non-goals.md) now states in
one line: two features that were specified and cut, and one open question that
got an answer.**

A maintainer document, like its neighbour. A user needs the outcome, and the
language reference states that where the feature would have been. This page keeps
the reasoning, because each argument constrains the next proposal that asks for
the same thing.

## Considered and cut: `for` and `while`

A `for`/`while` sugar was fully specified for v0.3, then removed.

The design: `for (x in xs) with (acc = init) { body }` desugared to a
tail-recursive local function, with the body evaluating to the next accumulator.
`while (cond) with (...)` worked the same way. A `Range` type and `a..b` /
`a..=b` operators came along so counting loops would not have to allocate an
array.

What it bought: familiar syntax for folds, and — the strongest argument — an
exemption from the capture rule of Section 10.6, since a loop body is inlined
control flow rather than a value of function type. You could write effectful
iteration directly instead of routing it through a `*Ctx` combinator.

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

If loops come back, they have to earn their keep on something other than
familiarity, and the capture-rule exemption has to be solved directly rather than
routed around.

## Considered and cut: the `|>` pipe operator

`x |> f(a)` meant `f(a, x)`, which is why the standard library originally put its
data *last*. Method syntax (Section 6.7) covers the case that mattered —
chaining operations that belong to a type — and resolves them with no import.
That left `|>` one job: chaining a function that is not a method of the
receiver's type. A `let` sequence reads at least as well, in a language that
already has no expression statements.

By the same standard that cut loops, it did not earn its keep. Removing it also
freed the argument convention: with `|>` gone, the receiver could move to the
front (Section 10.7), where it reads correctly for methods and for direct calls
alike.

## Answered: `I64` on a JavaScript target

This was an open question. The answer came back, and not the way the entry that
raised it expected.

`Int` is `I64` on every target. The question was whether "undefined above 2^53"
is a rule programmers internalize or one they discover. They discover it:
buri-lang/buri#8 and #4 are the same person finding it twice, from two
directions, porting nanosecond timestamps. So `I64`, `U64`, `I128` and `U128` are
`BigInt`s on that backend now.

The objection the entry raised — it taxes every loop counter for a case most
never reach — is real, and the design pays it rather than arguing it away. The
narrow widths keep the `number` representation, and a loop counter that does not
need the range can say `I32`.
[`native/VALUE-MODEL.md`](./native/VALUE-MODEL.md) §12 has the size of the tax,
measured on the conformance corpus rather than guessed. The alternative that
stays refused is a target-dependent `Int` width, which trades a performance
problem for a portability one.
