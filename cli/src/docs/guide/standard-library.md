## The standard library

The standard library ships with the toolchain. It is never listed in a
`dependencies`, it is available to every target, and it cannot be replaced —
there is one, and this is it. It owns two reserved module roots: `core/*`, the
deliberately small set of essentials, and `ui/*`, the reactivity vocabulary,
which is a different kind of thing and a much larger surface.

**The reference for a module is the module.** `buri docs core/list` renders it
from the source the compiler checked, so a signature on the page is the
signature that exists, and `buri docs core/list.map` renders one item of it.
`buri docs` lists every module. This page is the map over the top of that:
which modules there are, what each one costs, and what is deliberately absent.

### The purity tiers

Every function sits in one of three tiers, and the tier is visible in the
signature rather than in a comment. This is
[`SPEC.md` §10.5](./cli/src/docs/SPEC.md), applied:

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

### Values and control

`core/option`, `core/result`, `core/order`, `core/num`, `core/bool`,
`core/math`, `core/bits`.

`Option`, `Result`, `Order` and the comparison and operator traits are in the
prelude, so `derive Eq for Point;` works in a module that imports nothing.

### Text

`core/str`, `core/char`, `core/bytes`, `core/json`, `core/proto`.

- **`core/bytes`** — UTF-8, hex, base64, varints. Free functions rather than
  methods on `[U8]`: a method may only be declared in its type's defining
  module, and `[T]`'s is `core/list`. Decoding is strict, and validates before
  it allocates: an overlong UTF-8 encoding, a truncated sequence, or a
  surrogate is an error at a named index, not a replacement character.

  The varints live here rather than in `core/proto`, beside hex and base64,
  because a varint is an encoding of a number as bytes, it has exactly one
  definition, and anything speaking a length-prefixed format needs the same
  one. They do 64-bit arithmetic on two 32-bit halves, so a negative `int64`
  writes the ten bytes protoc writes; an `Int` is still a double, so a value
  past 2^53 survives only to that precision.

- **`core/json`** — a `Json` tree, `parse`, and `stringify`. **An object is an
  ordered association list, not a map**, so key order round-trips, no `Hash`
  bound is needed, and `get` is O(n). Every number is a `Float`, which is what
  JSON says a number is. Nesting is capped at `MAX_DEPTH`, because parsing
  recurses and the recursion is not in tail position — without the cap a deep
  enough document is a crash rather than an error.

  **`derive ToJson` and `derive FromJson` map it to your own types**, and
  `encode` and `decode` are the two functions that use them. Both are on
  `derive`'s fixed list — no reflection and no macro is involved. The mapping
  from Buri shapes onto JSON ones is stated in the module's own source; the
  decisions with something at stake in them are that an enum is externally
  tagged, that a positional struct is an array whatever its arity, and that
  `Option<T>` is `T` or `null` — so `Option<Option<T>>` does not round-trip.

  Both traits are **derived and never written by hand**, which the compiler
  enforces. A derived encoder stands for the type's shape, so a hand-written
  one would be called where the type is encoded on its own and silently skipped
  where a type holding it is.

- **`core/proto`** — the protobuf wire format: tags, wire types, the packed
  readers, and `ProtoError`. Nothing here is written by hand either, but for a
  different reason: a `.proto` schema in a package *becomes* a module, and this
  is the part of that generated code which is the same for every schema. See
  [the proto reference](./cli/src/docs/build/proto.md) for the mapping and for
  why those codecs are generated Buri rather than a descriptor walk.

  Reading a request and writing a reply over a pipe is what `Stdin.readBytes`
  and `Stdout.writeBytes` are for: `readLine` reads the stream to its end, so a
  program using it cannot answer before the other side has finished speaking.
  Text and octets are two questions about one stream, so they are two
  operations, and a program should ask only one of them.

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

`core/crypto` — SHA-256, HMAC-SHA-256, and a constant-time comparison. Written
in Buri rather than handed to the platform, because a dependency tree is a
second thing to audit. It is checked against the NIST vectors, as is the
independent SHA-256 the build cache uses, in two languages neither of which can
compile the other.

Deliberately absent, and not by oversight:

- **No ciphers.** Shipping a block function without a key schedule, a mode, a
  nonce discipline, and an authentication tag is how people end up with ECB.
