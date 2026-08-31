## Three ideas

**1. There is no mutation.** Every binding is final. No references, no borrowing,
no lifetimes, no aliasing hazards. "Updating" a value produces a new one, and the
runtime is expected to make that cheap through structural sharing and in-place
update when a value is provably unshared — an implementation strategy that is
never observable.

**2. Effects travel through arguments.** An **effect** is an interface — declared
with `effect` instead of `trait`, and only by platform modules — and a function
names the ones it needs as bounds on its context parameter:

```buri sig role=platform
# from "core/effect" import { Alloc, IoError };
# struct User(Int);
# enum LoadError { NotFound }
effect Fs {
  fn readFile(self, path: Str): Result<Str, IoError>;
  fn writeFile(self, path: Str, body: Str): Result<(), IoError>;
}

fn loadUser<C: Alloc + Fs>(ctx: C, id: Str): Result<User, LoadError>;
```

The compiler enforces where they may appear: **an effect-carrying parameter must
be `self` or `ctx`**, never any other name or position. So the question "can this
function touch the world?" is answered by reading the first two parameters and
stopping. No type may implement both an effect and a trait, so the boundary
between the world and your data is checked rather than assumed.

The platform supplies the implementations that really do anything, in a module
`core/host` that only the file exporting `main` may import. `main` takes no
parameters: it names the effects it wants, binds each to one of those
implementations, and passes the result down. A program whose `main` never names
`host.net` cannot open a socket anywhere in its transitive call graph — nothing
anywhere can obtain a value bounded by `Net`. A test builds its context the same
way, from the test runner's implementations instead.

Purity is therefore not a keyword, not an inferred effect row, and not something
to propagate through signatures. It is the *absence* of one argument:

> If a function has no `ctx` parameter, no effect-carrying `self`, captures no
> effect, and builds no context, then it is deterministic, effect-free, and
> freely cacheable.

The last clause covers `main` and a test, the only two places a context is
built. Neither is a function library code can call, so in ordinary code the
check is still just: *is there a `ctx` parameter?*

Three tiers fall out — pure, deterministic, effectful — and each is visible in
the signature at a glance. Allocation is tracked separately from I/O, so "does
no I/O" and "does not allocate" are separately expressible. The table, and the
rule that decides which tier an operation is in, are in
[the standard library](./standard-library.md#the-purity-tiers).

**3. The grammar is context-free and unambiguous.** Parsing never consults name
resolution or the type checker. That is a design constraint that cost real
ergonomics, and [SPEC.md §12](../SPEC.md#12-why-the-grammar-is-context-free-and-unambiguous)
lists all seventeen decisions with what each one gave up — parenthesized `if`
conditions, no record field shorthand, non-associative comparison, no `<<`/`>>` tokens,
dot-prefixed variants in patterns, and the rest.
