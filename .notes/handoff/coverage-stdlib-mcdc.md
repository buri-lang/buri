# Stdlib MC/DC under the all-decisions measure

Seeded from CI run 38427330933, then reseeded from run 38440131170 (x86_64 Linux) after 904063804 added built-ins: now 6185/10527 (58.8%). Per-module numbers below are from the first seed; coverage/stdlib-mcdc.txt on main has the current ones. Before the reset: 983/985 (99.8%), && and || chains only.

Total first seed: 6180/10520 conditions (58.7%).

Top modules by missed conditions:

| module | shown/total | missed |
|---|---|---|
| core/orderedmap | 514/1447 | 933 |
| core/map | 253/702 | 449 |
| ui/node | 398/839 | 441 |
| core/option | 406/683 | 277 |
| core/result | 300/574 | 274 |
| core/actor | 195/395 | 200 |
| ui/style/classify | 72/230 | 158 |
| core/buri/ast | 782/930 | 148 |
| core/bytes | 99/219 | 120 |
| std/codegen/proto/schema | 314/432 | 118 |
| core/proto | 256/371 | 115 |
| core/net/server | 115/229 | 114 |
| core/crypto | 76/168 | 92 |
| core/indexset | 78/152 | 74 |
| core/net/http | 63/128 | 65 |

Every module:

```
195 395 core/actor
2 3 core/alloc
78 109 core/bigint
2 6 core/bool
782 930 core/buri/ast
99 219 core/bytes
20 25 core/character
121 132 core/cli
24 29 core/codegen
94 108 core/compression
76 168 core/crypto
32 34 core/csv
160 224 core/date
62 78 core/decimal
8 8 core/env
5 9 core/hash
45 56 core/heap
78 152 core/indexset
4 5 core/io
184 234 core/json
227 280 core/list
253 702 core/map
42 43 core/math
63 128 core/net/http
115 229 core/net/server
1 2 core/net/tcp
114 127 core/net/url
12 23 core/net/websocket
42 75 core/number
406 683 core/option
28 40 core/order
514 1447 core/orderedmap
14 18 core/orderedset
94 103 core/path
256 371 core/proto
9 25 core/proto/any
7 16 core/proto/duration
1 5 core/proto/empty
4 7 core/proto/field_mask
63 110 core/proto/struct
7 16 core/proto/timestamp
27 82 core/proto/wrappers
17 22 core/queue
12 31 core/random
300 574 core/result
0 0 core/secret
12 13 core/set
52 93 core/simd
237 289 core/str
15 66 core/tasks
3 5 core/testing/check
12 29 core/time
14 21 core/uuid
316 351 std/codegen/proto
314 432 std/codegen/proto/schema
2 21 std/codegen/proto/wellknown
398 839 ui/node
13 15 ui/prop
0 0 ui/signal
1 6 ui/style
72 230 ui/style/classify
18 24 ui/theme
2 3 ui/web
```

## Phase 1 (2026-10-13, branch cov-stdlib-mcdc-p1)

JS-run total went from 6185/10527 to 6884/10041 (68.6%). CI raises the baseline itself on the push to main.

What changed:

- Tests: ui/node (398/839 to 738/792), orderedmap, map, and the well-known proto types (`cli/tests/conformance/lib/proto/test/known_types.buri`). Wrappers, any, duration, timestamp and struct are now at or near full.
- Compiler: an integer `/` or `%` by a nonzero constant expression no longer counts as a decision, and nor does a `core/bits` shift by a constant inside its width (`coverage::Aborts`). Condition labels keep their outer parentheses.
- Refactor: orderedmap `fixChild` lost an `at > 0` check that the public API can't make false.
- Bug fixed: `n / -PER_ROW` counted a decision; the constant reader now takes `-x` too.
- Perf note: in `map.buri`, don't move a branch's slot lookup into a helper. The call holds the children list again, so the insert below it copies instead of updating in place (`run/maps` was +7%).

On main at 503a8aa63.

Next targets, by gap: orderedmap 763, map 354, option 277, result 257, actor 203 (needs a runtime), buri/ast 146, proto/schema 117, net/server 114.

Blocked: Option and Result `Equal` comes from derives.rs, which another agent owns. The generated proto files' JSON `?` arms can't fail, because `decodeValueJsonAt` never fails.

## Phase 2 (2026-10-13, branch worktree-agent-a909c159216b3408c)

JS-run total went from 6889/10057 (68.5%) to 7271/9987 (72.8%).

| module | before | after |
|---|---|---|
| core/orderedmap | 551/1314 | 674/1222 |
| core/map | 302/667 | 321/696 |
| core/buri/ast | 782/928 | 903/923 |
| std/codegen/proto/schema | 314/431 | 412/412 |
| core/option | 406/683 | 416/689 |
| core/result | 331/588 | 331/588 |

Bugs fixed, each with a failing test first:

- JS backend: a field carried over by `S { ..s, f: x }` aliased the original's list, so a push on the copy grew the original (`memory/test/updates.buri`). Carried fields now go through `$fromShared`. A struct-literal base no longer constant-folds in the golden output; accepted as rare.
- buri/ast printed five constructs differently from the formatter when an operand, condition or scrutinee broke.
- proto/schema reported a peeked broken token twice, and quoted a field feature value in double backticks.

Refactors: orderedmap split asks for its key once; buri/ast `printDecls` and `pathText` lost unreachable `withDefault`s; proto/schema dropped `derive Equal for Token`, the byte-offset fallback, the `nearest` floor and `editDistance`'s defaults.

Option and result are blocked by per-instantiation counting: about half their unshown conditions are copies of a shown one in another type. Lifting that is a measure change (coverage golden tests), not tests. Actor (203) and net/server (114) are reachable from the JS runtime tests, but most misses are per-instantiation copies too; the real gaps are `SendError`/`Stopped` equality and the server config defaults.

Watch out: a refactor that adds a generic instantiation (e.g. `map.groupBy` over a new tuple) can drop that module's share below the baseline even with nothing lost. Compare each module against `coverage/stdlib-mcdc.txt` before pushing.

On main at 8d426c969.

Next targets: orderedmap 548, map 375, option/result (measure change), actor equality, net/server config arms.

Seen in passing: `repositories::language_server_open_cost` failed once on a busy arm64 runner (medians 15/11ms vs the usual 5/5ms), passed on rerun. It's a wall-time budget, so it's flaky by design.

## Phase 3 (2026-10-13, branch coverage-stdlib-mcdc-p3)

JS-run total went from 7271/9987 (72.8%) to 7833/10068 (77.8%). No module is below `coverage/stdlib-mcdc.txt`.

| module | before | after |
|---|---|---|
| core/orderedmap | 674/1222 | 1139/1255 |
| core/map | 321/696 | 362/723 |
| core/actor | 192/395 | 200/395 |
| core/net/server | 115/229 | 138/229 |
| core/option | 416/689 | 436/709 |
| core/orderedset | 14/18 | 19/19 |

No bugs found. Refactor: orderedmap `overfull` asks for the key past `MAX_ENTRIES` and the middle key in one tuple match, which drops the unreachable `.None` arm in each instantiation. `PlanListen` (semantics/shapes.buri) now reports address length, request limit and idle timeout on `Listener.handle`.

Test edits change the checking and formatting sampled corpora: rerun `BURI_BLESS=1 cargo test -p buri --test checking` and `--test formatting`.

Left in orderedmap: about 33 `C = Scope` copies (Show inside the test reporter, so unreachable), Bool/Mood, codegen-internal Str/Entry and Str/[Entry], and unreachable `.None`/`_` arms at fixChild, fixWithLeft/Right, takeFirst/Last and rangeFrom.

Left in core/map: about 260 of the misses are hash-collision code, unreachable for any key type that doesn't actually collide. Storing `Collision` keys and values as separate lists would let one K-only `positionOf` replace the four `(K, V)` list helpers and their unreachable arms. That's a data-structure change, so it needs the instructions bench.

Next targets: the map Collision refactor; a derive-Equal fix so a one-variant enum doesn't emit an unreachable "another variant" condition (`actor.Stopped`); actor's remaining per-instantiation `mailbox`/`closing` arms.

## Phase 4 (2026-10-13, on main at 517123204)

JS-run total went from 7833/10068 (77.8%) to 7795/9834 (79.3%). Most of the drop in the denominator is unreachable conditions that no longer exist. No module is below `coverage/stdlib-mcdc.txt`.

