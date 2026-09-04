# Standard library ideas, from a real Buri monorepo

Working notes, not a design document. Each item is something the database
monorepo writes by hand, with a pointer at the evidence. The sketches exist to be
argued with.

`design/STANDARD-LIBRARY.md` §5 applies to anything that comes of this: a
body-less declaration needs a conformance test that calls it, and the cost goes
on the user-facing page.

Monorepo paths are relative to `/Users/nick/Documents/GitHub/monorepo-buri`
(177 `.buri` files, read-only).

---

## 1. Doc comments: `//` is written where `///` was meant

The strongest finding, and it is not a library gap.

* The monorepo has **3,076 `//` comments, one `///`, zero `//!`**. The one `///`
  is `libs/greeting/greeting.buri:1` — the file `buri init` generates. Every
  comment the humans and AIs wrote is a `//`.
* **631 sit directly above an `export fn`/`struct`/`enum`/`let`** — doc comments
  in everything but the third slash (`libs/database/model/clock.buri:14`,
  `libs/database/storage_simulator/hex.buri:34`,
  `libs/database/session/expiry.buri:6`, hundreds more).
* The cost is invisible and total. `cli/src/documentation/layout.rs:423`
  recognizes only `//!` and `///`; hover renders "the signature in a fence, then
  the doc comment under it" (`cli/src/language_server/features.rs:96`). So
  `buri docs` and hover show nothing for a whole repository of prose written to
  be read.
* Nothing points the author at the right slash: `SPEC.md:119` mentions `///` in a
  three-line block and never mentions `//!`; the monorepo's own
  `.agent/skills/buri-language/SKILL.md` does not mention comments at all.

Two alternatives:

1. **Make `//` the doc-comment syntax.** One form, attached to the declaration
   that follows; `/* */` for the rest. Nothing is lost — no `//` comment here
   would be *wrong* to publish — and 631 sites become correct with no edit.
2. **A lint (`doc-comment-slash`).** An exported declaration preceded by a `//`
   block where `///` was meant. Autofixable: one byte per line.

Where a lint fits: a page at `cli/src/docs/reference/lints/<code>.md`, an
`l!(...)` line in `LINTS` (`cli/src/documentation/lints.rs:48`), a check beside
`check_warning_comments` (`cli/src/commands/lint.rs:925`, dispatched at `:816`).
That check already finds comments in the gaps between lexed tokens, so "the
comment block before this declaration's first token" is the same walk, and
`--fix` already carries edits (`cli/src/commands/lint.rs:329`).

Related: no module in the monorepo has a `//!` header either.

---

## 2. Hex

* `bytes.toHex` (`bytes.buri:77`) allocates a two-char `Str` per byte and joins.
  It is the reference implementation and the slow one.
* `char.toDigit(radix)` is the only hex primitive: no `char.isHexDigit`, no
  `char.fromDigit`, no `Int → hex Str`, no `str.toInt` with a radix.
* Rebuilt twice, character for character — same `HEX_DIGITS` table, same
  `nibble`, same 16-call unrolled `hex16`:
  `libs/database/storage_simulator/hex.buri:6-59` and
  `libs/database/database_simulator/prng.buri:6-81`.
* `libs/database/storage_simulator/labels.buri:19-22` re-implements `bytes.toHex`
  from its local `hexByte` + `join`, in a repo that calls the real one at
  `libs/database/session/identity.buri:39`.
* Hex validation is `character.toDigit(16).isSome()`
  (`libs/database/model/request.buri:56`).

```
char.isHexDigit(self): Bool
char.fromDigit(n: Int, radix: Int): Option<Char>
num.toHex<C: Alloc>(ctx, x, width: Int): Str      // zero-padded, lowercase
str.toRadix(text: Str, radix: Int): Option<Int>   // the general gap
```

---

## 3. Duration, and where it lives

`Duration` exists (`date.buri:98-102`) and is nearly unusable.

* Getters only, plus free `between`/`plus`. **No constructors** —
  `Duration.seconds(60)` cannot be written — no arithmetic, no `Show`.
* It lives in `core/date` while `Instant` lives in `core/time`. A method belongs
  to its receiver's defining module (SPEC 6.7.3), so `instant + duration` can
  never be a method. **`Duration` probably belongs in `core/time`.**
* `time.since` and `Instant.duration` return `Int` millis (`time.buri:17,22`) —
  the type exists and the clock does not produce it. `sleepMs(ctx, Int)` should
  be `sleep(ctx, Duration)`. Millisecond resolution only; the monorepo's clock
  domain is nanoseconds.

Built instead:

