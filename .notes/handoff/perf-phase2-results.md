# Perf phase 2 results: cold build and test of /path/to/workspace

Measurements only. Baseline is buri 3802141cd (phase 1's), after is 2f29c02d2 (`origin/main`). Both are release builds with `--features backend-llvm`, on the 12-core M3 Pro (6 performance, 6 efficiency cores).

## Method

Same as `perf-phase1.md`, with these scripts (all in `/tmp/perf2`, deleted after):

```sh
rsync -a --delete /tmp/code-perf2-src/ /tmp/code-perf2/   # pristine copy, drops .buri and out
rm -rf $BURI_HOME $XDG_CACHE_HOME                          # project-cold keeps $BURI_HOME/cross
RUSTC=$R/rustc CARGO=$R/cargo BURI_HOME=... XDG_CACHE_HOME=... BURI_PROFILE=1 \
  /usr/bin/time -l buri build //...
```

- Each timed run waited for a 1-minute load under 3. Runs alternated baseline and after, three rounds.
- Warm runs followed the first cold run at once, so their load is high.
- A Python poller ran `ps` every 0.5 s over buri's process tree. It gives the summed tree RSS and the count of test processes (`test-library`, `test-host`, `test-runner`, and `bun` once tests start).
- `pmset -g log` shows no sleep between 05:18 and 06:47.

## Timings (wall seconds, load in brackets)

| Run | Baseline runs | Baseline | After runs | After | Change |
|---|---|---:|---|---:|---:|
| `build //...` | 46.63 [2.80], 46.02 [3.05], 45.87 [2.93] | **46.0** | 34.10 [2.87], 33.33 [2.98], 33.36 [2.99] | **33.4** | −27% |
| `build --release //...` | 178.09 [2.96], 177.64 [2.95], 177.90 [2.97] | **177.9** | 97.09 [2.86], 97.07 [2.95], 96.85 [2.84] | **97.1** | −45% |
| `test` | 299.71 [2.97], 299.61 [2.98], 299.26 [2.96] | **299.6** | 201.63 [2.96], 201.45 [2.78], 201.59 [2.95] | **201.6** | −33% |
| `build //...`, cross runtime cached | 16.78 [2.99], 16.71 [2.99], 16.69 [2.79] | **16.7** | 11.09 [2.84], 11.04 [2.95], 11.07 [2.81] | **11.1** | −34% |

Warm, once each:

| Run | Baseline | After |
|---|---:|---:|
| `build //...` | 2.29 [3.64] | 1.41 [3.79] |
| `build --release //...` | 2.22 [10.66] | 1.35 [33.46] |
| `test` | 0.23 [6.37] | 0.23 [14.81] |

Peak memory, highest of the three cold runs. "buri" is `BURI_PROFILE`'s peak footprint; "tree" is the polled sum over buri and its children.

| Run | Baseline buri | After buri | Baseline tree | After tree |
|---|---:|---:|---:|---:|
| `build //...` | 0.72 GB | 1.40 GB | 3.7 GB | 6.0 GB |
| `build --release //...` | 1.11 GB | 4.89 GB | 3.7 GB | 6.1 GB |
| `test` | 5.94 GB | 5.77 GB | 12.7 GB | 13.0 GB |

Parallel program builds hold up to 12 programs at once, so release peak memory went up 4.4x.

## Cold `buri test`

| | Baseline | After |
|---|---:|---:|
| Tests start | 11.8–13.0 s | 11.3–12.9 s |
| Tail: last time 12 or more test processes ran, to the end | 67.7–68.8 s | 20.7–22.9 s |
| All seconds with fewer than 12, after tests start | 72.8–75.3 s | 34.1–39.3 s |
| Most test processes at once | 12 | 20–21 |
| Test child CPU | 2271 s | 1759 s |
| Sum of test durations | 2766 s (phase 1) | 2364 s |

Slowest suites, after (`--verbose`, cold run at 20 processes, so relative only):

| Suite | Time | Tests | Longest |
|---|---:|---:|---:|
| //libs/store/simulator/cluster | 1437 s | 69 | 152.4 s |
| //libs/store/server/node | 393 s | 308 | 120.0 s |
| //libs/store/simulator/protocol | 102 s | 82 | 31.6 s |
| //libs/store/server/registry | 78 s | 207 | 12.1 s |
| //libs/store/client/online | 65 s | 295 | 10.4 s |
| //libs/store/simulator/raft | 61 s | 25 | 13.8 s |
| //libs/authorization | 53 s | 162 | 14.6 s |
| //libs/finance (JS) | 36 s | 216 | 1.7 s |

- Node's 393 s is 390 s of eight applier.buri tests waiting on 30 s timeouts. They no longer hold a slot.
- The run's critical path is cluster `run.buri` "billing changes to hundreds of tenants split over commits under faults". Its process ran from 47.9 s to 200.0 s, and the run ended at 201 s. Alone it takes 94.6 s.
- It starts late because a cold run ranks jobs by test count. Cluster's 69 tests rank below node, server, online and registry.

## Profiles

Three nodes keep every invariant over 12 seeds (21.5 s alone, 15.3k samples):

- ordered map (`lookup`, `seek`, `insertInto`, `map_insert_at`, `map_replace_at`): 16.3%
- retain/release glue: 16.0%, plus 4.4% other glue (`elems`)
- `Sim_memberOf` and its closures: 7.2%
- memmove 6.6%, memcmp 3.8%; memmove's callers that unwind are mostly ordered map `lookup` and `insertInto`
- alloc/free 3.5%, `core_bytes` varint and framing 3.3%
- testing filesystem: 0.4% (was 18%); crc32c: gone (was 10.7%)

Billing changes to hundreds of tenants (95 s alone, 45.5k samples):

- retain/release glue: 30.5%, plus 4.1% other glue
- database engine planning (`mostSpecific`, `boundVariables`, `appendTermVariable`, `appendUnique`, `runPlanSeeing`): 21.6%
- `core_list$contains`: 5.3%, with `$derive$eq$Attribute` beside it
- memmove 6.8%, alloc/free 6.2%, memset 2.4% (from `alloc_zeroed`)

Most stencil frames don't unwind, so caller attribution is partial.

Cold release build, `BURI_PROFILE=1`:

- emit is 6,975 G instructions and 799 s of 811 s buri CPU. The cross runtime adds 122 s child CPU.
- 2028 objects, 32 links, 7281 modules loaded.
- 933 s of CPU on 12 cores packs into 78 s; actual is 97 s.
- The cross runtime's cargo runs from 0.1 s to 85.7 s, competing with emit.

Cold debug build: the cross runtime's cargo runs from 1.1 s to 31.6 s of 33.4 s. Everything else is done by 11.5 s. Generators (bun) run from 0.1 s to 7–10 s in every cold command.

## Opportunities, ranked by cold saving

1. **Take the cross runtime off the debug critical path.** It's 30.5 s of 33.4 s. Building only the features a program uses (phase 1's #3: no `paint`), or a cheaper cross profile for debug outputs, could bring debug cold near the 11 s project-cold time. Saves up to ~20 s debug cold and ~5–8 s release cold (95 s of CPU leaves emit's way). Files: cli/src/build/runtime_cross.rs, cli/runtime/manifest.toml.
2. **Start the slowest suite first on a cold run.** The critical-path test starts at 47.9 s. Started at 12 s it would end near 165 s, close to the 159 s CPU floor (1759 s / 12 + 12 s build). Saves ~35 s cold test. Rank by something that puts cluster `run.buri` first, such as summed test source length, and check that it does before trusting it. Files: cli/src/commands/test.rs (`queue_members`, `starts_of`).
3. **Cut retain/release.** It's 30.5% of the critical-path test and 16% of the three-nodes test. Halving it saves ~130 s of test CPU and ~20 s off the critical-path test, so ~15–20 s cold test. Look at borrowed parameters through `core_list$contains` and the planner's closures. Run `BURI_RT_HEAP_CHECK=1 cargo test -p buri-rt-tests`.
4. **Share release emit across programs.** Emit is 99% of release CPU over 2028 objects. Compiling shared stdlib and library units once, or keeping phase 1's unoptimized IR for the optimizer instead of lowering twice (perf-2i), saves ~20–40 s release cold. Files: cli/src/build/actions.rs (`codegen_key`, unit prefix), crates/llvm.
5. **Speed up the planner's stdlib calls.** Planning is 21.6% of the critical-path test; `core_list$contains` with derived `eq` on `Attribute` is 5.3% alone. A compiler-side win: derived `eq` and `contains` that compare without retaining each element. Saves ~5–10 s off the critical path.
6. **Overlap generators with compiling.** Bun runs for the first 7–10 s of every cold command, and tests start at 11–12 s. Saves ~5–7 s on each cold command. Files: cli/src/build/generators.rs.
7. **Ordered map without copies.** 16.3% of the three-nodes test, and `lookup` shows up as a memmove caller, which a read shouldn't be. Saves ~5 s cold test. Files: crates/stdlib/src/compiler/standard_library/sources/orderedmap.buri, `buri_rt_map_insert_at`.
8. **Skip zeroing fresh allocations.** alloc/free plus `alloc_zeroed`'s memset are 8.6% of the critical-path test. Zero only what the program reads before writing. Saves ~3–5 s cold test. Files: cli/runtime/memory.rs.

Watch memory: parallel release builds now peak at 4.9 GB in buri alone.