| module | before | after |
|---|---|---|
| core/map | 362/723 | 331/537 |
| core/orderedmap | 1139/1255 | 1130/1207 |
| core/actor | 200/395 | 199/393 |

No bugs found.

- **derive Equal:** a one-variant enum no longer emits an "another variant" condition (`coverage/derived.rs`). The test is `cli/tests/repositories/coverage/mcdc_one_variant`.
- **map:** `Collision(U64, [K], [V])`. One `positionOf` replaces the four `(K, V)` list helpers. A removal can now leave a collision with one pair, and none is `.None`. run/maps rose by 0.26% in instructions.
- **orderedmap:**
  - `rejoin` replaces fixWithLeft/Right.
  - `dropFirst`/`dropLast` with `leftmost`/`rightmost` replace takeFirst/Last.
  - rangeFrom and prefixAt take the child in hand, and `route` is gone.

Left in orderedmap:
- fixChild's inner `_` arm, in 4 instantiations. It's unreachable, because the first child always has a right sibling.
- lookup's `found` for (I64, [I64]) and (Str, Bool).
- Scope, Bool and Entry copies, as in phase 3.

Left in map:
- the Collision arms and `positionOf` for each K, which are inherent unless keys really collide
- the per-instantiation gaps for internal key types (Access, Uuid, IndexSet<Slot>)

Next targets: actor's `mailbox`/`closing` arms in each instantiation, and the gaps in core/option and core/list.

## Phase 5 (2026-10-13, on main at 26feadc5c)

Merged run on all four backends went from 7795/9834 (79.3%) to 7946/9733 (81.6%). No module is below `coverage/stdlib-mcdc.txt`.

| module | before | after |
|---|---|---|
| core/map | 331/537 | 489/567 |
| core/actor | 199/393 | 164/273 |
| core/list | 240/284 | 264/275 |
| core/orderedmap | 1130/1207 | 1131/1203 |
| core/option | 439/711 | 442/713 |

No bugs found.

- **Real collisions:** hashes are 32-bit FNV-1a, so colliding keys are easy to compute. `collections/test/map.buri` has colliding strings, integers, floats and pairs; `uuid/test/hashing.buri` has colliding uuids. Each test asserts its hash premise.
- **actor:** every `first()` over a one-element carried block is now a fold (`stepping`, `putting`), which drops their unreachable `.None` arms. Each actor in the corpus is now stopped twice and sent to after its stop.
- **list:** the binary searches match `(lo < hi, xs.get(mid))` in one tuple; `lastFrom` and `median` let `get(-1)` answer `.None`.
- **orderedmap:** `fixChild` puts `short` into both tuple matches, so both `_` arms are reachable.

Left:
- map: Collision arms for internal key types (Access, IndexSet<Slot>) and the test-only `Collider`/`AllOne`; removeNode's Branch `.None` (`remove` asks `has` first).
- option: 218 derived Equal/Ordered conditions at line 13, then per-T `isNone`/`map` arms.
- actor: `scopesLive()` in `across<T>` (one copy per T), WouldDeadlock/race arms.
- list: `trimmed` in indexset calls `findLastIndex` only after `findIndex` found a word, so its U64 `.None` is unreachable.

Next targets: core/result (257 unshown), core/option, core/net/server, core/crypto, core/indexset.

## Phase 6 (2026-10-13, on main at de5ec5394)

The merged run went from 7946/9733 (81.6%) to 8108/9722 (83.4%), which CI confirmed on x86_64. No module fell.

| module | before | after |
|---|---|---|
| core/crypto | 76/157 | 91/91 |
| core/option | 442/713 | 539/729 |
| core/result | 331/588 | 350/600 |
| core/net/server | 138/229 | 164/256 |
| core/net/websocket | 12/23 | 17/23 |

Bug fixed (f15265a32): `--coverage=mcdc` didn't count a `core/bits` rotate's out-of-range abort as a decision, though a shift's counted. The e2e case is `cli/tests/repositories/coverage/mcdc_rotate`.

- **crypto:** the hash state is a struct, and rotations are `bits.rotate*` with constant counts, so they add no decisions. Words are read by a `toU32BigEndian` recursion instead of `withDefault(0)`. Schedules and rounds use tuple matches. The DER tests cover each length mismatch and each wrong tag.
- **Option built-ins:** `filter` and `andThen` record whether the receiver is Some, not what the predicate answered. To show one, pass a None receiver once.
- **Derived Equal "another variant":** this is the shadow of `eq_enum`, not a measure bug. `bothVariants` in `data/test/optionresult.buri` shows Some≠None for each held type.
- **New instantiations:** calling `json.parse`, `bytes.fromUtf8` or `path.of` with `alloc()` (GeneralPurpose) creates a new C instantiation, and json, bytes and path fell. Use a `context { Allocator: alloc() }` literal instead, which is the existing "a context" type, or build the values directly.

Not done, with reasons:
- map `removeNode` Branch `.None`: dropping the `has` check adds a decision for each instantiation and path copies on a hot path.
- `across`/`scopesLive`: moving it means touching the runtime or copyAcross, which are hot paths.
- map Collision arms for Access and IndexSet<Slot>: these need real hash collisions between internal keys, which a public-API test can't arrange.

Next targets:
- core/indexset (79/155): mostly per-K arms for the test-local K types Octet, Seq, Row and Tagged in `collections/test/indexset.buri`.
- core/net/server: listener arms for S=I64 and S=Str.
- option and result: derived variants of proto and internal types, and Show for `Option<proto wrappers>`.

## Phase 7 (2026-10-17, on main at 022d9a4d1)

Measured locally, the merged run went from 8108/9722 (83.4%) to 8898/9833 (90.5%). No module fell.

| module | before | after |
|---|---|---|
| core/indexset | 79/155 | 235/245 |
| core/option | 539/729 | 700/731 |
| core/result | 350/600 | 589/606 |
| core/net/server | 164/256 | 229/256 |
| core/net/http | 63/128 | 127/128 |
| core/simd | 52/93 | 93/101 |
| core/proto | 267/326 | 287/328 |
| std/codegen/proto/wellknown | 2/21 | 21/21 |
| core/compression | 94/108 | 99/111 |
| core/hash | 5/7 | 7/7 |

Bug fixed (022d9a4d1): under `BURI_COVERAGE_STD=1`, `core/bits` is a source module, so a reached shift's or rotate's abort went uncounted. An unreached rotate was never counted at all, because `checked_traps` matched only `shift*`. The e2e cases are `mcdc_rotate` (the `idle` function) and `coverage_counts_its_own_shifts_alike_with_the_standard_library` in `cli/tests/build/repositories.rs`. The fix adds decisions, so simd, proto, compression and hash grew denominators and needed tests or refactors.

- **hash:** siphash reads words and the tail with tuple matches and rotates with `bits.rotateLeftU64`, so it has no shift decisions.
- **compression:** `runLengths` lost its unreachable "symbol past 18" arm, and `takeBit` lost a `c.error >= 0` check that both callers already make. New deflate tests cover truncated stored and dynamic headers, a stored block with too few bytes, a fixed literal past 285, and a repeat code with nothing before it.
- **Tuple scrutinees** need double parentheses: `match ((a, b)) { ... }`.

Not done:
- server: the `C = Scoped<a context>` copies and the websocket `.None` arm for S=I64 and S=Str. Reaching them needs contrived tests.

Next targets:
- core/tasks (15/66): mostly `beside` and `working`, which run only in a native `--release` program outside tests.
- After that, the lowest fractions in `target/stdlib-coverage/stdlib-mcdc.txt`.

## Phase 8

Stdlib MC/DC went from 8898/9833 (90.5%) to 9062/9852 (92.0%). No module fell. Staging is at `b0908b9e5`.

| Module | Before | After |
|---|---|---|
| core/date | 160/199 | 185/199 |
| core/json | 184/232 | 225/231 |
| core/number | 43/69 | 51/71 |
| core/option | 700/731 | 713/743 |
| core/order | 28/40 | 40/40 |
| core/proto | 287/328 | 297/328 |
| core/result | 589/606 | 595/612 |
| core/str | 239/266 | 248/266 |
| ui/node | 738/792 | 739/792 |
| ui/style/classify | 184/226 | 223/226 |