* `NANOS_PER_MILLISECOND` defined twice (`libs/database/model/clock.buri:7`,
  `libs/database/session/expiry.buri:5`); `core/date` exports `MILLIS_PER_*` and
  no nanos. `millis * 1_000_000` written inline in six more places
  (`tenant/tenant.buri:12`, `database_simulator/sim.buri:151`,
  `storage_simulator/labels.buri:46`, `commit/test/clock.buri:97`,
  `commit/test/durability.buri:94`, `session/test/connect.buri:314`).
* Unit-suffixed `I64` constants as the deadline vocabulary:
  `IDLE_TENANT_TIMEOUT_MILLIS` (`tenant/config.buri:19`),
  `DETACHED_SESSION_TTL_MILLIS` (`server/config.buri:20`, again at
  `database_simulator/sim.buri:40`).
* The same overflow-guarded deadline check three times — `checkedMul`,
  `checkedSub`, fall back to the sign: `session/expiry.buri:14`,
  `model/clock.buri:91`, `tenant/tenant.buri:110`.

Wants: nanosecond `Duration` with `seconds/millis/micros/nanos/minutes/hours`
constructors, `add/sub/mul/abs/negate`, saturating comparison so the overflow
dance disappears, `Instant.hasPassed(deadline)`, and `Show` ("1.5s", "300ms").

**No timers, no retry.** `tenant/manager.buri:252`: *"Buri has no background
task, so the server root runs it at the start of every event it handles"* (same
at `server/root.buri:164`). A grep for `retry`/`backoff` finds nothing but a test
title — worth naming as an absence in a repo that does fault injection.

---

## 4. Randomness

Today: `effect Rand { nextInt(lo, hi), nextFloat() }` (`effect.buri:142`),
`core/random` as a two-function wrapper (`random.buri:11,16`), `HostRand`
(`host.buri:96`), `rand().seed(Int)` on the test platform
(`host_testing.buri:936-941`).

* **No pure seeded generator.** Every draw needs a context, so a deterministic
  simulator cannot use `core/random` at all. Both simulators ship their own
  splitmix64, and they differ: `storage_simulator/prng.buri:4` (salted) vs
  `database_simulator/prng.buri:29` (counter-threaded).
* **No splitting**, so streams are decorrelated by hand — **14 salt constants** at
  `storage_simulator/workload.buri:21-45`.
* **No `bytes(n)`**: `randomBytes` is `list.range` +
  `random.int(0,256).wrapToU8()` per byte (`session/identity.buri:48`).
* **No derived distributions.** `bool`, `oneOf`, `weighted`, `chance(ppm)` are all
  open-coded: a percentage ladder at `storage_simulator/workload.buri:161-178`,
  PPM fault sampling at `:450`, `% 2`/`% 3` branch choices at
  `database_simulator/input.buri:68-96`.
* **Modulo bias** in the bounded draw (`storage_simulator/prng.buri:14`).
* **No CSPRNG, and it is load-bearing**: `session/identity.buri:24` cites
  `bugs/rand-not-csprng.md` — resume bearer tokens are "only as unguessable as
  the host's generator happens to be".
* `random.int(lo, hi)` **aborts on an empty range** (`random.buri:9`).

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

* **Lexicographic list comparison, five copies** — the same index-recursive
  `match ((left[at], right[at]))` at `storage/valuekey.buri:88,74`,
  `query/values.buri:219,234`, `storage_simulator/model.buri:239,253`. →
  `list.compareBy(a, b, cmp)`, and `Ord for [T]`.
* **Comparator combinators.** `compareKeys`/`compareRows`
  (`query/aggregate.buri:165,424`), `compareOrderBy` (`query/project.buri:61`),
  `impl Ord for PosEntry` (`storage/fields.buri:28`) are all "by this key, then
  that one". → `order.by(f)`, `order.chain([cmp])`, `order.reverseIf(Bool)` —
  the last is `directed` (`query/project.buri:85`), inlined again at
  `aggregate.buri:437`.
* **Total float order (NaN last), three times, differently**:
  `storage/valuekey.buri:145`, `query/values.buri:211`, and
  `storage_simulator/model.buri:278` — where NaN silently answers `.Equal`, a
  latent bug in a file that does generate `.Float64` values. → `num.compareTotal`.
* **UTF-16 vs scalar string order written out by hand**, surrogate arithmetic
  included (`query/values.buri:65,90,107`). The comment at `:97` is the request:
  *"`Str.compare` orders by UTF-16 code unit and says nowhere that it does."* →
  document it, and offer `str.compareScalars`.
