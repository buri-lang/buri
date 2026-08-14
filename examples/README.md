# Buri examples

Read in order; each file builds on the ones before it. Every file is a complete
module that exports `main`, so each one is independently runnable once a
compiler exists.

| File | What it covers |
|---|---|
| [`01-hello.buri`](./01-hello.buri) | The smallest program; how `main` builds its context |
| [`02-literals-and-primitives.buri`](./02-literals-and-primitives.buri) | Numbers, strings, chars, operators, `Template` vs `Str` |
| [`03-functions.buri`](./03-functions.buri) | Declarations, lambdas, function types, turbofish |
| [`04-structs.buri`](./04-structs.buri) | Nominal structs, tuple structs, functional update |
| [`05-enums.buri`](./05-enums.buri) | Rust-style sum types, generic and recursive enums |
| [`06-pattern-matching.buri`](./06-pattern-matching.buri) | Every pattern form, guards, exhaustiveness |
| [`07-arrays.buri`](./07-arrays.buri) | `[T]`, `Option`-returning indexing, which ops allocate |
| [`08-tuples-and-structs.buri`](./08-tuples-and-structs.buri) | Tuples, nominal structs, field shorthand |
| [`09-option-and-result.buri`](./09-option-and-result.buri) | `?`, `??`, error types, no null and no exceptions |
| [`10-generics.buri`](./10-generics.buri) | Type parameters, row variables, passing operations |
| [`11-purity-and-context.buri`](./11-purity-and-context.buri) | **The core idea.** Pure / deterministic / effectful |
| [`12-file-io.buri`](./12-file-io.buri) | `Fs`, keeping the parsing pure and the I/O at the edge |
| [`13-network.buri`](./13-network.buri) | `Net`, retries, threading context through callbacks |
| [`14-effect-attenuation.buri`](./14-effect-attenuation.buri) | Static confinement by bound; attenuation by wrapper |
| [`15-modules.buri`](./15-modules.buri) | Imports, exports, opaque types, type aliases |
| [`16-recursion.buri`](./16-recursion.buri) | Accumulators, guaranteed tail calls, trees |
| [`17-blocks-and-scope.buri`](./17-blocks-and-scope.buri) | Method chains, blocks, shadowing, evaluation order |
| [`18-state-machine.buri`](./18-state-machine.buri) | Modeling state without mutable state |
| [`19-grammar-corners.buri`](./19-grammar-corners.buri) | One example per ambiguity-avoidance decision |
| [`20-word-count.buri`](./20-word-count.buri) | A complete program, effects only at the edges |
| [`21-methods.buri`](./21-methods.buri) | `impl` blocks; `self` parameters; resolution through the receiver's type |
| [`22-traits.buri`](./22-traits.buri) | Interfaces, structural satisfaction, `impl`, `derive`, operators |

## Reading the signatures

The fastest way to understand a Buri file is to read only the signatures and
sort them into three piles:

```buri
fn area(self: Shape): F64                                     // pure
fn normalize<C: Alloc>(self: [F64], ctx: C): [F64]            // deterministic, allocates
fn loadConfig<C: Alloc + Fs>(ctx: C, p: Str): ...             // touches the world
```

The first two are inside an `impl` block, which is where a `self` parameter is
legal; the third is a top-level declaration.

No parameter can hide an effect: one must be named `self` or `ctx`, and a lambda
may not capture one. So the pile a function belongs in is decided by its
first two parameters — you never read its body, or its callees' bodies, to know
what it can do.

## Conventions the examples follow

- **Receiver first, context second** — enforced, not merely conventional.
  `map(self, ctx, f)`, called as `xs.map(ctx, f)`.
- **Effects are trait bounds.** `<C: Alloc + Fs>` is the same feature as
  `<T: Ord + Show>`; there is one constraint mechanism in the language.
- **Everything is nominal.** No records, no structural conformance — every type
  has a declaration and every `impl` is written down.
- **Contexts are built in `main`, and nowhere else in a program.** `main` takes
  no parameters; it binds each effect it wants to an implementation from
  `core/host` and passes the result down. A test does the same thing with the
  test runner's implementations. Everything in between is generic over `<C: ...>`
  and never learns where its context came from.
- **Effect calls are bound with `let _ =`.** There are no expression statements.
- **`.Variant` over `Enum.Variant`** where the expected type is known.
- **No `Result` is ever discarded.** `Result` is must-use, so every `let _ =` in
  these files drops a `{}` — often one that a `?` has already unwrapped. Grep for
  `let _ =` and check: none of them throws away a failure.
- **Methods live in an `impl` block and declare `self`.** `x.f(a)` then
  resolves `f` among the methods of `x`'s type, which live in that type's
  defining module — so the call needs no import. Anything else is an ordinary
  call, declared at the top level.
- **Iteration is `fold` or explicit recursion.** There are no loops; tail calls
  are guaranteed eliminated, so an accumulator-passing helper is a real loop.

## What these examples assume

A standard library sketched in SPEC.md section 11.1. It is not normative in
v0.2 — the signatures used here are indicative, and the purity tier of each one
is the part that matters.