### Refactors

- **json `asInt`:** it uses `toI64`'s `.Ok` directly. `toI64` already returns `.Err` for a fraction, so the second whole-number check could never be false.
- **date `parseDate`:** it matches `(get(0), get(1), get(2), get(3))` and the three `toInt`s as tuples. This replaces a length check followed by `andThen` calls whose `.None` sides could never happen.

### What's left, and why

- **core/tasks (51 unshown):** about 45 of these are `beside`/`working`. Tests can't reach them, because under a test the runtime's scope-beside returns false on purpose.
- **Unreachable sides that stay:**
  - `.get(i).withDefault` on lists whose length is already known (bytes, bigint, date, str, proto);
  - traps on a non-constant divisor;
  - json's `toChar` `.Err` arms.
- **ui/style/classify (3 unshown):** the Extracted arm, `declared`'s `_` arm and the `.all` built-in.
- **Biggest remaining modules:** core/actor (109), core/map (76), core/orderedmap (71), ui/node (53), core/option (30).

### Gotchas

- **New instantiations lower a module's share.** A test that compares `Option<Date>` or `Result<F64, ProtoError>` adds a new instantiation of `Equal`. Show every side of it in the same test (Some/Some, Some/None, None/Some), or core/option and core/result fall.
- **The sampled recovery corpora depend on the conformance files.** After you edit conformance tests, run `BURI_BLESS=1 cargo test -p buri --test checking --test formatting`.
- **`buri lint` flags context bounds that aren't used.** The language suite fails on `unused-context-bound`, so a test helper should ask only for the capabilities its body uses.
- **Don't trust a single-package lcov merged with the gate's lcovs.** Condition indexes differ between runs, so the merge can report conditions as unshown when they're shown. Read the package's own lcov, or run the whole gate.

## Phase 9

Staging: `867024c67`.

MC/DC, measured locally on four backends: 9062/9852 (92.0%) before, 9132/9824 (93.0%) after. No module fell.

| Module | Before | After |
|---|---|---|
| core/actor | 164/273 | 203/245 |
| core/map | 491/567 | 520/567 |
| core/orderedmap | 1132/1203 | 1134/1203 |

### Commits

- `33cf85bba`: the actor refactor and the actor tests.
  - `sendMessage`'s `.None` arm now calls `unanswered(driven)`, a non-generic helper. Before, every instantiation repeated a match on `driven`. This removed 28 conditions.
  - Each payload actor now also runs its whole life from a named function, called inside an `alloc.scoped` body that allocated first. That makes `scopesLive()` answer true natively, which shows `across`'s other side for each type. The tests are in these files:
    - `actor/test/payloads.buri`, `counter.buri` and `scoped.buri`;
    - `data/test/steps.buri`;
    - `tasks/test/scope.buri`;
    - `semantics/test/host_testing.buri`.
- `d0e0ee4e3`: the map tests.
  - `Collider`'s hash is now `(id % 8) * 32`, so the tree branches below the root.
  - The Collider tests now cover removal, `entries`, and absent removal for I64 values. A pair of lone keys covers Str values.
  - Two new Access and IndexSet cases: replacing an existing key, and looking up keys that were never stored.
  - New (I64, I64) cases for `mergeWith` and `equals`.
  - New cases for `countBy` under an Int key and `filterMapValues` to `()`.
  - A lookup of `false` in a Bool-keyed orderedmap that holds only `true`.
- `867024c67`: formatting.

### Traps

- A test can add an instantiation without anyone noticing, and that new instantiation can lower a module's share:
  - A second `tasks.scope` that answered a `Result` dropped core/tasks.
  - `isNone()` on an `Option<Mood>` dropped core/option.
- Reuse the instantiations that already exist. `assert.equal(x, .None)` is one.
- The gate compares shares, not counts.

### What remains

- **actor 343 `answering` `.None` and 452 `taking` `.Err`, for each of 13 actor types.** Each type's protocol needs a variant that sends to its own actor from inside a step. One protocol has `M = Str`, so it can't take such a variant.
- **actor 491 `discarding` `.Some`.** Only a native race reaches it.
- **map:**
  - IndexSet<Slot> collisions: no two test sets share a hash.
  - Access, AllOne and the `[(I64, I64, Item)]` keys: their hashes never collide, or never differ.
  - 318 `removeNode` Branch `.None`: unreachable.
  - The shift traps at lines 91 and 96.
- **orderedmap (Str, [Entry]) and (Str, Entry):** these maps are codegen_proto's. Reaching them needs a proto with more ambiguous or used names than a node holds.
- **orderedmap `C = Scope` copies:** only a failing assertion's reporter runs them.

### Validation

- The full suite passed in 425 s, including the build. That is over the 5-minute limit.
- fmt passed.
- Clippy shows four warnings, all from one existing line: `cli/tests/harness/case.rs:1295`, `.last()` on a DoubleEndedIterator. CI doesn't deny warnings.
- After the rebase, these tests passed again: language, native, checking, formatting and recovery.

## Phase 10

Total went from 9132/9824 (93.0%) to 9205/9822 (93.7%), measured with all four gate runs on aarch64-apple-darwin.

| module | before | after |
|---|---|---|
| ui/node | 739/792 | 766/792 |
| std/codegen/proto | 320/349 | 344/347 |
| std/codegen/proto/schema | 412/412 | 414/414 |
| core/cli | 121/132 | 130/132 |
| core/net/url | 114/125 | 123/125 |
| core/orderedmap | 1134/1203 | 1136/1203 |
| core/option | 713/743 | 713/741 |

No module fell.

## Changes

- **ui/node:** `lib/ui/test/render.buri` is in `lib/ui/BUILD.buri`, so it now runs under `buri test` on JavaScript as well as natively. `NATIVE_ONLY` is gone from `cli/tests/native/differential.rs`. New cases in tree, reactive and style_values cover focus binding, reactive labels, toggle styles, heading levels and run-time state styles.
- **std/codegen/proto:** new cases in proto_gen's scalars, mapping and failures cover enum defaults, packed enums, enum map values, three-way duplicate and ambiguous names, and imports.
  - `fieldType` and `defaultOf` match `mapKey(f)` once instead of testing `isSome()` and then matching. That drops an unreachable `.None` from each.
- **core/cli and core/net/url:** cases for bad tokens (`--=x`, `-=x`, `-ab=x`), a tool with no global flags, a flag with no letter, a Many flag with no value, broken relative authorities, reference queries, leading `../` and `./`, and `UrlError` inequality.

## Bug fixed

The schema reader refused a field type with a leading dot, such as `.p.Status`, saying "expected a field type, found `.`". protobuf allows it, and the generator's `resolveName` already handles it. The fix is `typeName` in `codegen_proto_schema.buri`. The e2e test is "a name that starts with a dot is looked up from the root" in `lib/proto_gen/test/failures.buri`.

## What's left

- **ui/node (26):** judged unreachable or folded by the compiler: artworkOf `_`, each's `.None`, nodeLines Raw and Fragment, runtime On/At in stylesReactiveAt and resolve, variableInline `.None`, VisuallyHidden in `declare`, and the Scope instantiation at 4119/4429.
- **std/codegen/proto (3):** upperFirst `.None` (127), wellKnownJson's parse `.Err` (3297), baseName `.None` (3822). All unreachable.
- **core/cli (2):** apply's `short` `.None` (658) and shortToken's `rest.charAt(0)` `.None` (854). A Named token always has a long or a short name.
- **core/net/url (2):** `percent`'s `fromDigit(...).withDefault('0')` (419, 420). A digit under 16 always converts.
- **Next candidates by misses:** core/bytes 99/133, core/number 51/71, core/bigint 78/92, core/proto 297/328, core/time 12/23, core/random 12/23.

## MC/DC phase 11

The total rose from 9205/9822 (93.7%) to 9231/9787 (94.3%). No other module changed.

| Module | Before | After |
|---|---|---|
| core/bigint | 78/92 | 83/86 |
| core/bytes | 99/133 | 104/122 |
| core/number | 51/71 | 51/68 |
| core/proto | 297/328 | 311/321 |
| core/random | 12/23 | 14/19 |
| core/time | 12/23 | 12/19 |

### Refactors

These cost nothing.