* Already-present things re-implemented: `order.int`/`order.bool`
  (`storage/valuekey.buri:109,113`, `storage_simulator/model.buri:270-280`),
  `Order.isLess`/`isEqual` (`query/bindings.buri:226-240`), and `Order.then`
  open-coded as `match { .Equal => next, ordered => ordered }`
  (`storage_simulator/model.buri:174,205`, `workload.buri:359`).

---

## 6. List and map operations that are missing

* **`traverse`/`sequence` — 13 hand-written folds.** The helper exists once,
  private to one package: `validateEach` + `pushValidated`
  (`wire/common.buri:11,23`). Everyone else re-inlines it
  (`query/project.buri:97,110`, `query/aggregate.buri:33,78,109,187,287,306,402`,
  `query/bindings.buri:167`, `query/engine.buri:51`, `wire/write.buri:40`). →
  `list.mapResult`, `list.filterResult`, and the `Option` pair. The
  highest-count missing combinator in the survey.
* **Duplicate detection: five algorithms, four packages.** `hasDuplicate` +
  `Duplicates` (`storage/storage.buri:289-304`) and `hasDuplicateColumns` +
  `Adjacent` (`query/plan.buri:132-149`) are the same sort-then-fold at different
  element types; `distinctTargets` (`wire/write.buri:36`) folds into an `OrdSet`;
  `wire/query.buri:213` compares set length to list length; `pushUnique` is O(n²)
  (`subscription/registry.buri:263`). → `list.unique`, `uniqueBy`, `hasDuplicates`.
* **`groupBy`, twice** — five declarations at `query/aggregate.buri:59-154`,
  three more at `subscription/registry.buri:169-259`. → `list.groupBy`,
  `indexBy`, `chunkBy`.
* **`maxBy`/`minBy`, four sites**: `query/aggregate.buri:251,261`,
  `database_simulator/sim.buri:264`, `server/response.buri:167`,
  `commit/clock.buri:65`. `core/list` has `maximum`/`minimum` on `Ord` only.
* `removeAt` as two slices + concat (`storage_simulator/state.buri:142`);
  `generate(n, fn(i))` as explicit recursion with a position counter
  (`storage_simulator/workload.buri:366-397`); `windows(2)`/`isSortedBy` as index
  arithmetic (`:333-348`); `filterMap` faked with a 0-or-1 list and `flatten`
  (`query/plan.buri:169,176`); `sumChecked` (`query/aggregate.buri:340`); a
  clamped window — `paginate` is `drop` + `take` + `num.max(0,_)`
  (`query/page.buri:20`).
* **`ordmap.update`/`alter`** — nested insert/remove with empty-inner pruning by
  hand (`storage/fields.buri:220,238`), get-then-insert at
  `session/manager.buri:227,352,382`. **`ordmap.mapValues`** — a sweep over every
  tenant is a fold that re-inserts each entry (`tenant/manager.buri:234-248`).
* **A counting map**: an 11-field struct with eight hand-written increments
  (`storage_simulator/exit.buri:16-138`), including a `flag(Bool): Int` helper at
  `:189` that is `Bool.toInt` (which exists, `bool.buri:27`).
* **`fold` with early exit** — eviction threads an `EvictionStep` accumulator by
  hand (`tenant/manager.buri:255-268`).

---

## 7. Bytes: a cursor, a builder, a cheap hash

* **A byte cursor written from scratch**: `struct Cursor` + `cursor` + `isDone` +
  `takeByte`/`takeVarint`/`takeSlice`
  (`storage/codec.buri:52,58,64,465,472,479`). `core/bytes` and `core/proto` both
  offer `readX(b, at) -> (value, next)`; the cursor is what everybody builds on
  top. → `bytes.Reader`.
* **Building is `[a, b, c].flatten(ctx)`**
  (`storage/codec.buri:152,186,241,282,318,338-353`) — `bytes.toHex` itself has
  the same shape. → a `bytes.Builder`, or at minimum `[U8].extend`.
* Length-prefixed framing by hand (`storage/codec.buri:454,459,240,246`); a
  counted-list read as explicit tail recursion twice, once per element type, each
  commented about avoiding O(n²) copying (`:265,437`).
* **Endian conversion by hand**: `bigEndianOctets` is eight unrolled
  `shrU64().wrapToU8()` (`storage_simulator/workload.buri:458-469`). →
  `bytes.fromU64Be/Le`, `toU64Be/Le`.
