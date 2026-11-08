# JS runtime CPU handoff

Phase 1 landed three runtime changes and one codegen change. Instructions retired under Bun, single process, every test:

| Suite | Before | After |
|---|---|---|
| data | 16.13G | 14.55G |
| crypto | 25.62G | 23.43G |
| collections | 20.75G | 19.70G |

The whole conformance corpus on JS went from about 115.1G to 111.3G.

- `$starts` remembers the last two strings asked about, short or long. `graphemeCount` asks about its string and a table in turn, and short strings were checked with a regex on every `charAt`.
- `$share` reads `$u` first, so a list costs one property read.
- `$toChar` builds a scalar below the surrogates with `fromCharCode`.
- `intrinsics.rs` passes a `bits.shift*`/`bits.rotate*` count as a `number` when it is a literal or a widened `U32`. `Number(bigint)` cost about 100 instructions a call.

## Left

- `$list_push` copies about 5% of samples. `core/orderedmap`'s `leaves` shares `above[0]` because the parent is read later, so each push copies. That is `rc` work in `crates/middle`.
- `$own` and `$shareEach` are another 3–5%.
- Module parse is about 8% of a run.
- `lib/data/test/unicode.buri:2352` fails on Bun (17085 vs 17086) on base and head alike, likely Bun's Unicode version.

## Measuring

- Count with `/usr/bin/time -l` ("instructions retired") and `BUN_JSC_useConcurrentJIT=false`; counts then repeat to about 0.1%.
- `buri test` clears the env for its runner, so set that variable inside the `BURI_JS` wrapper.
- `bun --cpu-prof-dir` needs a relative path that already exists.
- Sharded runs move tests between processes from run to run, so compare single-process runs of a suite's `test-library.mjs`, fed test indexes on stdin.
