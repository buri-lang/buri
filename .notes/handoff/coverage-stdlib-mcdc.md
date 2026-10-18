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
# Stdlib MC/DC phase 16

Total went from 9343/9723 (96.1%) to 9344/9703 (96.3%). Unshown conditions fell from 380 to 359. Every module held or rose. The figures are aarch64 runs on all four backends.

| Module | Before | After |
|---|---|---|
| core/alloc | 2/3 | 3/3 |
| core/compression | 105/113 | 107/113 |
| core/date | 184/185 | 185/185 |
| core/decimal | 61/67 | 60/65 |
| core/math | 42/43 | 43/43 |
| core/net/websocket | 17/23 | 18/23 |
| core/option | 740/741 | 739/739 |
| core/path | 94/96 | 93/94 |
| core/proto/struct | 103/104 | 104/104 |
| core/result | 608/612 | 608/610 |
| core/set | 12/13 | 13/13 |
| ui/node | 767/792 | 761/780 |
| ui/prop | 13/15 | 15/15 |

## Refactors

- path `withExtension` prefixes the new name with the text before the old one. A path with a file name always has a parent, so `parent()`'s `.None` arm was dead. Phase 15's "withExtension with no parent" test can't exist.
- decimal `dropDigits` reaches its halfway case only at `drop == 19`, where `5 * 10^18` fits, so `checkedMultiply(5)` is a plain `* 5`.
- ui/node `variableInline` is `take(1)` plus `split(":")`. Every declaration it gets has a colon, so both `.None` arms (times three contexts) were dead.

## Tests

- deflate: a stored block whose header ends on a byte boundary, and a dynamic block whose lengths use a code the code-length code lacks.
- `acosh(3.0)`, an RFC 850 date with a four-digit year, `distinctBy` by length with a repeat.
- Stats that differ only in requests, `Show` of `NullValue.Unrecognized`, a struct that encodes to JSON.
- A binary frame on the `Str`-state resuming client, and `each` over `.Const` and `.Cell` lists of numbers.
- Two test-only instantiations went: `guards.buri` uses `assert.some` and `document.buri` uses `assert.ok`.

No bugs found.

## Needs a compiler change

Both fit as private intrinsics, the way `web.stylesheet()` is a private `fn ...;` today. Nothing new is public.

- **A checked code point to `Char`.** Each module declares a private `fn charOf(code: U32): Char;` and calls it where a range check already ran. The backends lower it to the `U32` itself, since `Char` is a `U32` scalar. It removes 14 conditions, 7 of them unshown:
  - str: `scalar` (1233), and the three matches on its answer at 1298, 1353 and 1359;
  - json: the two `toChar` matches in `\u` escapes (304, 317);
  - buri/ast: `scalarOf` (5111).
- **A UTF-8 decode that can't fail.** bytes declares a private `fn fromUtf8Trusted<C: Allocator>(ctx: C, octets: [U8]): Str;` for `fromUtf8Lossy`, whose `cleaned` octets are valid by construction. The backends reuse `fromUtf8`'s string building without its validation. It removes 1 condition, unshown (bytes 108).

## Left, and why

- No safe refactor:
  - str 1415 `acc.get(starter)`: carrying the starter's `Char` adds state for one condition.
  - proto/wrappers 695 `value.value`: the generator writes `if (v) { 1 } else { 0 }` for every bool, so fixing it means a special case in codegen_proto plus new goldens, for one condition.
  - ui/node `each`'s `.None` arms: `Each` would need to carry keys instead of indexes.
- Unreachable with the test doubles: io 124 (an empty chunk), tcp 91 (`tcpConnect` never fails), websocket 241 (`connectReceive` after a close), websocket 205 for `S = Str` (the resuming client's URL is fixed).
- Needs a second web test package with no styles: web 154 `sheet == ""`.
- Only a failing property reaches check 198/217.
- Shifts by a variable count: compression 482, 483, 498, 757 and 777, simd, indexset and map.
- Out of scope, as before: `C = Scope` copies, `answering`/`taking`, orderedmap `Bool`/`Mood`, traps with a parameter divisor, and uuid `octetOf`.
