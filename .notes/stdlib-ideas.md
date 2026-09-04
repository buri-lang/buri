# Standard library ideas, from a real Buri monorepo

Working notes, not a design document. Each item is something the database
monorepo writes by hand, with a pointer at the evidence. The sketches exist to be
argued with.

The two rules in `cli/src/compiler/standard_library/mod.rs`'s header apply to
anything that comes of this: a
body-less declaration needs a conformance test that calls it, and the cost goes
on the user-facing page.

Nick's decision (do this): remove the `design/STANDARD-LIBRARY.md` in favor of generating the standard library documention on demand from the actual .buri type definitions in the CLI - that way the code is the source of truth and the documentation (in both the CLI and on the website) is always correct with what's implemented in the compiler.

Monorepo paths are relative to `/Users/nick/Documents/GitHub/monorepo-buri`
(177 `.buri` files, read-only).

---

## 1. Doc comments: `//` is written where `///` was meant

The strongest finding, and it is not a library gap.

- The monorepo has **3,076 `//` comments, one `///`, zero `//!`**. The one `///`
  is `libs/greeting/greeting.buri:1` — the file `buri init` generates. Every
  comment the humans and AIs wrote is a `//`.
- **631 sit directly above an `export fn`/`struct`/`enum`/`let`** — doc comments
  in everything but the third slash (`libs/database/model/clock.buri:14`,
  `libs/database/storage_simulator/hex.buri:34`,
  `libs/database/session/expiry.buri:6`, hundreds more).
- The cost is invisible and total. `cli/src/documentation/layout.rs:423`
  recognizes only `//!` and `///`; hover renders "the signature in a fence, then
  the doc comment under it" (`cli/src/language_server/features.rs:96`). So
  `buri docs` and hover show nothing for a whole repository of prose written to
  be read.
- Nothing points the author at the right slash: `SPEC.md:119` mentions `///` in a
  three-line block and never mentions `//!`; the monorepo's own
  `.agent/skills/buri-language/SKILL.md` does not mention comments at all.

Two alternatives:

1. **Make `//` the doc-comment syntax.** One form, attached to the declaration
   that follows; `/* */` for the rest. Nothing is lost — no `//` comment here
   would be _wrong_ to publish — and 631 sites become correct with no edit.
2. **A lint (`doc-comment-slash`).** An exported declaration preceded by a `//`
   block where `///` was meant. Autofixable: one byte per line.

Where a lint fits: a page at `cli/src/docs/reference/lints/<code>.md`, an
`l!(...)` line in `LINTS` (`cli/src/documentation/lints.rs:48`), a check beside
`check_warning_comments` (`cli/src/commands/lint.rs:925`, dispatched at `:816`).
That check already finds comments in the gaps between lexed tokens, so "the
comment block before this declaration's first token" is the same walk, and
`--fix` already carries edits (`cli/src/commands/lint.rs:329`).

Nick's decision (do this): do not add the lint, mention in a skill that `///` is for documentation comments, and look through all the docs to make sure documentation comments are used as appropriate (the AI likely just borrowed from patterns left in the documentation), make sure `//!` isn't anything special (the `!` has no special meaning) in the compiler itself.

---

## 2. Hex

- `bytes.toHex` (`bytes.buri:77`) allocates a two-char `Str` per byte and joins.
  It is the reference implementation and the slow one.
- `char.toDigit(radix)` is the only hex primitive: no `char.isHexDigit`, no
  `char.fromDigit`, no `Int → hex Str`, no `str.toInt` with a radix.
- Rebuilt twice, character for character — same `HEX_DIGITS` table, same
  `nibble`, same 16-call unrolled `hex16`:
  `libs/database/storage_simulator/hex.buri:6-59` and
  `libs/database/database_simulator/prng.buri:6-81`.
- `libs/database/storage_simulator/labels.buri:19-22` re-implements `bytes.toHex`
  from its local `hexByte` + `join`, in a repo that calls the real one at
  `libs/database/session/identity.buri:39`.
- Hex validation is `character.toDigit(16).isSome()`
  (`libs/database/model/request.buri:56`).

```
char.isHexDigit(self): Bool
char.fromDigit(n: Int, radix: Int): Option<Char>
num.toHex<C: Alloc>(ctx, x, width: Int): Str      // zero-padded, lowercase
str.toRadix(text: Str, radix: Int): Option<Int>   // the general gap
```

Nick's decision (do this): please implement this functionality into the standard library. Also, assuming the false positive rate is really low, add lints for this that point developers to use the standard library.

---

## 3. Duration, and where it lives

`Duration` exists (`date.buri:98-102`) and is nearly unusable.

- Getters only, plus free `between`/`plus`. **No constructors** —
  `Duration.seconds(60)` cannot be written — no arithmetic, no `Show`.
- It lives in `core/date` while `Instant` lives in `core/time`. A method belongs
  to its receiver's defining module (SPEC 6.7.3), so `instant + duration` can
  never be a method. **`Duration` probably belongs in `core/time`.**
- `time.since` and `Instant.duration` return `Int` millis (`time.buri:17,22`) —
  the type exists and the clock does not produce it. `sleepMs(ctx, Int)` should
  be `sleep(ctx, Duration)`. Millisecond resolution only; the monorepo's clock
  domain is nanoseconds.

Built instead:

- `NANOS_PER_MILLISECOND` defined twice (`libs/database/model/clock.buri:7`,
  `libs/database/session/expiry.buri:5`); `core/date` exports `MILLIS_PER_*` and
  no nanos. `millis * 1_000_000` written inline in six more places
  (`tenant/tenant.buri:12`, `database_simulator/sim.buri:151`,
  `storage_simulator/labels.buri:46`, `commit/test/clock.buri:97`,
  `commit/test/durability.buri:94`, `session/test/connect.buri:314`).
- Unit-suffixed `I64` constants as the deadline vocabulary:
  `IDLE_TENANT_TIMEOUT_MILLIS` (`tenant/config.buri:19`),
  `DETACHED_SESSION_TTL_MILLIS` (`server/config.buri:20`, again at
  `database_simulator/sim.buri:40`).
- The same overflow-guarded deadline check three times — `checkedMul`,
  `checkedSub`, fall back to the sign: `session/expiry.buri:14`,
  `model/clock.buri:91`, `tenant/tenant.buri:110`.

Wants: nanosecond `Duration` with `seconds/millis/micros/nanos/minutes/hours`
constructors, `add/sub/mul/abs/negate`, saturating comparison so the overflow
dance disappears, `Instant.hasPassed(deadline)`, and `Show` ("1.5s", "300ms").

Nick's decision (do this): implement the wants listed above for the `Duration`. Also, assuming the false positive rate is low, add lints that stear developers to using the standard library for this.

**No timers, no retry.** `tenant/manager.buri:252`: _"Buri has no background
task, so the server root runs it at the start of every event it handles"_ (same
at `server/root.buri:164`). A grep for `retry`/`backoff` finds nothing but a test
title — worth naming as an absence in a repo that does fault injection.

---

## 4. Randomness

Today: `effect Rand { nextInt(lo, hi), nextFloat() }` (`effect.buri:142`),
`core/random` as a two-function wrapper (`random.buri:11,16`), `HostRand`
(`host.buri:96`), `rand().seed(Int)` on the test platform
(`host_testing.buri:936-941`).

- **No pure seeded generator.** Every draw needs a context, so a deterministic
  simulator cannot use `core/random` at all. Both simulators ship their own
  splitmix64, and they differ: `storage_simulator/prng.buri:4` (salted) vs
  `database_simulator/prng.buri:29` (counter-threaded).
