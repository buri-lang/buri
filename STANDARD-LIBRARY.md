# The Buri standard library

`core/*` ships with the toolchain. It is never listed in a `dependencies`, it
is available to every target, and it cannot be replaced — there is one, and
this is it.

The reference for any module is the module: `buri docs std core/list` renders
it from the source, so it cannot drift. This document is about the parts that
are decisions rather than signatures — what is here, what is deliberately not,
and what each thing costs.

## The purity tiers

Every function sits in one of three tiers, and the tier is visible in the
signature rather than in a comment. This is `SPEC.md` §10.5, applied:

| Tier | Shape | Example |
|---|---|---|
| **Pure** | no `ctx` parameter | `xs.len()`, `date.weekday(d)`, `v.dot(o)` |
| **Deterministic** | `ctx` bounded by `Alloc` only | `xs.map(ctx, f)`, `json.stringify(ctx, v)` |
| **Effectful** | `ctx` bounded by anything else | `fs.readText(ctx, p)`, `time.now(ctx)` |

The rule that decides it: an operation whose result size is fixed is pure, and
one whose result size depends on runtime data names `Alloc`. `len` and `fold`
are pure; `map` and `filter` are not. A `F32x4` is four numbers in a struct, so
every operation in `core/simd` is pure — the vector types are exactly the shape
the rule was drawn around.

## What is here

### Values and control

`core/option`, `core/result`, `core/order`, `core/num`, `core/bool`,
`core/math`, `core/bits`.

`Option`, `Result`, `Order` and the comparison and operator traits are in the
prelude, so `derive Eq for Point;` works in a module that imports nothing.

### Text

`core/str`, `core/char`, `core/bytes`, `core/json`.

- **`core/bytes`** — UTF-8, hex, base64. Free functions rather than methods on
  `[U8]`: a method may only be declared in its type's defining module, and
  `[T]`'s is `core/list`. Decoding is strict, and validates before it
  allocates: an overlong UTF-8 encoding, a truncated sequence, or a surrogate
  is an error at a named index, not a replacement character.
- **`core/json`** — a `Json` tree, `parse`, and `stringify`. **An object is an
  ordered association list, not a map**, so key order round-trips, no `Hash`
  bound is needed, and `get` is O(n). Every number is a `Float`, which is what
  JSON says a number is. Nesting is capped at `MAX_DEPTH`, because parsing
  recurses and the recursion is not in tail position — without the cap a deep
  enough document is a crash rather than an error.

  There is **no `derive`-driven mapping to your own types**. Buri has no
  reflection and no macros, and `derive` takes a fixed list, so that has to be
  a language feature rather than a library. See the roadmap in `TODO.md`.

### Collections

`core/list`, `core/queue`, `core/map`, `core/set`, `core/bitset`.

Every one of these is a value, so every "modification" returns a new one. That
is not free, and the cost is stated per module rather than implied:

| | Lookup | Insert | Note |
|---|---|---|---|
| `core/queue` | O(1) | O(1) amortized | Banker's deque: two lists, the front reversed. The reversal is what makes both ends an append. |
| `core/map`, `core/set` | O(1) expected | O(b) in buckets | Buckets of association lists. Grows and rehashes past a load factor of 4. **Iteration order is unspecified and will change.** |
| `core/bitset` | O(1) | O(n/32) | 32 bits to an `Int` word — 32 and not 64 because `Int` is signed, and a bit in position 63 would make every shift a question about sign extension. |

`Queue`, `Map`, `Set` and `BitSet` provide `equals` rather than deriving `Eq`,
because a derived `Eq` would compare the *representation*: two queues holding
the same elements need not have the same front/back split, and two maps built
in different orders need not have the same bucket layout.

### Numbers and vectors

`core/simd` — `F32x4` and `I32x4`.

**On the JavaScript backend these are scalar and buy no speed.** There is no
SIMD reachable from a plain `.mjs` artifact. What they buy is the shape: a
kernel written lane-wise, with no loop-carried dependency, is the form a
backend with vector registers can lower directly, and the same kernel written
as a fold over a list is not, because a fold says "in this order". Do not
benchmark against a scalar loop expecting a win; there is not one today.