- **No public-key anything.**
- **No cryptographically secure randomness.** `core/random` is seeded and
  reproducible on purpose — a hermetic test needs the same numbers every run.
  A CSPRNG is a different thing, and it would need a new `Entropy` effect in
  `core/effect` with a host implementation behind it. That is a decision to take
  deliberately, not a function to add quietly beside these.

`sha256` is **not a password hash**. It is fast, which is the wrong property.

### The platform

`core/effect` declares the effects; `core/host` implements them and may be
imported only by the module that exports `main`. `core/io`, `core/fs`,
`core/env`, `core/random`, `core/net/http` are the interfaces those effects
are used through. `core/testing/assert` and `core/testing/context` are
importable only from a test source.

### User interfaces

`ui/effect` declares `Watch`, `Ui` and `Fetch`, and the `Scope` a reactive
closure is handed. `ui/signal` is `Signal<T>` — `get`, `set`, `update` — plus
`signal` and `watch`; `ui/prop` is `Prop<T>` and `memo`. `ui/testing` is a
headless platform, importable only from a test source.

The whole of it rests on one idea: **a signal handle is inert data, and the
authority to read or write it travels through `ctx`** — the same split `Alloc`
and `Region` use. So a `Signal<T>` may be captured by an event handler, and the
handler takes its context as a parameter rather than closing over one.

| | Cost |
|---|---|
| `signal(ctx, v)` | O(1) |
| `get` | O(1) outside a computation. Inside one, O(k) in that computation's dependencies so far, because the edge is recorded once and recording it looks first |
| `set`, `update` | O(1) when the value is unchanged, and otherwise O(d) over what read the cell, transitively through memos |
| `memo(ctx, f)` | O(1) to declare — `f` does not run until something reads it, and then only after a cell it actually read has changed |
| `watch(ctx, f)` | runs once now, and once per batch in which something it read changed |

Tracking is automatic and exact: dependencies are collected afresh on every
run, so a read behind an `if` subscribes to the branch taken and not to the
other one. Writing a value identical to the one already there is not a change
and re-runs nothing.

### Allocators

`core/alloc` — `GeneralPurpose`, `Arena`, `FixedBuffer`. Three implementations
of `Alloc`, importable anywhere, because `Alloc` is the one effect whose
implementation carries no authority: a `Region` is a number, so a library that
builds its own allocator has been granted nothing.

- **`GeneralPurpose`** — unbounded, counts. `gp.stats()` answers
  `Stats { allocations, bytes }`.
- **`FixedBuffer(n)`** — a byte budget, and charging past it **aborts**. That
  is forced rather than chosen: `allocate` answers `Region` and not
  `Result<Region, _>`, so there is no value to report a failure with, and
  [`SPEC.md` §6.10](./cli/src/docs/SPEC.md) says that is what an abort is for.
  The message carries both numbers.
- **`Arena`** — a separate counter, and nothing more than a counter. It does
  not free in bulk, and it says so.

What an allocator is told about is narrower than what the cost model defines,
identically on both backends: **every `allocate(ctx, n)`, and nothing else.**
The charge for an operation is *defined* rather than measured — a `Str` of *n*
UTF-8 bytes charges `16 + n`, a `[T]` of *n* charges `16 + n * stride(T)`, a
view charges nothing — and the list and string rows are charged by definition
and reported to no allocator. The model is written down beside `Alloc` in
`core/effect`, where a reader of the effect meets it.

### What is deliberately not here

- **Struct-of-arrays / `MultiArrayList`.** Not typeable today: exposing "column
  *i* of `T`, at `T`'s *i*-th field type" needs dependent or row types, and
  [`SPEC.md` §5.5](./cli/src/docs/SPEC.md) has no records. Write the two-field
  struct yourself; on the JavaScript backend that is all a library would do.
- **Bulk reclamation — a real `Arena`.** The type is here and its counter is
  real; the bulk free is not. It needs a language feature that bounds a
  context's lifetime, which does not exist yet.
- **Automatic accounting of the list and string rows.** Stated above: the cost
  model defines them and no allocator is told about them.

Why each of those is where it is, and what would have to change, is in
[`design/STANDARD-LIBRARY.md`](./design/STANDARD-LIBRARY.md) — that is a
contributor's document, not a user's.