- **No splitting**, so streams are decorrelated by hand — **14 salt constants** at
  `storage_simulator/workload.buri:21-45`.
- **No `bytes(n)`**: `randomBytes` is `list.range` +
  `random.int(0,256).wrapToU8()` per byte (`session/identity.buri:48`).
- **No derived distributions.** `bool`, `oneOf`, `weighted`, `chance(ppm)` are all
  open-coded: a percentage ladder at `storage_simulator/workload.buri:161-178`,
  PPM fault sampling at `:450`, `% 2`/`% 3` branch choices at
  `database_simulator/input.buri:68-96`.
- **Modulo bias** in the bounded draw (`storage_simulator/prng.buri:14`).
- **No CSPRNG, and it is load-bearing**: `session/identity.buri:24` cites
  `bugs/rand-not-csprng.md` — resume bearer tokens are "only as unguessable as
  the host's generator happens to be".
- `random.int(lo, hi)` **aborts on an empty range** (`random.buri:9`).

The principle to hold: **an RNG either takes a seed or takes a context.**

```
export struct Gen(U64);                      // pure, seeded, splittable
export fn seeded(seed: Int): Gen;
impl Gen {
    export fn nextInt(self, lo, hi): (Int, Gen);   // rejection-sampled
    export fn nextFloat(self): (Float, Gen);
    export fn nextBool(self): (Bool, Gen);
    export fn nextBytes<C: Alloc>(self, ctx, n): ([U8], Gen);
    export fn split(self): (Gen, Gen);
}
export fn gen<C: Rand>(ctx: C): Gen;         // draw a seed, then go pure
export fn bytes<C: Alloc + Rand>(ctx, n): [U8];
```

Nick's decision (do this): implement the above interface, including for any new crypto standard library functions that could be seeded.

Plus either a `Csprng` effect or an explicit promise on `Rand` that it is not one
— today the docs promise neither way, which is what the bug is about.

Adjacent `core/bits` gap: `rotateLeft`/`rotateRight` exist for `Int` only, so
`rotateLeft64` is hand-written twice out of `shlU64 | shrU64`
(`database_simulator/prng.buri:38`, `storage_simulator/workload.buri:454`). Add
the `U8/U32/U64` rotate variants the shifts already have.

---

## 5. Ordering and comparators — the largest duplication cluster

`core/order` has `Order`, `then`, `flip`, `reverse`, five primitive comparators.
Past that everything is hand-written, repeatedly.

- **Lexicographic list comparison, five copies** — the same index-recursive
  `match ((left[at], right[at]))` at `storage/valuekey.buri:88,74`,
  `query/values.buri:219,234`, `storage_simulator/model.buri:239,253`. →
  `list.compareBy(a, b, cmp)`, and `Ord for [T]`.
- **Comparator combinators.** `compareKeys`/`compareRows`
  (`query/aggregate.buri:165,424`), `compareOrderBy` (`query/project.buri:61`),
  `impl Ord for PosEntry` (`storage/fields.buri:28`) are all "by this key, then
  that one". → `order.by(f)`, `order.chain([cmp])`, `order.reverseIf(Bool)` —
  the last is `directed` (`query/project.buri:85`), inlined again at
  `aggregate.buri:437`.
- **Total float order (NaN last), three times, differently**:
  `storage/valuekey.buri:145`, `query/values.buri:211`, and
  `storage_simulator/model.buri:278` — where NaN silently answers `.Equal`, a
  latent bug in a file that does generate `.Float64` values. → `num.compareTotal`.
- **UTF-16 vs scalar string order written out by hand**, surrogate arithmetic
  included (`query/values.buri:65,90,107`). The comment at `:97` is the request:
  _"`Str.compare` orders by UTF-16 code unit and says nowhere that it does."_ →
  document it, and offer `str.compareScalars`.
- Already-present things re-implemented: `order.int`/`order.bool`
  (`storage/valuekey.buri:109,113`, `storage_simulator/model.buri:270-280`),
  `Order.isLess`/`isEqual` (`query/bindings.buri:226-240`), and `Order.then`
  open-coded as `match { .Equal => next, ordered => ordered }`
  (`storage_simulator/model.buri:174,205`, `workload.buri:359`).

Recommendation: the combinators go in `core/order`; the list comparison goes in
`core/list`, because a method on `[T]` may only be declared in `[T]`'s defining
module (SPEC 6.7.3); the NaN-last comparator goes beside `order.float` rather
than in `core/num`, so every comparator has one address.

```buri
// core/order
export fn by<T, K>(key: fn(T) => K, cmp: fn(K, K) => Order): fn(T, T) => Order;
export fn chain<T>(cmps: [fn(T, T) => Order]): fn(T, T) => Order;
export fn reverseIf<T>(descending: Bool, cmp: fn(T, T) => Order): fn(T, T) => Order;
export fn totalFloat(a: Float, b: Float): Order;   // -0.0 < 0.0, NaN last, never .Equal by accident
// core/list
impl<T> [T] { export fn compareBy(self, other: [T], cmp: fn(T, T) => Order): Order; }
impl<T: Ord> Ord for [T] { export fn compare(self, other: [T]): Order; }
```