* **FNV-1a implemented inline, twice**: `fnv32`/`fnvStep` with a hand-written
  `U32_MASK` (`storage/codec.buri:95,99,28-34`) and a rolling 64-bit variant
  (`storage_simulator/workload.buri:17-19,144-158`). The comment at
  `storage/codec.buri:92` says why: *"a hand-written checksum rather than
  `core/crypto`"* — sha256 is 32 bytes of frame for a flipped-bit guard. → a
  **`core/hash`** with `fnv1a32/64`, `crc32c`, siphash. The `Hash` trait exists;
  the functions do not.
* Test vectors written one hex byte per line
  (`proto/test/wire_compat.buri:44-51`). `bytes.fromHex` exists; nothing makes it
  comfortable inside an `assert.eq`.

---

## 8. Strings, formatting, and `Show`

* **`${...}` holes accept only `Int/Float/Bool/Char/Str`** (SPEC 3.6: *"no
  user-extensible display mechanism in v0.3"*). Consequence: **251 of the 255
  interpolation holes in the monorepo are a bare identifier**, because every
  non-primitive is `let`-bound first — six `let`s before one format at
  `database_simulator/input.buri:131-141`. And ~25 hand-written `*Label`/`*Line`
  functions exist that are `Show` under another name
  (`libs/database/*/labels.buri`), including **two independent `valueLabel`s for
  the same `Value` type** (`storage_simulator/labels.buri:7` vs
  `database_simulator/labels.buri:18`). `.show(ctx)` is called twice in the whole
  repo. → letting a `T: Show` into a hole is the biggest ergonomic win here.
* **No text/report builder.** Reports are `[Str]` then `.join(ctx, "\n")`, with a
  private `bullet()` defined separately in each simulator
  (`storage_simulator/report.buri:83`, `database_simulator/report.buri:90`) and
  headers duplicated field-for-field (`:47-66` vs `:52-71`). → `core/text` with
  `bulletList`, `section`, `indent`.
* **Column alignment is literal `\t` in format strings**
  (`storage_simulator/violation.buri:190-198`, `runner.buri:231-234`) —
  `padStart`/`padEnd` exist and are never used in this repo.
* **A `key=value` log line split across two functions because the format string
  got too long** (`storage_simulator/exit.buri:111-137`). → a logfmt renderer;
  there is no `core/log` at all.
* **No path joining**: `str.format(ctx, "${dir}/storage.header")` × 4
  (`storage/durable.buri:40-43`). → `core/path`, or at minimum `fs.join`.

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

---

## 10. Filesystem

* **Three copies of a 12-method `Fs` impl**, because there is no null or
  decorating base: `apps/database_server/main.buri:55-106`,
  `database_simulator/nofs.buri:6-57`, `storage_simulator/faults.buri:54-126`
  (the last a fault injector that must restate every method to forward it). →
  `fs.NullFs`, `fs.ReadOnlyFs`, and a forwarding base. The general shape —
  *decorating an effect requires restating it* — likely applies beyond `Fs`.
* **Atomic write hand-rolled** (temp, `sync`, `rename`, `sync` the directory) with
  five chained `mapErr`s (`storage/durable.buri:75-105`). → `fs.writeAtomic`.
* **"read it, or `.None` if absent"** as an adapter matching `NotFound`
  (`storage/durable.buri:169`). → `fs.readBytesOptional`.
* `Fs` has `makeDir` and no directory removal
  (`storage_simulator/runner.buri:206`).

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

**`assert.fail` returns `()`, not the bottom type** (`assert.buri:43`), while the
private `failExpected` does return bottom (`:89`). So a failing match arm cannot
produce a value and a test must fabricate one:
`storage_simulator/test/runner.buri:20-40` calls `assert.fail(...)` then
`emptySuccess()`, a dummy constructor that exists only to type-check. Same idiom
at `database_simulator/test/runner.buri:52-59,89-92`. → make `fail` diverge, and
add `assert.matches(value, .Variant)` returning the payload.

---

## 12. Numerics, small gaps

* **Floor division and Euclidean remainder by hand**: `PhysicalTime.parts`
  (`model/clock.buri:75`) and `core/date` itself (`date.buri:183,269-273`). →
  `num.divFloor`, `num.remEuclid`.
* `Saturating` is used **once** in the whole monorepo
  (`subscription/registry.buri`), while `checkedSub`-then-decide-by-sign appears
  three times as a saturating subtract in disguise (§3).
* Masking the sign bit to make a positive `I64` from a `U64` draw
  (`database_simulator/input.buri:236-251`).
* `math.INFINITY` is spelled `1.0e400` (`math.buri:54`) — worth a look.

---

## 13. Not stdlib material — for the maintainer

**Compiler bugs worked around**, all tracked as `bugs/*.md` in that repo. Listed
because the *shape* of the workaround is the cost.

* `derive-ord-array-native-backend` (5 sites) — the native backend cannot compile
  a derived comparison over an array. Forces the `ValueKey` newtype to exist at
  all (`storage/valuekey.buri:5`), a hand-written `impl Ord for PosEntry`
  (`storage/fields.buri:18`), 80 lines of comparators
  (`storage_simulator/model.buri:204-280`), and a hex-key indirection so an
  `OrdMap` can be keyed by a token (`session/manager.buri:41`,
  `session/identity.buri:34`).
* `native-backend-host-fs-and-env` (5, two in `BUILD.buri` files) — two apps ship
  `outputs: [{ platform: JS }]` only.
* `rand-not-csprng` (3) — §4. `js-proc-exit-drops-buffered-output` (3) plus
  `js-main-err-prints-nothing`: a report survives a pipe and a terminal but not
  `> report.txt`, and returning `.Err` instead prints nothing at all.
* `let-bound-projection-off-a-generic-call-aborts-natively` (3, all tests) —
  reading one field out of `assert.some(xs[0])` aborts the native test binary.
* `str-compare-orders-by-utf16-code-unit` (2) — §5.
  `float-equality-in-a-condition-is-ieee` (2) — forces
  `!(v < 0.0 || v > 0.0 || v == 0.0)` as the NaN test (`storage/valuekey.buri:161`).
  `coalesce-option-of-array-native-backend` (2) — `??` over an `Option` holding
  heap contents aborts natively, so it is the `match` the operator would have been.
* **Two silent-wrong-answer bugs, the category to fix first.**
  `js-tuple-destructuring-calls-twice`: the JS backend evaluates a call once per
  element of a destructured tuple, so `let (m, out) = connect(...)` mints two
  sessions — an entire `struct Step<T>` exists to avoid tuples
  (`session/manager.buri:52,59`). `list-map-swallows-result-element`: a narrowing
  `toU8` inside `map` answers a `Result` that `map` drops with no diagnostic,
  leaving a list of zeros (`session/identity.buri:44`).
* Also: `match-arm-bindings-clobbered-by-a-sibling-field-read`
  (`server/root.buri:302`), `mutual-tail-recursion-native-backend`,
  `option-of-decoded-struct-field-abort` (a field must be recomputed by re-walking
  every patch, `storage/fields.buri:86`), `storage-simulator-native-match-abort`
  (buri#41 — a whole suite pinned to `platforms: [JS]`),
  `fs-cannot-remove-a-directory`.

**Misuse / discoverability — the library has the answer and nobody found it:**

* `Result.ignore()` used **once**; the four-line
  `match (io.println(...)) { .Ok(_) => (), .Err(_) => () }` written six times
  (`database_simulator/cli.buri:96,103`, `storage_simulator/cli.buri:98,105`,
  `storage_simulator/runner.buri:219,272`), as duplicated `printLine`/`logLine`
  helpers in both simulators.
* `order.int`/`order.bool`, `Order.then`, `Order.isLess`, `Bool.toInt`,
  `padStart`/`padEnd`, `Saturating`, `.show(ctx)`, `context Fixture {}` — all
  present, all re-implemented or unused. A discovery problem as much as an API
  one, and it is what makes §1 matter: a library nobody can hover over is a
  library nobody finds.
* Test fixtures are copy-pasted rather than shared:
  `at(millis)`/`clock(millis)`/`commitTime` in at least six files
  (`commit/test/clock.buri:97`, `commit/test/durability.buri:94`,
  `commit/test/committer.buri:340`, `tenant/test/operations.buri:458`,
  `tenant/test/residency.buri:317`, `database_simulator/test/checks.buri:22`);
  `identity`/`entity`/`alice`/`bob` across four files in `storage/test/` alone.
  `query` solved it with a `testing/lib.buri` and nobody copied the pattern.
  `let ctx = context { Alloc: alloc() };` is written ~20 times where
  `context Fixture { ... }` (`database_simulator/test/runner.buri:21-25`) does it
  once — used in exactly one file.
* A 14-line doc comment appears verbatim twice in a row at
  `storage/valuekey.buri:117-130` and `:131-144`.

**Both simulators duplicate whole modules**, not just helpers: `prng.buri`,
`hex.buri`, `labels.buri`, `report.buri`, `args.buri`, `cli.buri`,
`violation.buri` exist in both with substantially the same content. Some is
stdlib material (§2, §4, §8, §9); the rest is a missing shared library there.