### Time

`core/time` is the clock, and reading it is an effect. `core/date` is the
calendar, and none of it is: what day of the week a date falls on does not
depend on anything.

`core/date` uses Hinnant's `days_from_civil`, which is exact over the whole
range of `Int` using integer arithmetic only. `Duration` is a length and
`Instant` is a point, and they are different types on purpose.

**There is no timezone database, and there will not be one.** tzdata is
megabytes that change several times a year, and this toolchain has no
dependencies and ships no data files. `Zoned` carries a fixed offset in
minutes, which covers UTC, a stored offset, and arithmetic within one offset.
It does not cover `America/New_York`, and it does not pretend to.

### Cryptography

`core/crypto` — SHA-256, HMAC-SHA-256, and a constant-time comparison.

Written in Buri rather than handed to the platform, for the same reason
`cli/src/build/cache.rs` hand-writes SHA-256 in Rust: the toolchain is pinned by hash
and a dependency tree is a second thing to pin. The two implementations are
checked against the same NIST vectors, in two languages, neither of which can
compile the other.

Deliberately absent, and not by oversight:

- **No ciphers.** Shipping a block function without a key schedule, a mode, a
  nonce discipline, and an authentication tag is how people end up with ECB.
- **No public-key anything.**
- **No cryptographically secure randomness.** `core/random` is seeded and
  reproducible on purpose — a hermetic test needs the same numbers every run.
  A CSPRNG is a different thing, and it would need a new `Entropy` effect in
  `core/cap` with a host implementation behind it. That is a decision to take
  deliberately, not a function to add quietly beside these.

`sha256` is **not a password hash**. It is fast, which is the wrong property.

### The platform

`core/cap` declares the effects; `core/host` implements them and may be
imported only by the module that exports `main`. `core/io`, `core/fs`,
`core/env`, `core/random`, `core/net/http` are the interfaces those effects
are used through. `core/testing/assert` and `core/testing/context` are
importable only from a test source.

## What is deliberately not here

**`MultiArrayList` / struct-of-arrays.** Not expressible, and not honourable on
this backend:

1. A struct is already an array in the JavaScript representation, so `[Point]`
   is an array of arrays. A columnar `{ xs: [Float], ys: [Float] }` really is
   faster in a JIT — but a user gets that today by writing the two-field struct
   themselves, and a library adds nothing.
2. A generic `MultiArrayList<T>` is not typeable. Exposing "column *i* of `T`,
   at `T`'s *i*-th field type" needs dependent or row types, and `SPEC.md` §5.5
   has no records.
3. The version that would work is a type-generating `derive` — `derive Soa for
   Point;` producing a `PointSoa` and its accessors. Today `derive` only
   attaches conformance; generating a *new type* is a language change, and it
   belongs beside the native backend, which is where the layout would actually
   pay.

**Typed JSON encoding.** See `core/json` above.

**Allocators — `GeneralPurpose`, `Arena`, `FixedBuffer`.** `Alloc` is a
type-level budget here and nothing more: a function that allocates says so in
its signature, and nothing counts. That is the honest state on a backend with a
garbage collector, where an arena would reclaim nothing and a general-purpose
allocator would report a synthetic number rather than a measurement. The three
types are worth having when there is real memory underneath, which is the
native backend's job. `TODO.md` keeps the design notes.

## Two rules for anything added here

1. **Every body-less declaration needs a conformance test that calls it.**
   `cli/tests/stdlib.rs` stops after type checking, so a declaration with no
   runtime function behind it passes that suite silently and fails only when a
   real program reaches it. The suites under `cli/tests/conformance/lib/` are
   what actually run the code.
2. **State the cost.** Every structure here is persistent, and persistent
   structures have costs that mutable ones do not. Saying `set` is O(n/32) is
   better than implying the O(1) a mutable bit set would have.