All pure — a comparator captures a function value, which SPEC 10.6 permits and
`order.reverse` already does. The UTF-16 bullet is **covered by the
Unicode-scalar ordering change already landed**: `Str.compare` is scalar order on
both backends, so do not add `str.compareScalars` — it would be a second name
for `compare`. The residual there is one sentence on `str.compare` (already in
`core/order`'s `str` doc comment) and deleting the monorepo's surrogate
arithmetic. `Ord for [T]` is additive, and is also the library-level answer to
`derive-ord-array-native-backend` (§13) — worth checking against its five sites
before that bug is fixed at the backend.

Nick's decision (do this): do the recommendation above

---

## 6. List and map operations that are missing

- **`traverse`/`sequence` — 13 hand-written folds.** The helper exists once,
  private to one package: `validateEach` + `pushValidated`
  (`wire/common.buri:11,23`). Everyone else re-inlines it
  (`query/project.buri:97,110`, `query/aggregate.buri:33,78,109,187,287,306,402`,
  `query/bindings.buri:167`, `query/engine.buri:51`, `wire/write.buri:40`). →
  `list.mapResult`, `list.filterResult`, and the `Option` pair. The
  highest-count missing combinator in the survey.
- **Duplicate detection: five algorithms, four packages.** `hasDuplicate` +
  `Duplicates` (`storage/storage.buri:289-304`) and `hasDuplicateColumns` +
  `Adjacent` (`query/plan.buri:132-149`) are the same sort-then-fold at different
  element types; `distinctTargets` (`wire/write.buri:36`) folds into an `OrdSet`;
  `wire/query.buri:213` compares set length to list length; `pushUnique` is O(n²)
  (`subscription/registry.buri:263`). → `list.unique`, `uniqueBy`, `hasDuplicates`.
- **`groupBy`, twice** — five declarations at `query/aggregate.buri:59-154`,
  three more at `subscription/registry.buri:169-259`. → `list.groupBy`,
  `indexBy`, `chunkBy`.
- **`maxBy`/`minBy`, four sites**: `query/aggregate.buri:251,261`,
  `database_simulator/sim.buri:264`, `server/response.buri:167`,
  `commit/clock.buri:65`. `core/list` has `maximum`/`minimum` on `Ord` only.
- `removeAt` as two slices + concat (`storage_simulator/state.buri:142`);
  `generate(n, fn(i))` as explicit recursion with a position counter
  (`storage_simulator/workload.buri:366-397`); `windows(2)`/`isSortedBy` as index
  arithmetic (`:333-348`); `filterMap` faked with a 0-or-1 list and `flatten`
  (`query/plan.buri:169,176`); `sumChecked` (`query/aggregate.buri:340`); a
  clamped window — `paginate` is `drop` + `take` + `num.max(0,_)`
  (`query/page.buri:20`).
- **`ordmap.update`/`alter`** — nested insert/remove with empty-inner pruning by
  hand (`storage/fields.buri:220,238`), get-then-insert at
  `session/manager.buri:227,352,382`. **`ordmap.mapValues`** — a sweep over every
  tenant is a fold that re-inserts each entry (`tenant/manager.buri:234-248`).
- **A counting map**: an 11-field struct with eight hand-written increments
  (`storage_simulator/exit.buri:16-138`), including a `flag(Bool): Int` helper at
  `:189` that is `Bool.toInt` (which exists, `bool.buri:27`).
- **`fold` with early exit** — eviction threads an `EvictionStep` accumulator by
  hand (`tenant/manager.buri:255-268`).

Recommendation: ship the `Result`/`Option` traversals into `core/list` first —
13 sites is the highest count in the survey and everything else here is a
distant second. They allocate, so they name `Alloc` and sit beside `map`:

```buri
impl<A> [A] {
    export fn mapResult<B, E, C: Alloc>(self, ctx: C, f: fn(A) => Result<B, E>): Result<[B], E>;
    export fn mapOption<B, C: Alloc>(self, ctx: C, f: fn(A) => Option<B>): Option<[B]>;
    export fn filterMap<B, C: Alloc>(self, ctx: C, f: fn(A) => Option<B>): [B];
}
impl<T> [T] {
    export fn removeAt<C: Alloc>(self, ctx: C, index: Int): [T];
    export fn windows<C: Alloc>(self, ctx: C, size: Int): [[T]];
    export fn isSortedBy(self, cmp: fn(T, T) => Order): Bool;
    export fn maxBy(self, cmp: fn(T, T) => Order): Option<T>;   // and minBy
    export fn uniqueBy<C: Alloc>(self, ctx: C, cmp: fn(T, T) => Order): [T];
}
export fn generate<T, C: Alloc>(ctx: C, n: Int, f: fn(Int) => T): [T];
```

`groupBy`/`indexBy` must **not** go in `core/list` — they answer a map, and
`core/list` is the bottom of the dependency order. Declare them as free
functions in the module that owns the answer: `map.groupBy(ctx, xs, key)` and
`ordmap.groupBy(ctx, xs, key)`, matching `map.of`/`ordmap.of`'s existing
free-function shape. `ordmap.alter(ctx, key, fn(Option<V>) => Option<V>)` and
`ordmap.mapValues` belong in `core/ordmap` and subsume the nested-insert dance;
`alter` with `.None` returned is the empty-inner pruning written once. The
counting map is `alter` plus `Bool.toInt` and needs no new API. **Defer
early-exit `fold`**: `foldResult` already exists (`list.buri:47`) and an early
exit is `.Err` carrying the answer — that is a doc-comment example, not a
function.

Nick's decision (do this): do the above recommendation

---

## 7. Bytes: a cursor, a builder, a cheap hash

- **A byte cursor written from scratch**: `struct Cursor` + `cursor` + `isDone` +
  `takeByte`/`takeVarint`/`takeSlice`
  (`storage/codec.buri:52,58,64,465,472,479`). `core/bytes` and `core/proto` both
  offer `readX(b, at) -> (value, next)`; the cursor is what everybody builds on
  top. → `bytes.Reader`.
- **Building is `[a, b, c].flatten(ctx)`**
  (`storage/codec.buri:152,186,241,282,318,338-353`) — `bytes.toHex` itself has
  the same shape. → a `bytes.Builder`, or at minimum `[U8].extend`.
- Length-prefixed framing by hand (`storage/codec.buri:454,459,240,246`); a
  counted-list read as explicit tail recursion twice, once per element type, each
  commented about avoiding O(n²) copying (`:265,437`).
- **Endian conversion by hand**: `bigEndianOctets` is eight unrolled
  `shrU64().wrapToU8()` (`storage_simulator/workload.buri:458-469`). →
  `bytes.fromU64Be/Le`, `toU64Be/Le`.
- **FNV-1a implemented inline, twice**: `fnv32`/`fnvStep` with a hand-written
  `U32_MASK` (`storage/codec.buri:95,99,28-34`) and a rolling 64-bit variant
  (`storage_simulator/workload.buri:17-19,144-158`). The comment at
  `storage/codec.buri:92` says why: _"a hand-written checksum rather than
  `core/crypto`"_ — sha256 is 32 bytes of frame for a flipped-bit guard. → a
  **`core/hash`** with `fnv1a32/64`, `crc32c`, siphash. The `Hash` trait exists;
  the functions do not.
- Test vectors written one hex byte per line
  (`proto/test/wire_compat.buri:44-51`). `bytes.fromHex` exists; nothing makes it
  comfortable inside an `assert.eq`.

Recommendation: three separable pieces, only two of which are ready.

1. **`bytes.Reader` — do it.** A pure struct over the existing `readX(b, at)`
   pair, since `[U8]` slices are views and cost nothing:

```buri
export struct Reader { export bytes: [U8], export at: Int }
export fn reader(b: [U8]): Reader;
impl Reader {
    export fn isDone(self): Bool;
    export fn takeByte(self): Result<(U8, Reader), DecodeError>;
    export fn takeSlice(self, n: Int): Result<([U8], Reader), DecodeError>;
    export fn takeVarint(self): Result<(Int, Reader), DecodeError>;
    export fn takeFramed(self): Result<([U8], Reader), DecodeError>;  // length-prefixed
}
export fn fromU64Be<C: Alloc>(ctx: C, x: U64): [U8];   // and Le, U32, and toU64Be/Le
```

2. **`core/hash` — do it, as its own module.** `fnv1a32`, `fnv1a64`, `crc32c`,
   `siphash24`, pure over `[U8]`. It is not `core/crypto`, and the module doc
   should say so in the same voice `crypto.buri` already uses about what it
   refuses: these are checksums, not digests. It also gives the existing `Hash`
   trait something to be implemented in terms of.
3. **`bytes.Builder` — defer.** Values are immutable, so a builder has no
   amortized append to offer over `[[U8]].flatten`, and the honest version needs
   either a mutable region or a rope. `flatten` is already the answer; revisit
   after the native backend has a growable block.

Nick's decision (do this): do the above recommendation

Hex test vectors are **covered by the hex work in flight** — `bytes.fromHex`
plus `assert.eq` over the decoded bytes is the readable form once `fromHex` is
comfortable; nothing more needed.

---

## 8. Strings, formatting, and `Show`

- **`${...}` holes accept only `Int/Float/Bool/Char/Str`** (SPEC 3.6: _"no
  user-extensible display mechanism in v0.3"_). Consequence: **251 of the 255
  interpolation holes in the monorepo are a bare identifier**, because every
  non-primitive is `let`-bound first — six `let`s before one format at
  `database_simulator/input.buri:131-141`. And ~25 hand-written `*Label`/`*Line`
  functions exist that are `Show` under another name
  (`libs/database/*/labels.buri`), including **two independent `valueLabel`s for
  the same `Value` type** (`storage_simulator/labels.buri:7` vs
  `database_simulator/labels.buri:18`). `.show(ctx)` is called twice in the whole
  repo. → letting a `T: Show` into a hole is the biggest ergonomic win here.
  Nick's decision (do this): do the above recommendation to use `T: Show` for the holes
- **No text/report builder.** Reports are `[Str]` then `.join(ctx, "\n")`, with a
  private `bullet()` defined separately in each simulator
  (`storage_simulator/report.buri:83`, `database_simulator/report.buri:90`) and
  headers duplicated field-for-field (`:47-66` vs `:52-71`). → `core/text` with
  `bulletList`, `section`, `indent`.
- **Column alignment is literal `\t` in format strings**
  (`storage_simulator/violation.buri:190-198`, `runner.buri:231-234`) —
  `padStart`/`padEnd` exist and are never used in this repo.
- **A `key=value` log line split across two functions because the format string
  got too long** (`storage_simulator/exit.buri:111-137`). → a logfmt renderer;
  there is no `core/log` at all.
- **No path joining**: `str.format(ctx, "${dir}/storage.header")` × 4
  (`storage/durable.buri:40-43`). → `core/path`, or at minimum `fs.join`.

Recommendation: the `Show`-in-holes item is a **language change, not a library
addition** — amend SPEC 3.6. Today a hole must be `Int/Float/Bool/Char/Str` and
"constructing a `Template` allocates nothing", which is what lets
`io.println(ctx, "hi ${name}")` name only `Stdout`. A `T: Show` hole breaks that
property, because `show` names `Alloc`. The mechanism that keeps it honest: admit
`T: Show` holes **only in argument position of a call whose context argument is
in scope**, and lower `${x}` to `x.show(ctx)` against that same `ctx` before the
`Template` is built. The consequence to state in the amendment rather than
discover later: such a call's context must satisfy `Alloc`, so
`io.println(ctx, "${point}")` needs `Stdout + Alloc` where the bare-identifier
form needs only `Stdout`. That is the real cost, it is visible in the signature,
and it is what buys ~25 hand-written `*Label` functions back.

Nick's decision (do this): You can use `T: Show` for the holes, but if `ctx` is required the user must manually do the conversion

**Done, and here is where the caveat put the line.** `Show.show` is
`fn show<C: Alloc>(self, ctx: C): Str` and there is no second signature
(`core/order.buri:35`), so read literally "if `ctx` is required" would exclude
every `Show` there is. But `ctx` is only *required at the call*: at a **derived**
impl `middle::monomorphize` drops it — "`show` and `toJson` each take a context
they do not use here: rendering, and building the tree, are the runtime's"
(`monomorphize.rs:1529`) — and rewrites the call to `structuralShow(value,
descriptor)`. A hand-written `impl Show` is the case where the context really is
required, because the function has to be entered.

So the rule shipped is: **a hole holds a primitive, or a value whose `Show` is
derived** — a `derive Show` type, an array of one, a tuple of them, all the way
down. It renders through the same `structuralShow` a derived `x.show(ctx)`
renders through, so `"${p}"` and `"${p.show(ctx)}"` are the same text and
`io.println(ctx, "${point}")` still needs only `Stdout`. The Recommendation's
`Stdout + Alloc` cost is not paid, and the "only in argument position of a call
whose context is in scope" mechanism is not needed: no context is threaded
anywhere. A hand-written `impl Show`, and a bounded `T: Show` (whose
instantiation decides which of the two it is), stay the author's `.show(ctx)`.

The `T: Show` bound is deliberately *not* the admission test. A derived
rendering is a fold over the components, so a `Pair<Int, Suit>` whose `Suit` has
a hand-written `Show` would be rendered structurally and disagree with `Suit`'s
own `show` — the same trap `is_derive_only` exists for on `ToJson`
(`types.rs:422`). Conformance is not enough; being *structural* is.

Measured against the ~25 `*Label` functions, honestly: **none of them
disappear.** Every one picks a format the structural rendering does not produce
— `valueLabel`'s `entity:`/`string:` prefixes (`storage_simulator/labels.buri:7`),
`hlcLabel`'s `${millis}:${counter}:${node}` (`:43`), `scanLabel`'s
`kind=scan path=osp object=…` (`:80`), and `writeStatusLabel`'s `"Applied"`
where a derive writes `".Applied"` (`:110`). What they become is a hand-written
`impl Show` on the type in `model`, which is the win that was actually
available: it kills the duplication the section found — **two independent
`valueLabel`s for one `Value`** (`storage_simulator/labels.buri:7` vs
`database_simulator/labels.buri:18`) collapse to one impl the type carries — and
turns each of the 251 hole sites into `${v.show(ctx)}` rather than a `let` plus
`${valueLabel(ctx, v)}`. Shrunk, not vanished, which is what the caveat asked
for.

The larger win is the one the `*Label` count does not show: the monorepo has
**127 `derive … Show` declarations** and calls `.show(ctx)` twice. Every one of
those 127 types can now go straight into a hole.

The rest of §8, ranked:

- **`core/path` — do it, and not `fs.join`.** Path manipulation is pure string
  work; every function in `core/fs` names `Fs` in its bounds and its module doc
  says the disk is visible in the signature. Putting a pure `join` there would be
  the first exception. `core/path` with `join<C: Alloc>(ctx, parts: [Str]): Str`,
  and pure `dirname`/`basename`/`extension` returning views.
- **`core/text` and a logfmt `core/log` — defer.** Two simulators and one log
  line is thin evidence, and `padStart`/`padEnd` already exist and were never
  reached for, which makes this look like a discovery problem (§13) before it is
  an API one. Revisit after the generated stdlib docs land.

Nick's decision (do this): Also defer the core/text and core/log suggestions. But for the path, I'd suggest creating a new Path type that has all the methods. That way any Path is validly formatted. All IO file operations should require a Path and not a Str.

---

## 9. CLI argument parsing

Both simulators hand-roll a near-identical parser:
`storage_simulator/args.buri:125-198` (`parseFrom`, `takesValue`, `applyValued`,
`valueAt`, `nonNegative`, `positive`) and the thinner
`database_simulator/args.buri:58-105`. Each ships an args struct, an error enum,
a defaults constructor, a parser, and a hand-written error-message table
(`:96-108` / `:49-56`).

Missing: `--flag=value`, short flags, a `--` terminator, `--help`. The last `else`
of `applyValued` silently means `--dir` (`storage_simulator/args.buri:154-177`,
with a comment there because the control flow is not self-evident). The inverse —
rendering args back to a replay command line, omitting defaults — is also
hand-written (`:80-123`), as is the env-var fallback merged after parsing
(`storage_simulator/cli.buri:58-63`).

→ `core/cli`: a declarative flag spec yielding parse + `--help` + canonical
re-render + typed errors. The largest single "utility module that is really
stdlib material" in the repo.

Recommendation: build `core/cli`, and keep it at the **`Alloc` tier** — it parses
a `[Str]` and answers a value, so it must not name `Env` or `Proc`. That is what
makes it testable without a host, and it keeps the env-var fallback where it
belongs: the caller merges `env.get` results itself, which is the one place the
precedence rule is a decision rather than a default.

```buri
export enum Arity { Switch, Value(Str) }               // the Str names the value in --help
export struct Flag { export long: Str, export short: Option<Char>,
                     export arity: Arity, export help: Str }
export struct Parsed { export values: Map<Str, Str>, export switches: Set<Str>,
                       export positional: [Str] }
export enum ParseError { UnknownFlag(Str), MissingValue(Str), UnexpectedValue(Str) }
derive Eq, Show for ParseError;
export fn parse<C: Alloc>(ctx: C, spec: [Flag], argv: [Str]): Result<Parsed, ParseError>;
export fn help<C: Alloc>(ctx: C, spec: [Flag], program: Str): Str;
export fn render<C: Alloc>(ctx: C, spec: [Flag], parsed: Parsed): [Str];  // canonical replay
```

`parse` handles `--flag=value`, short flags, `--` and `--help` — the four the
hand-rolled parsers are missing — and `derive Show for ParseError` deletes both
hand-written message tables. `render` is the replay line, and it is worth
shipping in the same wave: it is the reason this is a spec and not a loop, since
one `[Flag]` drives parse, help, and re-render and they cannot drift. No
`derive`-generated struct of typed fields — `derive` only attaches conformance
(`design/non-goals.md`) — so accessors return `Option<Str>` and the
caller uses `str.toInt`.

Nick's decision (do this): the standard library should provide two utilities - one to get the CLI args unparsed as a `[Str]`, but it should also provide an opinionated way to create a CLI with parsed args and also implement things like `--help`, `--version`, and other common CLI args. You should provide immediately below some code snippet examples of the interface and the usage of this opinionated CLI parsing + command firing thing.

**The raw half already ships.** `effect Env` declares `args(self): [Str]`
(`effect.buri:166`), `core/env` is the two-line wrapper over it, and both hosts
already drop the program's own name (`cli/runtime/host.rs`'s `skip(1)`,
`js/runtime.js`'s `process.argv.slice(2)`). The unparsed utility therefore needs
no addition — it needs pointing at:

```buri
from "core/env" import * as env;
from "core/host" import * as host;
from "core/io" import * as io;

export fn main(): Result<(), Str> {
    let ctx = context { Alloc: host.alloc, Env: host.env, Stdout: host.stdout };
    let argv: [Str] = env.args(ctx);          // no program name, no parsing
    let _ = io.println(ctx, "${argv.len()} arguments").ignore();
    .Ok(())
}
```

One consequence lands on the opinionated half: **there is no `argv[0]`**, so
`--help` has to be _told_ what to call the program. That is a field on the spec
below, not something a parser can recover.

### Proposed `core/cli` — an interface sketch, not shipped code

One `Cli` value still drives parse, `--help`, error rendering and the replay
line, so they cannot drift — the recommendation above, kept. What is new is that
the spec also carries _what to run_: a `Command` holds its handler the way
`server.Server` holds `onRequest`, so firing is a library call rather than the
caller's `match` over a command name.

```buri
// ---- the spec ------------------------------------------------------------

export enum Arity { Switch, Value(Str), Many(Str) }   // the Str names it in --help

export struct Flag {
    export long: Str,
    export short: Option<Char>,
    export arity: Arity,
    export help: Str,
}

export struct Arg { export name: Str, export help: Str, export required: Bool }

/// One subcommand and the function that fires when it is chosen. `C` is the
/// caller's own context, exactly as `server.Server`'s `onRequest`: a handler
/// may do anything the context it was written beside can do — print, read a
/// file, `proc.exit` — and `core/cli` never learns what that is.
export struct Command<C> {
    export name: Str,
    export about: Str,
    export run: fn(C, Parsed) => Result<(), Str>,
    export flags: Option<[Flag]>,
// Nick's decision (do this): name this `arguments` instead of `args`
    export args: Option<[Arg]>,
}

export struct Cli<C> {
    export name: Str,      // there is no argv[0]; --help is told the name
    export version: Str,   // what --version prints
    export about: Str,
    export commands: [Command<C>],
    export flags: Option<[Flag]>,   // global, accepted before the subcommand
}

// ---- what a parse answers ------------------------------------------------

// Nick's decision (do this): name this `Arguments` rather than `Parsed`
export struct Parsed {
    export values: Map<Str, Str>,
    export switches: Set<Str>,
    export positional: [Str],
}

impl Parsed {
    export fn on(self, long: Str): Bool;             // a switch was given
    export fn value(self, long: Str): Option<Str>;   // caller uses str.toInt
    export fn arg(self, name: Str): Option<Str>;     // a declared positional
    export fn many<C: Alloc>(self, ctx: C, long: Str): [Str];
}

export enum ParseError {
    UnknownFlag(Str), MissingValue(Str), UnexpectedValue(Str),
    UnknownCommand(Str), NoCommand, MissingArg(Str),
}
derive Eq, Show for ParseError;

/// `--help` and `--version` are handled here and are not errors: the parse
/// answers the *text* and does not print it, which keeps every function below
/// at the `Alloc` tier and testable with no host.
export enum Selection<C> {
    Fire { command: Command<C>, parsed: Parsed },
    Print(Str),
}

export fn parse<C: Alloc, X>(
    ctx: C, spec: Cli<X>, argv: [Str],
): Result<Selection<X>, ParseError>;
export fn help<C: Alloc, X>(ctx: C, spec: Cli<X>, command: Option<Str>): Str;
export fn errorText<C: Alloc>(ctx: C, e: ParseError): Str;
export fn render<C: Alloc>(ctx: C, flags: [Flag], parsed: Parsed): [Str];  // replay

// ---- firing --------------------------------------------------------------

/// `fire`, with the arguments read for you. The one call a `main` needs.
export fn run<C: Alloc + Env + Stdout + Stderr>(ctx: C, spec: Cli<C>): Result<(), Str>;

/// Parse `argv` and fire the command it names. `Env` is absent from the bound
/// on purpose: a test hands this an argv and needs no host.
///
/// `.Ok(())` is exit 0. A parse error goes to stderr with the usage under it
/// and comes back as `.Err`, which `main`'s contract turns into exit 1; a
/// command that owns a *particular* status calls `proc.exit` itself.
// Nick's Decision: it seems like `fire` is not necessary given the example below. if so, then do not expose the fire function.
export fn fire<C: Alloc + Stdout + Stderr>(
    ctx: C,
    spec: Cli<C>,
    argv: [Str],
): Result<(), Str> {
    match (parse(ctx, spec, argv)) {
        .Ok(.Print(text)) => {
            let _ = io.println(ctx, "${text}").ignore();
            .Ok(())
        },
        .Ok(.Fire { command, parsed }) => {
            let go = command.run;
            go(ctx, parsed)
        },
        .Err(e) => {
            let message = errorText(ctx, e);
            let _ = io.eprintln(ctx, "${spec.name}: ${message}").ignore();
            let _ = io.eprintln(ctx, "${help(ctx, spec, .None)}").ignore();
            .Err(message)
        },
    }
}
```

Usage — a tool with two subcommands, wired end to end:

```buri
from "core/cli" import * as cli;
from "core/fs" import * as fs;
from "core/host" import * as host;
from "core/io" import * as io;
from "core/str" import * as str;

fn countLines<C: Alloc + Fs + Stdout>(ctx: C, parsed: cli.Parsed): Result<(), Str> {
    let path = parsed.arg("file").withDefault("-");
    let body = fs
        .readText(ctx, path)
        .mapErrCtx(ctx, fn(c, e) => str.format(c, "${path}: ${e}"))?;
    match (parsed.on("quiet")) {
        true => .Ok(()),
        false => {
            let _ = io.println(ctx, "${body.lines(ctx).len()}").ignore();
            .Ok(())
        },
    }
}

fn hashFile<C: Alloc + Fs + Stdout>(ctx: C, parsed: cli.Parsed): Result<(), Str> {
    // ... same shape: read the arg, do the work, print, `.Ok(())`
    .Ok(())
}

// The spec is a value, so it is a function of the context, exactly as
// `core/actor`'s `counter()` is.
fn tool<C: Alloc + Fs + Stdout>(): cli.Cli<C> {
    cli.Cli {
        name: "wc2",
        version: "0.1.0",
        about: "counts and hashes files",
        flags: .Some([
            cli.Flag { long: "quiet", short: .Some('q'), arity: .Switch,
                       help: "print nothing on success" },
        ]),
        commands: [
            cli.Command {
                name: "count",
                about: "count the lines in a file",
                args: .Some([cli.Arg { name: "file", help: "file to read",
                                       required: true }]),
                run: fn(c, parsed) => countLines(c, parsed),
            },
            cli.Command {
                name: "hash",
                about: "print a file's digest",
                args: .Some([cli.Arg { name: "file", help: "file to read",
                                       required: true }]),
                run: fn(c, parsed) => hashFile(c, parsed),
            },
        ],
    }
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc, Env: host.env, Fs: host.fs,
        Stdout: host.stdout, Stderr: host.stderr,
    };
    cli.run(ctx, tool())
}
```

`wc2 --help`, `wc2 count --help` and `wc2 --version` are answered inside `parse`
and exit 0. `wc2 count` with no file is `.MissingArg("file")` — rendered to
stderr with the usage under it, exit 1. `wc2 count README.md` fires
`countLines`, and that handler's own `.Err` is the message `main` exits 1 with.
The env-var fallback stays outside all of this, as argued above: a caller that
wants one merges `env.get` into the value it read from `Parsed`, because the
precedence rule is a decision and not a default.

Nick's decision (do this): the above API works with me, with the comments I provided. If possible, it would be great if the parsed arguments in the run function were actually of the correct struct type, so the run function could be type safe. but if that requires a new language feature (e.g., reflection) let's go with what you recommended above. It seems like, though that many proposed exported functions (like parse, fire, help, render, errorText) aren't necessary (at least by looking at the example it seems like they're not necessary)? Only export necessary functions from this library.


Shipped as `core/cli` (`sources/cli.buri`), and where it diverges from the sketch above:

- **The exported surface is `Arity`, `Flag`, `Arg`, `Command<C>`, `Cli<C>`, `Arguments` and
  `run` — one function.** `parse`, `fire`, `help` and `errorText` are private, and so are the
  types only they would have exposed (`ParseError`, `Selection<C>`). `render` is not written:
  it could only be reached through an export, and there is none to give it. Nothing was kept
  back that a test needs — a spec is tested by handing `core/host/testing`'s
  `env().arguments([...])` to `run` with a captured `Stdout`/`Stderr`, which is the whole
  conformance package (`cli/tests/conformance/lib/cli/test/arguments.buri`, 25 blocks).
- **The typed handler is not possible without a new language feature**, as suspected. `derive`
  only attaches a conformance to a type that already exists, and generics do not reach it
  either: one `Cli` holds *one* `[Command<C>]`, so a per-command argument type would have to be
  the same type in every element — which is exactly what a per-command argument struct is not.
  Existentials plus a macro would do it; reflection would do it. Neither exists, so `Arguments`
  is the dynamic accessor bag the recommendation argued for.
- `Parsed` is `Arguments` and `Command.args` is `Command.arguments`, per the two inline
  decisions above.
- `Arguments`' fields are private and its accessors are five rather than four: `on`, `value`,
  `many`, `arg` and `positionals` (every positional the command was given, which is what a
  variadic command reads). `many` needs no `ctx` — the values are already a list in a private
  `Map<Str, [Str]>`.
- **An empty argument list prints the help and answers `.Ok(())`.** A tool invoked with nothing
  to do is asking what it can do. Global flags and *then* no command is still `NoCommand`,
  which is an error.
- A value taken from the next token is refused when that token begins with `--`, so
  `--out --help` is `MissingValue` rather than an `--out` whose value is `--help`; `-` and `-1`
  are still values. Short flags do not cluster.
- Global flags are accepted only before the subcommand, which is what the sketch's comment on
  `Cli.flags` said; after it, only the command's own.

---

## 10. Filesystem

- **Three copies of a 12-method `Fs` impl**, because there is no null or
  decorating base: `apps/database_server/main.buri:55-106`,
  `database_simulator/nofs.buri:6-57`, `storage_simulator/faults.buri:54-126`
  (the last a fault injector that must restate every method to forward it). →
  `fs.NullFs`, `fs.ReadOnlyFs`, and a forwarding base. The general shape —
  _decorating an effect requires restating it_ — likely applies beyond `Fs`.
- **Atomic write hand-rolled** (temp, `sync`, `rename`, `sync` the directory) with
  five chained `mapErr`s (`storage/durable.buri:75-105`). → `fs.writeAtomic`.
- **"read it, or `.None` if absent"** as an adapter matching `NotFound`
  (`storage/durable.buri:169`). → `fs.readBytesOptional`.
- `Fs` has `makeDir` and no directory removal
  (`storage_simulator/runner.buri:206`).

Recommendation: two library functions, one language proposal, one already done.

- **`fs.writeAtomic` and `fs.readBytesIfExists` — do both, in `core/fs`.** The
  four-call checkpoint sequence is already written out in `fs.buri`'s module
  doc; making it a function is turning documentation into code.
  `writeAtomic<C: Alloc + Fs>(ctx, path: Str, body: [U8]): Result<(), IoError>`
  writes a sibling temp, syncs it, renames, syncs the directory, and reports the
  first failure. Name the optional read `readBytesIfExists` rather than
  `readBytesOptional` (a name has one meaning, SPEC 11.1); it answers
  `Result<Option<[U8]>, IoError>` and the `.None` comes from matching `NotFound`
  on the read, never from a preceding `exists` — the two-call form is a race.
- **Directory removal is covered by `Fs.removeDir` in flight** (`fs.buri` already
  documents the `mkdir -p`/`rmdir` asymmetry). Residual: none — a recursive
  delete stays the caller's walk, deliberately.
- **The forwarding base is a language gap, not an `Fs` gap.** `core/alloc`'s own
  `Scoped<C>` restates sixteen effects to forward them
  (`alloc.buri:547-756`) — the stdlib pays the same tax the fault injector pays.
  The fix is effect delegation (a context that forwards every method of an effect
  it does not override), and it belongs in the language proposals beside scoped
  contexts. Meanwhile add a `NullFs` and a `ReadOnly` wrapper to
  `core/host/testing` beside `TestFs`, which collapses two of the three copies
  without waiting for it.

Nick's decision (do this): we can do the writesAtomic and readBytesIfExists and removeDir suggestions. Instead of the forwarding base, perhaps we should split the effects into readonly fs and writable fs. I don't care about the forwarding base because it seems like the current solution is to be explicit.

#### What "forwarding base" means, and why it is a language gap

The gap is one sentence: **an `impl` block must supply every method the effect
declares, and there is no way to say "the rest come from the field."** So a
wrapper that changes _one_ operation restates the other eleven. A read-only
filesystem — the runtime kind that satisfies `Fs` and refuses writes, not SPEC
10.8's attenuator that stops satisfying `Fs` at all — is the smallest honest
example:

```buri
// TODAY. Twelve methods written out so that six can be refused.
export struct ReadOnly<C>(C);

impl<C: Fs> Fs for ReadOnly<C> {
    // The five that forward, written out because they must be.
    fn readFile(self, path: Str): Result<Str, IoError> {
        self.0.readFile(path)
    }

    fn readFileBytes(self, path: Str): Result<[U8], IoError> {
        self.0.readFileBytes(path)
    }

    fn fileExists(self, path: Str): Bool {
        self.0.fileExists(path)
    }

    fn readDir(self, path: Str): Result<[Str], IoError> {
        self.0.readDir(path)
    }

    fn syncFile(self, path: Str): Result<(), IoError> {
        self.0.syncFile(path)
    }

    // The one this type exists for.
    fn writeFile(self, path: Str, body: Str): Result<(), IoError> {
        .Err(.ReadOnly)
    }

    // ...and writeFileBytes, appendFile, renameFile, removeFile, removeDir and
    // makeDir: six more one-line bodies, each with its whole signature typed
    // out again from `effect Fs`.
}
```

`storage_simulator/faults.buri:54-126` is the same shape with _nothing_ refused
— a fault injector that forwards all twelve and only counts them.

**The stdlib pays the same tax, sixteen times over.** `core/alloc`'s `Scoped<C>`
is a wrapper whose `Alloc` is a scope's arena and whose every other effect is
the inner context's. One block is interesting; the rest is transcription:

```buri
// alloc.buri:547 — "The one implementation that does not forward."
impl<C> Alloc for Scoped<C> {
    fn allocate(self, bytes: Int): Region {
        Region(arenaAllocate(self.1, bytes))
    }
}

// alloc.buri:553 — "Everything below here forwards, and says nothing else."
impl<C: Stdout> Stdout for Scoped<C> {
    fn print(self, text: Template): Result<(), IoError> {
        self.0.print(text)
    }

    fn println(self, text: Template): Result<(), IoError> {
        self.0.println(text)
    }

    fn writeBytes(self, b: [U8]): Result<(), IoError> {
        self.0.writeBytes(b)
    }
}

// ...then Stderr, Stdin, Fs, Net, Clock, Rand, Env, Proc, Tasks, Listen,
// Sockets, Ui and Watch — twelve more blocks, `alloc.buri:555-768`, every body
// `self.0.<the same name>(<the same arguments>)`.
```

The cost is not the typing. It is that **a new method on an existing effect is a
silent hole in every wrapper until a human notices**: `alloc.buri:445` already
says "a new effect in `core/effect` is a new block here", which is a comment
asking somebody to remember.

**Proposal (language, not library): delegation on an `impl`.** The `impl` names
where the unwritten methods come from and supplies only the ones that differ.
SPEC 10.1's "an `impl` supplies every method" is the rule that gains the
exception; SPEC 10.8 is where the motivation lives.

```buri
// PROPOSED SYNTAX — does not exist today.
impl<C: Fs> Fs for ReadOnly<C> via self.0 {
    fn writeFile(self, path: Str, body: Str): Result<(), IoError> { .Err(.ReadOnly) }
    fn writeFileBytes(self, path: Str, b: [U8]): Result<(), IoError> { .Err(.ReadOnly) }
    fn appendFile(self, path: Str, b: [U8]): Result<(), IoError> { .Err(.ReadOnly) }
    fn renameFile(self, source: Str, dest: Str): Result<(), IoError> { .Err(.ReadOnly) }
    fn removeFile(self, path: Str): Result<(), IoError> { .Err(.ReadOnly) }
    fn removeDir(self, path: Str): Result<(), IoError> { .Err(.ReadOnly) }
    fn makeDir(self, path: Str): Result<(), IoError> { .Err(.ReadOnly) }
}

// and sixteen of Scoped's seventeen blocks become one line each:
impl<C: Stdout> Stdout for Scoped<C> via self.0 {}
impl<C: Fs> Fs for Scoped<C> via self.0 {}
impl<C: Env> Env for Scoped<C> via self.0 {}
// ...
```

`via self.0` reads as what it does — every method not written here is
`self.0.<name>(<args>)` — and it needs no new carve-out: the generated bodies
are exactly the ones SPEC 10.2 already permits an implementor to write by hand.
A method added to `effect Fs` then reaches every delegating wrapper for free,
and a wrapper that _must_ see the new method says so by naming it.

**The alternative is a derive** — `derive Fs via 0 for Scoped;` beside the
struct instead of an empty `impl`. Shorter for the pure-forwarding case, but it
cannot express the common one ("forward everything _except_ these seven")
without also growing an override block, at which point it is the proposal above
with different punctuation. Prefer `via` on the `impl`.

---

## 11. Testing

`core/testing/assert` has nine functions. Usage: `eq` 785, `ok` 333, `some` 276,
`isTrue` 118, `err` 88, `none` 38, `notEq` 25, `isFalse` 25, `fail` 9.

The `isTrue` calls say what is missing: **48 wrap `.contains(...)`, 26 wrap
`.isEmpty()`, 25 wrap a comparison operator**, one wraps `.len() == 3`. Each
prints "expected true, got false" on failure — the least useful message a test
can give. Thirteen consecutive `assert.isTrue(x.contains(...))` at
`database_simulator/test/report.buri:32-44`;
`assert.isTrue(drawn >= 0 && drawn < 32)` at
`storage_simulator/test/prng.buri:11`;
`assert.isTrue(found.details.len() == 3)` at
`storage_simulator/test/invariants.buri:98`.

→ `assert.contains`, `isEmpty`/`notEmpty`, `len`, `gt/ge/lt/le`, `approxEq`, and a
diffing `assert.eq` for `[T]` (the failure renderer already walks the type
structurally, so the diff is reachable).

Nick's decision (do this): add the above additions to `assert`

**`assert.fail` returns `()`, not the bottom type** (`assert.buri:43`), while the
private `failExpected` does return bottom (`:89`). So a failing match arm cannot
produce a value and a test must fabricate one:
`storage_simulator/test/runner.buri:20-40` calls `assert.fail(...)` then
`emptySuccess()`, a dummy constructor that exists only to type-check. Same idiom
at `database_simulator/test/runner.buri:52-59,89-92`. → make `fail` diverge, and
add `assert.matches(value, .Variant)` returning the payload.

Recommendation: make `fail` diverge first — it needs **no language change**. SPEC
6.9 says there is no bottom type, but the private `failExpected<T, R>(...): R`
already gets the effect from a free return type variable, so `export fn fail(message: Str): ()`
becomes `export fn fail<R>(message: Str): R` and the dummy `emptySuccess()`
constructors go away. The one thing to check before shipping: today's
`assert.fail("x");` sites stand alone as expression statements (SPEC 11.2.1,
which admits any expression of type `()`), so inference must settle `R` at `()`
in statement position or every existing call site breaks. If it does not, this is
a compat break worth a `failWith` under a second name instead.

Nick's decision (do this): remove `assert.fail` instead of adding a bottom type, and potentially also remove `assert.failExpected`.

Then widen the assertion set — all in `assert.buri`, all `()`-returning, all
taking no `ctx` because rendering is the runner's:

```buri
export fn contains<T: Eq>(haystack: [T], needle: T): ();
export fn isEmpty<T>(xs: [T]): ();          // and notEmpty
export fn len<T>(xs: [T], expected: Int): ();
export fn gt<T: Ord>(actual: T, bound: T): ();   // and ge, lt, le
export fn approxEq(actual: Float, expected: Float, tolerance: Float): ();
```

That covers 99 of the 118 `isTrue` calls with a message that names both values.
The diffing `assert.eq` for `[T]` is a change to the **runner's failure
renderer**, not to `assert.buri` — the renderer already walks the type
structurally, so it is where the diff is reachable. **Defer
`assert.matches(value, .Variant)`**: a pattern is not a value and Buri has no
macro to take one, so it is a language question; `ok`/`err`/`some` already cover
the shapes that recur.

---

## 12. Numerics, small gaps

- **Floor division and Euclidean remainder by hand**: `PhysicalTime.parts`
  (`model/clock.buri:75`) and `core/date` itself (`date.buri:183,269-273`). →
  `num.divFloor`, `num.remEuclid`.
- `Saturating` is used **once** in the whole monorepo
  (`subscription/registry.buri`), while `checkedSub`-then-decide-by-sign appears
  three times as a saturating subtract in disguise (§3).
- Masking the sign bit to make a positive `I64` from a `U64` draw
  (`database_simulator/input.buri:236-251`).
- `math.INFINITY` is spelled `1.0e400` (`math.buri:54`) — worth a look.

Recommendation: two small additions to `core/num` and one correction to
`core/math`.

```buri
// core/num, beside min/max/clamp — Int-only, pure, aborting on a zero divisor
// exactly as `/` does (SPEC 6.9), so no Option in the signature.
export fn divFloor(a: Int, b: Int): Int;
export fn remEuclid(a: Int, b: Int): Int;
```

Both have three callers each including `core/date` itself (`date.buri:183,269`),
so the first user of `divFloor` is the standard library. **Fix `math.INFINITY`**:
`1.0e400` is a literal that relies on overflow-to-infinity at parse time, which
is a property of whichever float parser ran, not a stated one. Make it a
body-less `export let INFINITY: Float;` backed by an intrinsic, add `NAN` and
`NEG_INFINITY` beside it, and pin all three in a conformance test that both
backends run — `cli/src/compiler/standard_library/mod.rs`'s rules require the test anyway, and the
value being identical on JS and native is the whole point. **Defer** the
saturating-subtract and sign-bit items: `Saturating` and `Checked` already exist
and were not found, which puts them in §13 rather than here.

Nick's decision (do this): do the above recommendation

---

## 13. Not stdlib material — for the maintainer

**Compiler bugs worked around**, all tracked as `bugs/*.md` in that repo. Listed
because the _shape_ of the workaround is the cost.

- `derive-ord-array-native-backend` (5 sites) — the native backend cannot compile
  a derived comparison over an array. Forces the `ValueKey` newtype to exist at
  all (`storage/valuekey.buri:5`), a hand-written `impl Ord for PosEntry`
  (`storage/fields.buri:18`), 80 lines of comparators
  (`storage_simulator/model.buri:204-280`), and a hex-key indirection so an
  `OrdMap` can be keyed by a token (`session/manager.buri:41`,
  `session/identity.buri:34`).
- `native-backend-host-fs-and-env` (5, two in `BUILD.buri` files) — two apps ship
  `outputs: [{ platform: JS }]` only.
- `rand-not-csprng` (3) — §4. `js-proc-exit-drops-buffered-output` (3) plus
  `js-main-err-prints-nothing`: a report survives a pipe and a terminal but not
  `> report.txt`, and returning `.Err` instead prints nothing at all.
- `let-bound-projection-off-a-generic-call-aborts-natively` (3, all tests) —
  reading one field out of `assert.some(xs[0])` aborts the native test binary.
- `str-compare-orders-by-utf16-code-unit` (2) — §5.
  `float-equality-in-a-condition-is-ieee` (2) — forces
  `!(v < 0.0 || v > 0.0 || v == 0.0)` as the NaN test (`storage/valuekey.buri:161`).
  `coalesce-option-of-array-native-backend` (2) — `??` over an `Option` holding
  heap contents aborts natively, so it is the `match` the operator would have been.
- **Two silent-wrong-answer bugs, the category to fix first.**
  `js-tuple-destructuring-calls-twice`: the JS backend evaluates a call once per
  element of a destructured tuple, so `let (m, out) = connect(...)` mints two
  sessions — an entire `struct Step<T>` exists to avoid tuples
  (`session/manager.buri:52,59`). `list-map-swallows-result-element`: a narrowing
  `toU8` inside `map` answers a `Result` that `map` drops with no diagnostic,
  leaving a list of zeros (`session/identity.buri:44`).
- Also: `match-arm-bindings-clobbered-by-a-sibling-field-read`
  (`server/root.buri:302`), `mutual-tail-recursion-native-backend`,
  `option-of-decoded-struct-field-abort` (a field must be recomputed by re-walking
  every patch, `storage/fields.buri:86`), `storage-simulator-native-match-abort`
  (buri#41 — a whole suite pinned to `platforms: [JS]`),
  `fs-cannot-remove-a-directory`.

**Misuse / discoverability — the library has the answer and nobody found it:**

- `Result.ignore()` used **once**; the four-line
  `match (io.println(...)) { .Ok(_) => (), .Err(_) => () }` written six times
  (`database_simulator/cli.buri:96,103`, `storage_simulator/cli.buri:98,105`,
  `storage_simulator/runner.buri:219,272`), as duplicated `printLine`/`logLine`
  helpers in both simulators.
- `order.int`/`order.bool`, `Order.then`, `Order.isLess`, `Bool.toInt`,
  `padStart`/`padEnd`, `Saturating`, `.show(ctx)`, `context Fixture {}` — all
  present, all re-implemented or unused. A discovery problem as much as an API
  one, and it is what makes §1 matter: a library nobody can hover over is a
  library nobody finds.
- Test fixtures are copy-pasted rather than shared:
  `at(millis)`/`clock(millis)`/`commitTime` in at least six files
  (`commit/test/clock.buri:97`, `commit/test/durability.buri:94`,
  `commit/test/committer.buri:340`, `tenant/test/operations.buri:458`,
  `tenant/test/residency.buri:317`, `database_simulator/test/checks.buri:22`);
  `identity`/`entity`/`alice`/`bob` across four files in `storage/test/` alone.
  `query` solved it with a `testing/lib.buri` and nobody copied the pattern.
  `let ctx = context { Alloc: alloc() };` is written ~20 times where
  `context Fixture { ... }` (`database_simulator/test/runner.buri:21-25`) does it
  once — used in exactly one file.
- A 14-line doc comment appears verbatim twice in a row at
  `storage/valuekey.buri:117-130` and `:131-144`.

**Both simulators duplicate whole modules**, not just helpers: `prng.buri`,
`hex.buri`, `labels.buri`, `report.buri`, `args.buri`, `cli.buri`,
`violation.buri` exist in both with substantially the same content. Some is
stdlib material (§2, §4, §8, §9); the rest is a missing shared library there.

Recommendation: nothing in this section is a library addition, and most of it is
already moving.

- **Fix the two silent-wrong-answer bugs first.**
  `js-tuple-destructuring-calls-twice` and `list-map-swallows-result-element` are
  the only entries here that produce a wrong answer with no diagnostic; every
  other bug aborts or fails to compile, which is a cost but not a lie. A whole
  `struct Step<T>` exists to route around the first.
- **Already covered:** `str-compare-orders-by-utf16-code-unit` by the landed
  scalar-order change; `coalesce-option-of-array-native-backend` by the removal
  of `??` in favour of `withDefault`; `native-backend-host-fs-and-env` and the
  two `platforms: [JS]` pins by the native `host.fs`/`env` work in flight;
  `fs-cannot-remove-a-directory` by `Fs.removeDir`; `rand-not-csprng` by the
  Entropy/CSPRNG surface. `derive-ord-array-native-backend` gets a library-level
  answer from §5's `Ord for [T]` while the backend fix waits.
- **The discoverability half is covered by the generated stdlib docs and the
  doc-comment audit already queued** — a library nobody can hover over is a
  library nobody finds, and that is the whole diagnosis. Residual, and worth
  adding to that wave rather than a later one: put a worked `context Fixture { }`
  example on the SPEC 11.3 / testing page, since it is present, solves the
  copy-pasted-fixture problem, and was used in exactly one file.
- **The duplicated modules are the monorepo's problem, not the library's.** Once
  §2, §4, §8 and §9 land, what is left of `prng.buri`/`hex.buri`/`args.buri` is
  a shared library that repo should have and does not — worth saying back to
  them, not worth a `core/*` module.