- **Reads through `get`:** bounds checks followed by `get().withDefault()` became matches on `list.get`. This covers:
  - bytes: `cleaned`, `continuations`, `padding`, `equalAt`, and the U16, U32 and U64 reads;
  - bigint: `fromTop`, `subLimbs`, `diagonal`, `downFrom`, `divide`, `topNonZero`, `fold` and `reciprocal`;
  - random: `exchange`.
- **Removed duplicates:**
  - proto's `floorDiv` and `floorMod` are replaced by `number.divideFloor`, and its `divide` by constant divisors;
  - bytes' `octet64` and `octet32` are replaced by U32 reads;
  - number's `significantNibbles` is replaced by `hexDigits`;
  - random's `gather` uses `octets`.
- **Constant divisors:** time's Duration Show and bigint's `residueOf` now divide by constants, so those divisions are no longer trap decisions.

### Bugs

None found.

### What is left

Everything still unshown needs one of these to happen:

- **An abort:** a division, remainder or shift by a value that isn't a constant. This covers number 164–480, bigint 591–592, random 179–180, proto 267/276 and time 224.
- **A `Scope` context:** this covers the `C = Scope` copies in time Show and `bytes.toHex`.
- **An impossible input:**
  - lists whose length is already known (the `andThen` calls in fromHex and the `withDefault` calls in varints);
  - `.None` from `alphabet.charAt`;
  - the `_` arm of random's `exchange`;
  - random's `radius == 0.0`.

### Next candidates

The six modules are now close to their limit. Run `/tmp/p11-unshown.py` on the next modules with the most conditions left unshown.

Staging: 707a43866.

## Phase 12

Stdlib MC/DC went from 9231/9787 (94.3%) to 9302/9785 (95.1%). No module fell.

| Module | Before | After |
|---|---|---|
| core/heap | 45/52 | 56/56 |
| core/queue | 17/22 | 22/22 |
| core/proto/field_mask | 4/7 | 7/7 |
| core/csv | 32/34 | 34/34 |
| core/proto/struct | 103/110 | 103/104 |
| core/list | 265/275 | 274/275 |
| core/option | 713/741 | 739/741 |
| core/result | 595/612 | 608/612 |
| core/codegen | 24/29 | 26/29 |

### Refactors

- `heap.mergePairs` pairs children by index through a recursive `pairUp`. Its first child is no longer an Option that is always Some.
- The generator's well-known Struct decoders (`codegen_proto_wellknown.buri`, and `proto_struct.buri`, which it writes) build a `Value` straight from JSON. Every JSON value is a `Value`, so the inner `?`s could never fail.

### Tests

The conformance tests now cover:

- the side of each Option and Result question that was missing;
- a pop that finds something;
- CSV errors of different kinds that aren't equal;
- a FieldMask cut short in a tag, in a path, and in a skipped field;
- actor sends after a stop;
- malformed codegen inputs.

### Unreachable, left alone

- codegen's write failure needs a custom Stdout, which would add an instantiation.
- Show of Result `Ok` and every `C = Scope` copy run only in a failing assertion's reporter.
- `withDefault` on lists whose length is already known.
- The division and shift traps.

### Next candidates

In order of the conditions they have left:

1. core/orderedmap (67)
2. core/tasks (51)
3. core/map (47)
4. core/actor (42)
5. core/net/server (27)
6. ui/node (26). One planned test: `isInFocusOrder: .Some(.Const(true))` on a stack that has no `hasFocus`.
7. core/buri/ast (20)
8. core/str (18)
9. core/bytes (18)
10. core/number (17)

### Other changes

- `cli/tests/recovery.rs`: raised the ceiling for "a syntax error stays a syntax error" × `delete-separator ()` from 2 to 3.
  - The new assertions changed which tokens the test samples, and six commas that were already in the files are now sampled. 17 of 708 is 2.4%.
  - The previous staging head passes, and nothing in the parser or the checker changed.
- Clippy warns about `Iterator::last` in `cli/tests/harness/case.rs:1295`. It came from staging, not from this phase.

Staging: bb6216d49.

## Phase 13

Stdlib MC/DC went from 9302/9785 (95.1%) to 9313/9782 (95.2%), measured with all four gate runs on aarch64-apple-darwin. No module fell.

| Module | Before | After |
|---|---|---|
| ui/style | 1/6 | 6/6 |
| core/buri/ast | 903/923 | 905/920 |
| core/net/server | 229/256 | 231/256 |
| ui/node | 766/792 | 767/792 |
| core/option | 739/741 | 740/741 |

### Bug fixed

`ast.print` kept a struct literal's fields on one line after its head broke, as in `g(h(match (a) { _ => 1 }) { x: 1 })`. `buri format` breaks the fields too. `structLit` now takes the literal's own `forces(e)` instead of asking about the spread and fields again. This removes 3 conditions. The test is in `lib/buri_ast/test/edges.buri`, "blocks, bare spreads and struct literals print".

### Tests

- `ui/test/style_values.buri`: `alpha` on each `Color` kind at run time. Constant style lists fold at compile time, so only a `late(...)` value reaches `alpha`.
- `ui/test/tree.buri`: an accept entry `text*` with no slash, and a stack with `isInFocusOrder` but no `hasFocus`. The second test adds no conditions; `asksFocus` was already shown.
- `semantics/test/effects.buri`: a server with no websocket binds with its request limit and idle timeout.
- `buri_ast/test/edges.buri`: an unclosed `\u{` escape at the end of the text, a doc line with no space after `///`, and an enum with no comma after its last variant.

### Tried and dropped

- server's `asked > MAX_HANDLERS`: `SerialTasks` runs at most three items, so no test can see the 1024 cap. A test would run the branch without checking what it does.
- `derive Equal for (A, B)` and `({ a }).b`, for buri/ast 889 and 2059: `buri format` doesn't parse either one.

### What's left

- orderedmap (67): `C = Scope` copies (Show runs only in a failing assertion's reporter); Bool/Mood can't split; Str/[Entry] is codegen's.
- tasks (51): `beside`/`working` don't run under tests.
- map (47): collisions between internal keys, the shift traps, and removeNode's `.None`.
- actor (42): `answering .None`/`taking .Err` for 13 actor types. Each type needs a reentrant step. Also `discarding .Some`, which only a native race reaches.
- server (25): `Scoped<a context>` copies and the websocket `.None` arms.
- ui/node (25): as listed in Phase 10.
- buri/ast (15): `.None` arms on lists whose length is already known, importKey's `_other`, and `parsePrimary`'s `p.panicking` for `C = a context`.
- str, bytes, number: division and shift traps, plus `withDefault` calls that can't fail. Also str's `lo == 0` in `rangeValue`: both tables start at or below the first code point asked about.

### Next candidates

The modules above are close to their limit. What's left needs a measure change, such as not counting `C = Scope` copies or division and shift traps, rather than tests. Of the smaller modules, websocket's `reasonOf` arms need a test platform socket that closes with an unnamed or negative code.

### Validation

- `cargo test --workspace --features buri/backend-llvm` passed in 698 s, including the build, on a loaded machine.
- fmt passed.
- Clippy warns only about `cli/tests/harness/case.rs:1295`, which came from staging.
- No ceiling, gate or baseline changed.

Staging: 37b9e00fd.

# Stdlib MC/DC phase 14: refactor away conditions that can't be shown

Total went from 9313/9782 (95.2%) to 9316/9739 (95.7%). Unshown conditions fell from 469 to 423. Every module held or rose.

| Module | Before | After |
| --- | --- | --- |
| core/bigint | 83/86 | 82/84 |
| core/bytes | 104/122 | 110/114 |
| core/cli | 130/132 | 126/126 |
| core/date | 185/199 | 183/186 |
| core/decimal | 62/71 | 61/67 |
| core/json | 225/231 | 225/229 |
| core/list | 274/275 | 277/278 |
| core/net/url | 123/125 | 123/123 |
| core/number | 51/68 | 51/66 |
| core/str | 248/266 | 250/259 |

## Refactors

- Hex and decimal digits are slices of a constant digit string (`HEX`, `DIGITS`, `DECIMAL`) instead of `character.fromDigit(..).withDefault`. This applies to bytes `toHex`, url `percent`, json `escape`, number `toHex` and `toRadix`, and str `carry`. Decimal and bigint text use `str.fromInt`.
- Fixed-shape reads match a tuple of `get`s instead of a length check plus `withDefault`. This applies to bytes `fromHex` and `decode64`, date's HTTP-date, clock and offset parsers, str graphemes, and cli short options.
- Base64 `encode64` walks one group per octet present. `quad` takes the second and third octets as `Option<U8>`, so padding follows from the type rather than a `shown` count.
- cli `Token.Named` holds a `Spelling` (`Long(Str)` or `Short(Char)`), so a short name is one `Char` by construction.
- Dead code is gone:
  - date `endsGmt` and a `filter` that `splitAny` already did;
  - str `codePoint` and `charCode`, the `lo == 0` branch in `rangeValue`, and the `fuel` in `reorder` (a comment proves it terminates);
  - number and decimal `countDown`, and bigint `plainDigits`.
- codegen_proto `baseName` matches `lastIndexOf("/")` directly.

## What's left

- **`C = Scope` copies and `answering`/`taking`** (out of scope). orderedmap lines 978 and 999 alone hold 41 unshown `Scope` copies.
- **orderedmap `K = Bool, V = Mood`.** It comes from the generated proto `map<bool, Mood>`; a two-key map can't split or branch, so it's inherent to that instantiation.
- **map collision and `h == hash` arms** for `Access`, `IndexSet<Slot>`, `AllOne` and `I64` keys. Those key types never collide in their tests. Showing them needs colliding keys of those types, not a refactor.
- **Division and shift traps** whose divisor is a parameter: number `gcd`, `divMod` and `toRadix`, bigint `here / divisor`, decimal `units / divisor`, and map's shifts. Each is either a real trap or provably safe but not a constant, so a constant can't replace it.
- **Compiler changes needed:**
  - Code point to `Char` after a range check (`scalar`, json `toChar` `.Err`, str 1233/1298/1353/1359) needs an unchecked conversion intrinsic.
  - bytes `fromUtf8Lossy`'s `withDefault("")` needs a `fromUtf8` that can't fail on input already cleaned.
- **`withDefault` on reads known to be in range:**
  - bytes varint `bytes.get(i).withDefault(0)` (656, 685) and compression `byteAt` and `copyBack`. Removing these takes a forward fold over halves. While the U8 instantiation of `Option.withDefault` is shown by base64, removing them is safe for core/option.
  - str 1224/1225 table reads (generated tables), and date 466 and 583.
- **str `sameFolded` `_`** is the length fast path, so it stays.

# Stdlib MC/DC phase 15

Total went from 9316/9739 (95.7%) to 9343/9723 (96.1%). Unshown conditions fell from 423 to 380. Every module held or rose. The figures are aarch64 runs on all four backends.

| Module | Before | After |
|---|---|---|
| core/bytes | 110/114 | 110/112 |
| core/codegen | 26/29 | 25/26 |
| core/compression | 99/111 | 105/113 |
| core/date | 183/186 | 184/185 |
| core/json | 225/229 | 224/227 |
| core/map | 520/567 | 532/567 |
| core/net/http | 127/128 | 128/128 |
| core/path | 94/103 | 94/96 |
| core/proto | 311/321 | 318/320 |
| core/uuid | 14/19 | 16/17 |

## Refactors

- bytes: one forward `varintFrom` gives (hi, lo, end), which replaces `varintEnd`, `varintValue` and `varintLow`.
- compression:
  - `memberStart` and `checked` match a tuple of the reads they make.
  - `chains` builds its keys with a rolling `keysFrom`.
  - `emitting` matches `raw.get`.
  - `copyBack` repeats a sliced window.
- proto:
  - The varint and fixed readers match `(in range, get)`.
  - `digitsValue` replaces seven `toInt().withDefault` calls on text whose digits are already checked.
- uuid: `parse` reads four 32-bit hex words with `word`.
- path: the `startsWith` and `globOne` checks that duplicated their `.None` arms are gone. `inGlobSet` reads its range ends as options.
- http, codegen, date and json: small matches in place of guarded reads.

## Bug fixed

An exported proto reader whose `end` ran past its bytes read zeros. It now answers `Truncated`. The test is in lib/proto/test/wire.buri.

## New tests

- indexset: IndexSet keys that share a hash, which drives the map's collision arms. The sets came from a search, `/tmp/p15-isets.py`.
- deflate: a gzip member whose file name never ends.
- path: unclosed and closed bracket globs.

## Left, and why

- No safe refactor:
  - compression `at`: one condition with about 20 callers.
  - uuid `octetOf`: it draws random bytes, and changing that could change seeded output.
- Unreachable today:
  - str 712: the exponent `toInt`.
  - json 405 `d > first`: `toFloat` rejects `1e`.
  - map 318: the `.None` arm of `removeNode`.
- Can't collide: Access keys (16 values under a 64-bit hash) and buri_ast's internal groupBy keys.
- Inherent: AllOne's hash is constant.
- Tests that could still be added:
  - compression 842: a stored block at a byte boundary.
  - compression 984: a bad symbol inside `runLengths`.
  - path 463: `withExtension` with no parent.
- Needs a compiler change: the phase 14 list, which includes an unchecked code point to `Char` conversion and a `fromUtf8` that can't fail on cleaned input.

## Stdlib MC/DC phase 17

Two private intrinsics now take out conditions that can never be false. Commit d1b409cd5 is on staging.

```buri
fn charOf(code: U32): Char;                          // str, json
fn fromUtf8Trusted<C: Allocator>(ctx: C, octets: [U8]): Str;  // bytes
```

- `charOf` turns the U32 into a Char with no check. Stencil and LLVM pass the value through unchanged, and JS uses `String.fromCodePoint`.
- `fromUtf8Trusted` decodes bytes that `cleaned` has already repaired. The native side is `buri_rt_bytes_from_utf8_trusted`, and JS uses `TextDecoder` with `ignoreBOM`.
- str uses `charOf` in `scalar`, which `valuesFrom`, `hangulParts` and `primaryComposite` all call. A script checked that every code in the NFD, NFKD, compose and fold tables is a valid scalar.
- json uses it in `unicodeEscape`, after the surrogate checks.
- bytes `fromUtf8Lossy` uses `fromUtf8Trusted`.
- buri/ast 5111 stays as it is. The lexer doesn't range-check `\u{...}`, so `parse` accepts `'\u{D800}'` and `scalarOf` answers `' '` for it. `charOf` there would build an invalid Char.

### MC/DC

| Module | Base | New |
|---|---|---|
| total | 9344/9703 (96.3%) | 9338/9690 (96.4%) |
| core/bytes | 110/112 | 110/111 |
| core/json | 224/227 | 222/223 |
| core/str | 250/259 | 246/251 |
| core/buri/ast | 905/920 | 905/920 |

### Compiler branches

| Area | Base | New |
|---|---|---|
| crates/backend | 68/68 | 68/68 |
| crates/js | 985/1018 | 985/1018 |
| crates/llvm | 551/552 | 555/556 |
| crates/middle | 1837/1874 | 1839/1876 |
| crates/stencil | 1067/1111 | 1069/1113 |
| crates/stdlib | 12/12 | 12/12 |
| total | 14837/15120 | 14845/15128 |

### Tests

Each test passes on base and new, on JS, stencil and LLVM release.

- `lib/json/test/parsing.buri` checks escapes at `퟿`, `￿`, `𐀀` and `􏿿`.
- `lib/data/test/unicode.buri` checks that the last Hangul syllable decomposes and composes back, and that U+1D15E decomposes under both Nfd and Nfc.
- `lib/text/test/bytes.buri` checks that a BOM and a NUL byte survive `fromUtf8Lossy`.

### Output comparison

All four modes pass 4694/4694 on `cli/tests/conformance`, base and new.

- **JS debug.** Every package file differs. Each one bundles the full runtime, which gained `$str_charOf`, `$json_charOf` and `$bytes_fromUtf8Trusted`. Each file also gets a new random test seed per run.
- **JS release.** Only the packages that use json, str or bytes differ: collections, data, generators, json, proto, proto_presence, text and web. In each, the `$toChar` check or the `fromUtf8` fallback is gone.
- **Tool files.** Their names are cache keys. The one file that is new or renamed has the same bytes as an existing one.
- **Native, str, json and bytes objects.** These differ in both modes, as expected.
- **Native debug, other objects.**
  - core/list differs only in renumbered `fused` symbol ids.
  - core/number and core/result keep the same symbol set.
- **Native release, other objects.** The one other difference is the large JSON-derive test object, which gains one symbol.

### Validation

- Full suite with `--features buri/backend-llvm`: green.
- `BURI_RT_HEAP_CHECK=1 cargo test -p buri-rt-tests`: 487 passed.
- `cargo fmt --all --check`: clean.
- Both clippy commands: only the existing warnings.

## Stdlib MC/DC phase 18

### MC/DC

| Module | Base | New |
|---|---|---|
| total | 9548/9893 (96.5%) | 9584/9880 (97.0%) |
| core/actor | 203/245 | 226/244 |
| core/tasks | 15/66 | 15/48 |
| core/time | 12/19 | 18/19 |
| core/net/tcp | 1/2 | 2/2 |
| core/result | 608/610 | 614/616 |

Every other module is unchanged.

### Refactor

`beside` and the inner `parallel` in `working` now hand `parallel` a list of two closures, so neither half asks which half it is. That takes out `match (isBody)` and `match (claimed < 0)`: 18 conditions, none of which a test could show.

### Tests

- **A step that sends to its own actor.** It gets `.Err(.WouldDeadlock)`, and its loop steps the message later. Each test actor's message type gained `Within(fn(C) => ())`, which runs inside the step. Covered for 12 of 13 actor types:
  - `actor/test/payloads.buri`: unit, list, text, record, desk and tally, and a job inside `tasks.scope`
  - `actor/test/counter.buri`, `actor/test/scoped.buri`
  - `data/test/steps.buri`, `tasks/test/scope.buri`, `semantics/test/host_testing.buri`
- **time.** `ui/test/reactivity.buri` has a watcher that formats a duration at every unit size. `Show` for a duration runs once per context, and the UI's `Scope` is its own context.
- **tcp.** `tcp/test/stream.buri` adds a test-written `Tcp` that refuses one host and connects another, so `connect` shows both arms in one context.
- **Equal for `Result<Ran, SendError>`.** The job test compares `Ok` and `Err` both ways, so the new instantiation shows every condition.

The touched packages passed 30 runs on JS and 20 runs natively.

### What's left

- **tasks: 33 conditions.** `scopeBeside` answers false in every test, both in the runtime (`in_a_test()`) and in JS `$tasks_scopeBeside`. So `beside`'s answer, `working`, `ran`, `resting` and `running` run only in a release program. Showing them needs a test hook that makes `scopeBeside` true under `TestTasks`. That changes `crates/js`, which is out of scope here, and the native runtime. The `scopeLeave`, `scopeEnter`, `taking .None` and `held.first .None` arms are races, or can't be reached at all.
- **actor 343 and 452: 2 conditions.** These are the dialler in `host_testing.buri`. Its `M = Str` can't carry a closure, so its step can't send to itself. Making it generic means rewriting the dialler's tests.
- **actor 491: 15 conditions.** `discarding .Some` needs a stop to land while messages wait. That needs a native race, and a `mailboxClose` from inside a step blocks for up to 30 s.
- **actor 389: 1 condition.** `unanswered .Ok(())` is a race, or can't be reached.
- **check: 2 conditions.** A false property is `forAll`'s failure path, so no passing test shows it.
- **time 224: 1 condition.** The division trap stays.

## Stdlib MC/DC phase 19

### MC/DC

| Module | Base | New |
|---|---|---|
| total | 9584/9880 (97.0%) | 9727/9958 (97.7%) |
| core/orderedmap | 1136/1203 | 1157/1203 |
| core/net/server | 231/256 | 335/336 |
| core/indexset | 235/245 | 241/245 |
| core/decimal | 60/65 | 63/65 |
| core/net/websocket | 18/23 | 23/23 |
| ui/theme | 18/24 | 22/22 |

Every other module is unchanged. A script checked every module against `coverage/stdlib-mcdc.txt`, and none dropped.

### Refactor

In ui/theme, `valueText` now writes a token as `var(--name)`, and `.Page` calls it directly, so `colorText` is gone. Its old `.Token(_) => ""` arm couldn't be reached.

### Tests

- **Watchers that show a value.** A UI watcher's `Scope` gets its own copy of `Show`. Build the value outside the watcher with `context { Allocator: alloc() }`, or the watcher makes new, partly-shown copies of `of`.
  - `collections/test/orderedmap.buri`: a map of eight entries, which has split, for seven key and value types.
  - `collections/test/indexset.buri`: a set that straddles a word.
  - `bignum/test/decimal.buri`: a price of each sign and scale.
- **Theme.** `ui/test/theme.buri` binds tokens to `Rgba`, `Transparent`, `Inherit` and a faded token, and has one binding whose left half isn't a token.
- **Server in an allocation scope.** `semantics/test/effects.buri` runs every socket-server case inside `core/alloc`'s `scoped` as well. It adds the tuned knobs, a server with no websocket path, and a request whose listener has closed.
- **Dialler endings.** `semantics/test/host_testing.buri` adds a test-written `WebSocketClient` that refuses one URL and ends sockets abnormally, with an overflow, with a custom code, or normally.

### What's left

- **orderedmap: 46 conditions.**
  - The `C = Scope` copies for `(I64, Maps)`, `(I64, Tree)`, `(Str, Address)` and `(Str, Value)` need a watcher in the proto and memory packages, as for the builtin types. That's about 12 conditions.
  - `Bool`/`Mood` and the map collisions can't be reached.
- **map: 35.** Collisions for keys that can't collide, and 318 `.None` (the key is known present).
- **tasks: 33.** Unchanged; see phase 18 and the queue note.
- **server 532: 1.** The `MAX_HANDLERS` clamp.
- **ui/node 19, buri/ast 8, compression 6, str 5, random 5.** Not looked at.
- **number 15, simd 8.** Traps.

## Stdlib MC/DC phase 20

### MC/DC

| Module | Base | New |
|---|---|---|
| total | 9727/9958 (97.7%) | 9735/9941 (97.9%) |
| core/orderedmap | 1157/1203 | 1170/1203 |
| ui/node | 761/780 | 761/772 |
| core/buri/ast | 1109/1117 | 1105/1111 |
| core/str | 246/251 | 245/248 |

Every other module is unchanged. The gate compares shares, not counts. buri/ast and str show fewer conditions because some that the refactors removed were shown. A script checked every module against `coverage/stdlib-mcdc.txt`, and no share dropped.

### Refactors

- **buri/ast.**
  - `leadingImports` collects `(importKey, item)` pairs while it matches `.Import`. So `isImport` and `importKey`'s unreachable `_other` arm are gone.
  - `needsParens` no longer exempts a bare block. That exemption was a bug: `({ 1 }).abs()` printed as `{ 1 }.abs()`, which doesn't parse.
- **str.**
  - `equalsIgnoreCase` drops its length precheck, so `sameFolded`'s `_` arm is reachable.
  - `composeFrom` carries the starter character, so it no longer reads it back through an `Option`.
- **ui/node.** The compiler turns every `On`, `At` and `VisuallyHidden` into classes before a node holds it (`semantics::styles`; a non-static `On`/`At` is `style-not-static`). So their arms in `stylesReactiveAt`, `resolve` and `declare` are gone.

### Tests

- **Watchers that show a map that has split.**
  - `memory/test/boxes.buri`: a `Tree` with eight kids.
  - `proto/test/maps.buri`: `Maps` with eight places and eight nested maps, and a `Struct` with eight fields.
- **A block heading a chain.**
  - `buri_ast/test/expressions.buri` prints a bare block before `.length()`.
  - `buri_ast/test/reading.buri` reads `({ 1 }).abs()` and `({ f })(2)` back.

### Compiler bug, not fixed

`buri format` refuses a file that holds `({ -1 }).abs()`. It reports that the file "does not parse", although `buri test` compiles and runs it. The formatter probably prints `{ -1 }.abs()`, the same bug buri/ast had.

### What's left

- **map: 35.** Collisions for keys that can't collide, and 318 `.None`. No buri-level refactor removes them without an intrinsic in `crates/middle`, or merging Entry and Collision at a cost to performance.
- **orderedmap: 33.** `Bool`/`Mood`, and the `Str`/`[Entry]` codegen copies.
- **tasks: 33, actor: 18.** Unchanged; see phase 18.
- **number: 15, simd: 8.** Traps.
- **ui/node: 11.**
  - `nodeLines`' `.Raw` and `.Fragment` arms: only `bindDynamic` builds these, and `describe` never walks it.
  - 879 `artworkOf` and the `each` `.None` arms at 1796 and 1802 can't be reached.
  - 4113 `painted.checked .Some` and 4423 `slotAgain`, both for C = Scope, need a reactive toggle, or a reactive element with two pairs in one slot.
- **buri/ast: 6.**
  - 889 `derivedName _other`, and the `.None` arms of `elseLinks`/`elseForces`, are reached only by a hand-built tree.
  - `digitAt` and `floatOf` are traps or reads that can't fail.
- **compression: 6.** Shift traps and `at()`'s `withDefault`.
- **random: 5.**
  - The `%` traps at 179 and 180.
  - Exchange `_` at 453.
  - `radius == 0.0` at 467.
- **str: 3.** 712 `withDefault`, and `digitAt` at 1224 and 1225.

## Stdlib MC/DC phase 21: core/tasks

### MC/DC

core/tasks went from 15/48 to 33/44, measured over the tasks, semantics, collections and actor packages, merged across stencil, JS and LLVM `--release`.

### Runtime changes

- **An empty round gives the drain up.** `scopeRound` clears `draining` under the same lock when it finds nothing waiting, in `rt.rs` and in `runtime.js`. So a spawn can no longer race the drain's end, and `scopeLeave` and `resting` are gone.
- **Only a `TestTasks` block runs body-first.** `scopeBeside` used to answer false anywhere in a test binary. Now it answers false only while the current test block has made a `TestTasks` double (`testing::a_double_schedules`). A block that binds its own `Tasks` runs scopes as its program does.

### Tests

- `cli/tests/native/own_scheduler.rs` uses a scheduler that runs three items in reverse. It checks that `buri run` and `buri test` print the same order. It failed before the gate change.
- Conformance siblings under a local `InOrder` scheduler, one per answer type:
  - `tasks/test/scope.buri` and `background.buri`
  - `collections/test/flags.buri` and `indexset.buri`
  - `actor/test/payloads.buri`

### What's left (11)

- **314 `_` of `halves.first()`, once per A (8).** Unreachable, because `parallel` answers in order. Pulling an `A` out with no `match` needs a default. A lazy default captures `ctx`, which SPEC 10.6 forbids, and an aborting unwrap only moves the decision into `core/option`.
- **337 `scopeSpare true`.** It needs a worker parked in `scopeClaim` while another claims. A sequential scheduler deadlocks or never parks, so this needs real threads.
- **401 `taking .None` and 404 `held.first .None`.** Unreachable, because every index is taken exactly once. If `scopeTaskAt` returned `[Carried<T>]` (empty where taken) instead of `Option`, one of the two would go. That means changing `rt.rs`, `runtime.js` and `runtime_table.rs` for a single decision, so it wasn't done.

## Stdlib MC/DC phase 22

### MC/DC

Measured with the whole gate on aarch64-apple-darwin. The total went from 9814/9998 (98.2%) to 9824/9997 (98.3%).

| Module | Before | After |
|---|---|---|
| core/buri/ast | 1105/1111 | 1106/1111 |
| core/codegen | 25/26 | 26/26 |
| core/io | 4/5 | 5/5 |
| core/json | 222/223 | 222/222 |
| core/list | 281/282 | 282/282 |
| core/net/server | 335/336 | 336/336 |
| core/path | 93/94 | 94/94 |
| core/result | 614/616 | 616/616 |
| std/codegen/proto | 344/347 | 346/347 |

Every other module is unchanged, core/tasks included (33/44).

### Refactor

- **json.** `grammatical` no longer checks that an exponent has a digit. `toFloat` refuses `1e`, `1e+` and `2.5E-`, so a number like that never gets there.

### Tests

- `generators/test/failure.buri`: a test-written `Stdout` that fails every write. `codegen.run` answers the error.
- `streams/test/io.buri`: a test-written `Stdin` that answers `.Some([])`. `readAllBytes` treats it as the end.
- `memory/test/scoped.buri`: a path from the root, read inside `alloc.scoped`.
- `semantics/test/effects.buri`: a listener that claims 5000 handlers runs 1024. `SerialTasks` runs three items at most, so this case uses the testing `tasks()`.
- `proto_gen/test/entry.buri`: a schema at the repository root, and `upperFirst("")`.
- `buri_ast/test/items.buri`: a derive about `[Id]` stays where it was written. `tokens.buri`: windows of size 0.
- `data/test/optionresult.buri`: `Result<Int, Str>` shown as `.Ok(7)` and `.Err("no")`.
- `json/test/parsing.buri`: `[2.5E-]` and `0E` are bad numbers.

### What's left

- **tasks: 11.** Unchanged; see phase 21.
- **ui/web 154 `sheet == ""`.** The stylesheet belongs to the whole test binary, and `web/test/document.buri` styles two nodes. Showing it needs a package of its own that styles nothing.
- **ui/style/classify 318 `declared`'s `_`.** It would go if `atomsOf` matched each property itself, but that moves about 55 arms for one condition. 234 `.Extracted` and the `.all` built-in at 722 stay.
- **proto/wrappers 695.** `writeBoolValue`'s `if (value.value)` runs only when the value is true. The file is generated, so the fix belongs in `writeRaw` in `std/codegen/proto`, which would have to tell an implicit-presence field from an optional or packed one.
- **Reads that can't fail:** `digitAt` in character, str and buri/ast; `alphabetIndex` in bytes; `octetOf` in uuid; str 712; and compression 463.
- **Unreachable:** random 453 (`exchange`'s `_`) and 467 (`radius == 0.0`); buri/ast 1707 and 2011 (an `if` with an empty else list); buri/ast 6278 `floatOf .None`; std/codegen/proto 3297 (a well-known source that doesn't parse).
- **Traps:** number 15, simd 8, compression 5, indexset 4, bigint 2, decimal 2, proto 2, time 1.
- **map 35, orderedmap 33, actor 18, ui/node 11.** Unchanged; see phases 18 to 20.

## Stdlib MC/DC phase 23

### MC/DC

Measured with the whole gate on aarch64-apple-darwin. The total went from 9824/9997 to 9844/10000 (98.4%). No module fell.

- core/tasks: 33/44 to 40/40
- core/uuid: 16/17 to 26/26
- core/bytes: 110/111 to 107/107
- core/str: 245/248 to 247/248
- core/character: 20/22 to 21/22
- core/buri/ast: 1106/1111 to 1107/1111
- core/list: 282/282 to 284/284, from new instances that are covered

### Decisions removed

- **tasks `taking .None` and `held.first .None`.** `scopeTaskAt` now writes a one-element block, or an empty one where the task is taken, so `running` is a `foldCtx`. Every index is taken exactly once, so neither arm could run. Changed in `rt.rs`, `runtime.js` and `runtime_table.rs`.
- **uuid `octetOf`'s `withDefault`.** `stamped` and `version7` walk the sixteen octets with `mapIndexed`. `randomBytes(ctx, 16)` always answers sixteen. The output is byte-for-byte the same, and the pinned texts in `uuid/test/uuid.buri` still pass.
- **bytes `alphabetIndex .None`.** `sextetsFrom` checks and looks up each character in one walk, so decoding never looks up a character it was told is there. `inAlphabet` is gone.
- **`digitAt`'s `toDigit(36).withDefault(0)` in character, str and buri/ast.** The tables hold only `0`-`9` and `a`-`z`, so a digit is its code minus 48 or 87. The `code < 97` split replaces it, and both sides run.
- **str 712's `toInt().withDefault(0)`.** `fromFloat` always writes the exponent as a sign and digits, so `exponentOf` folds the digits by code. Both signs run (`6e-7` and `1e21` in `data/test/strings.buri`).

### Covered

- **tasks `_` of `halves.first()`, once per A (8).** It's reachable: a program's own scheduler can break `parallel`'s promise and answer nothing. An `Idle` scheduler that answers `[]` covers each answer type in `scope.buri`, `background.buri`, `flags.buri`, `indexset.buri` and `payloads.buri`. The `_` arm runs the body on the scope's own thread.
- **tasks `scopeSpare true`.** The runtime counts a worker as free from when it's forked until it claims a task, and again after it finishes one. So a sequential scheduler with two spawns reaches it. `cli/tests/native/side_by_side.rs` checks that every task runs beside the body when workers are free.

### What's left

There's no indexed read in Buri that can't answer `.None`, so these need a language change or a different AST shape:

- **`digitAt`'s `charAt .None`** in character, str and buri/ast.
- **compression 463 `at`.** 23 call sites index fixed tables and sort keys.
- **random 453 `exchange`'s `_`** (I64 and Str). A shuffle swaps two keys of an `OrderedMap`, and both reads answer `Option`.
- **buri/ast 1707 and 2011 `otherwise.first() .None`.** The else branch is a one-element list. A `fold` would cost a stack frame per `else if` link.
- **buri/ast 6278 `floatOf .None`** and **std/codegen/proto 3297** (`ast.parse` of a fixed source). Both parse text that always parses.

**random 467 `radius == 0.0`** is a real guard against `ln(0)`. It needs two draws of exactly 0.5, and there's no public way to build that generator.

## Stdlib MC/DC phase 24

### MC/DC

Measured with the whole gate on aarch64-apple-darwin. The total went from 9835/9991 to 9828/9959 (98.7%). No module fell.

- core/actor: 226/244 to 210/212
- core/orderedmap: 1182/1215 to 1191/1215

### Decisions removed

- **actor 389 `unanswered .Ok(())`.** `drive` now answers the `SendError` itself, so `sendMessage` no longer asks a second time why the drive left nothing. Where the drive ran, the answer is there.
- **actor 491 `discarding .Some`, once per instantiation (15).** `stop` used to pop leftover messages one at a time, and only a race with a send could leave one. The new `actor.mailboxRest` intrinsic takes every waiting message in one list and gives the room back, so `stop` has no decision. Changed in `rt.rs`, `runtime.js`, `runtime_table.rs`, `monomorphize.rs`, and the lists in `park.rs`, `rc.rs` and `actor_cross.txt`.

Both new tests pass on the old code too:

- `actor/test/counter.buri`: "a stop after a step's own sends finds each of them stepped, in order".
- `native/agreement.rs`: `a_stop_racing_sends_lets_go_of_what_it_finds_waiting`. On JS every send lands in the mailbox before the stop, so the non-empty rest is dropped every time.

`rt.rs` has the unit test `a_stop_before_a_drive_takes_every_message_still_waiting`.

### Covered

- **orderedmap's (Str, Entry) and (Str, [Entry]) splits.** Two new tests in `proto_gen/test/failures.buri` use two packages that each declare 60 messages. A field reaching each short name gets an ambiguity report for every one, in field order. A field reaching each name in full gets no report.

### What's left

- **actor 343 and 452** for the dialler (S = I64, M = Str, R = Bool). Its step can't reach its own address. Changing the types would only avoid the instantiation.
- **map, 35.** These are collisions for keys that can't collide: Access, I64/[(I64, I64, Item)], IndexSet<Slot> and AllOne. AllOne's hash is constant, so its branch paths can't run. 318 `.None` can't run either, because `remove` asks `has` first with the same pure hash. Folding it would break the in-place `removeAt`. 91 and 96 are shift traps.
- **orderedmap, 24.** All of them are K = Bool, V = Mood. A map with two keys never splits.

## Stdlib MC/DC phase 25

### MC/DC

Measured with the whole gate on aarch64-apple-darwin. The total went from 9828/9959 to 9833/9959 (98.7%). No module fell below `coverage/stdlib-mcdc.txt`.

- core/map: 581/616 to 585/616
- core/orderedmap: 1191/1215 to 1192/1215

### Decision removed

- **map 91, `chunkAt`'s shift by `depth * BITS`.** `depth` is at most 12, so the count never reaches 64. A descent now carries the hash already shifted past the levels above, and each level shifts it by the constant `BITS`, which isn't a decision. Only a leaf that makes room asks `shiftedAt(h, depth)` for its own hash, and its `depth == 0` runs both ways. The `maps` bench program retires about 0.6% fewer instructions natively.

### Covered

- **map, (IndexSet<Slot>, Str) and (IndexSet<Slot>, ()).** `collections/test/indexset.buri` looks `beside` up and stores it in a map holding only `a`, so it meets a lone entry with a different hash. `set.of(ctx, [a, beside])` does the same for a set.
- **orderedmap 999 `walkLeaf .Some` (Bool, Mood, C = Scope).** The watcher in `proto/test/maps.buri` shows a non-empty `moods`.

### What's left

- **map, 31.**
  - 112 `bitOf`'s shift: the chunk is masked to 0..31, but the count is a variable. A shift-free `2^chunk` would cost every lookup, so it stays.
  - Access (15 conditions): all sixteen values have distinct low five bits, so two keys never share a slot, and none share a hash.
  - (I64, [(I64, I64, Item)]) in buri/ast's `printDecls`: an `I64` that fits 32 bits hashes one-to-one, and the keys are declaration indexes.
  - AllOne: its hash is constant, so `h == hash` never fails and the root never holds two slots.
  - 338 `removeNode`'s `.None` (6): `remove` asks `has` first with the same hash.
- **orderedmap, 23.** All K = Bool, V = Mood. A map with two keys never splits or branches.

## Stdlib MC/DC phase 26

### MC/DC

Measured with the whole gate on aarch64-apple-darwin. The total went from 9919/10048 (98.7%) to 9922/10046 (98.8%). No module fell.

- ui/node: 761/772 to 761/770
- ui/style/classify: 223/226 to 223/225
- ui/web: 2/3 to 3/3
- core/proto/wrappers: 81/82 to 81/81
- std/codegen/proto: 346/347 to 348/349, from the new `writeSet` match, both arms shown

### Decisions removed

- **ui/node 4113 `painted.checked .Some` for C = Scope.** `widgetBodyUnder` is the only `Scope` caller of `answers`. Its painters are the tooltip bubble (`plain`), `emitBox` (`plain`), `answering` and the `hasFocus` copy of a box, and each one sets `checked: .None`. A toggle or radio rebuilds its region and paints with `widgetBody`. So `widgetBodyUnder` now calls `stateAnswers` (disabled and focused) and skips the `checked` match.
- **ui/style/classify 722, `.all(fn(piece) => piece.isEmpty())`.** `splitAny` drops empty pieces (`buri_rt_str_split_any` and `$str_splitAny`), so the predicate could never answer true. `plain` now asks `.isEmpty()` of the pieces, which means the same thing.
- **core/proto/wrappers 695, `if (value.value) { 1 } else { 0 }`.** It runs only after `value.value == false` failed. `codegen_proto` gained `writeSet`, which writes an implicit-presence bool as `1`. `proto_wrappers.buri` was changed by hand to match, and so was the pinned text in `proto_gen/test/scalars.buri`.

### Covered

- **ui/node 4423 `slotAgain` for C = Scope.** `ui/test/reactive.buri`: a box with a constant `.Width(.Px(1))` and a `.When` holding `.Width(.Px(3))`. Both pairs share a slot under the watcher, and the later one wins.
- **ui/web 154 `sheet == ""`.** The new package `lib/web_unstyled` builds no node, so the compiler extracts no stylesheet, not even the reset. Its `shell` has no `<style>`. It's JS-only: `backends: [JS]`, `excluded` in `native/conformance.rs`, and `JAVASCRIPT_ONLY` in the gate.

### What's left

- **Traps:** number 15, simd 8, compression 5, indexset 4, bigint 2, decimal 2, proto 2, time 1, random 2. A test can't expect an abort.
- **core/testing/check 198 and 217.** A failing property fails the test, so no passing test can show it.
- **ui/node, 9.** 879 `artworkOf _` (`semantics::icons` refuses a non-constant source), the `each` `.None` arms at 1796 and 1802 (6: the runtime asks only for indexes below the length), and 3527 `.Raw`/`.Fragment` (only `bindDynamic` builds them, and `describe` never walks it).
- **ui/style/classify 234 `.Extracted` and 318 `declared`'s `_`.** See phase 22.
- **Reads that can't fail:** `digitAt`'s `charAt` in character, str and buri/ast; compression 463; random 453; buri/ast 1707, 2011 and 6283; std/codegen/proto 3297.
- **random 467 `radius == 0.0`.** It needs two draws of exactly 0.5.
- **map, orderedmap and actor** were out of scope here (see phase 25 and the actor design decision).
