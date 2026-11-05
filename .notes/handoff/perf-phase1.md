# Perf phase 1: cold build and test of /path/to/workspace

Measurements only. Nothing is optimized yet. buri 0.3.36 (worktree HEAD ef15c7e81), release build with `--features backend-llvm`, 12-core M3 Pro.

## Repro

```sh
cp -R /path/to/workspace /tmp/code-perf     # never measure in the original
R=/nix/store/b0b2wcbdnv1ipp8ir5cciwmwxjnqg3gy-rust-minimal-1.99.0/bin   # has musl std; rustup stable doesn't
cd /tmp/code-perf
rm -rf .buri out /tmp/perf/home /tmp/perf/xdg           # cold
RUSTC=$R/rustc CARGO=$R/cargo BURI_HOME=/tmp/perf/home XDG_CACHE_HOME=/tmp/perf/xdg \
  BURI_PROFILE=1 /usr/bin/time -l buri build //...      # or: build --release //..., test
```

Caches a cold run must clear:

- `<project>/.buri/` (cache, link, out) and `<project>/out`
- `$BURI_HOME` (default `~/.buri`): cross, test-host, programs, link-replay, linker-identity
- `$XDG_CACHE_HOME/buri/exe-identity` (default `~/.cache/buri`)

"Project-cold" below keeps a `$BURI_HOME` that already holds the cross runtime.

To profile a test's runtime, make the test host fail its probe so buri links a `test-runner` with symbols, then `sample` that child:

```sh
echo "off for profiling" > $BURI_HOME/test-host/<key>/probe
buri test //libs/store/simulator/cluster '--filter=three nodes keep every invariant over 12 seeds' &
/usr/bin/sample <test-runner pid> 30 1 -file out.sample
```

Don't time through `/tmp/buri-heavy.sh`: it caps jobs at 3. Every timed run below started at a 1-minute load under 3 (shown in brackets); load rose to 4–13 during the runs from buri itself.

## Timings (wall seconds)

| Run | Cold 1 | Cold 2 | Cold 3 | Median | Warm |
|---|---:|---:|---:|---:|---:|
| `build //...` | 45.92 [2.33] | 45.70 [2.94] | 45.40 [2.95] | **45.7** | 2.33 [2.96] |
| `build --release //...` | 177.47 [2.96] | 177.06 [2.94] | 177.34 [2.99] | **177.3** | 2.23 [2.92] |
| `test` | 298.87 [2.93] | 298.39 [2.98] | 297.33 [2.98] | **298.4** | 0.27 [2.97] |
| `build //...`, project-cold | 16.59 [2.82] | 16.54 [2.86] | 16.44 [2.96] | **16.5** | |

Release project-cold is 149.5 s, so the cross runtime costs about 28–29 s of both cold builds.

## Where the time goes

Debug build, fully cold (45.7 s):

- Cross runtime (nested `cargo rustc --release` for linux-x86_64 musl, `lto="thin"`, `codegen-units=1`, `paint` feature with taffy, cosmic-text, tiny-skia and font crates): about 95 s child CPU and 29 s wall. It's the critical path.
- buri itself: 12.2 s CPU. Phases: check 25.6 G instr / 2.8 s, emit 28.1 G / 3.4 s, middle 9.6 G / 1.5 s, other 24.7 G / 3.2 s, link 2.3 s child CPU.
- 1996 objects compiled but only 349 distinct unit names. Every program recompiles core_list, core_str, core_result and friends (32 times each), since the codegen key carries a per-package `unit_prefix` and monomorphization makes each program's units distinct (cli/src/build/actions.rs ~1835–1880).
- 32 links, 7281 modules loaded.

Debug build, project-cold (16.5 s), averaging 1.8 cores:

- 0–7 s: 63 bun generator processes in two waves (28, then 35) from `prepare_rules` (cli/src/build/generators.rs). buri sits idle; every target waits on all generators.
- 7–17 s: buri at about one core. Main-thread samples: `prepare_rules` wait 5.3k, `build_target` 5.8k (link wait 1.25k, `linker_for` 632, check ~1.3k), lint (`findings_and_regenerable`) ~1k. Worker threads barely run.
- The serial loop is in cli/src/commands/build.rs ~108–170: `for` over targets and outputs calling `actions::build_target` one at a time. Only emit is parallel, and only inside one program.

Release build, fully cold (177.3 s):

- buri CPU 953 s, 8.87 T instructions, peak RSS 1.1 GB. LLVM emit is 8.80 T / 945 s of that.
- Timeline alternates 1000% bursts (one program's emit) with long single-core stretches (that program's check, monomorphize, middle, link). About 45 s runs at 200% or less, plus 7 s idle on generators.
- Perfect packing across 12 cores would be about 80 s plus the cross runtime overlap.

Test, fully cold (298.4 s):

- Build is done by about 18 s: 132 suites, 2405 objects, 6 links, 13 images, buri CPU 27.6 s.
- Test processes: 165 of them, 6144 tests, 2248 s child CPU (18.3 T instructions), peak RSS 5.2 GB. About 12 run at once from 20 s to 230 s; the last 70 s has fewer than 12.
- Ideal packing of 2248 s on 12 cores is 187 s, against about 280 s actual.

Slowest suites (sum of test durations, 2766 s total; measured at load 6–12, so relative only):

| Suite | Time | Tests |
|---|---:|---:|
| //libs/store/simulator/cluster | 1782 s | 69 |
| //libs/store/server/node | 393 s | 308 |
| //libs/store/simulator/protocol | 120 s | |
| //libs/store/server/registry | 92 s | |
| //libs/store/client/online | 62 s | |
| //libs/store/simulator/raft | 61 s | |
| //libs/authorization | 55 s | |
| //libs/finance (JS) | 34 s | |

The top test takes 135 s (cluster `run.buri`, "billing changes to hundreds of tenants..."). The top 50 tests take 2132 s. Eight applier.buri tests take exactly 30, 90 or 120 s: they sit in actor `sendMessage`'s documented 30 s real-time timeout, about 390 s of a job slot doing nothing.

Runtime profile, cluster "three nodes keep every invariant over 12 seeds" (33 s, ~24.9k samples):

- Test filesystem in cli/runtime/testing.rs: ~4.4k (18%). renameFile 1329, writeFileBytes 836, readFileBytes 607, metadata 523, append 398, readDir 274, removeFile 261. Top of stack: `fs_parents` 1362, `fs_read` 846, memcmp/memmove ~2.5k. `Slot::Files` is a `Vec<(String, Vec<u8>)>`; every read, put and parent check scans all entries, `fs_parents` scans once per path component, and `fs_read` clones the bytes.
- `core_hash$crc32cByte`: ~2.66k (10.7%). hash.buri computes CRC32C a bit at a time, 8 recursive shifts per byte, and its comment calls that a speed decision.
- `core_orderedmap` seek, lookup, insert: ~1.5k (6%).
- Derived `cmp` plus `buri_rt_str_compare`: ~700. Retain/release glue and `Sim_memberOf` closures follow.

Runtime profile, authorization "a sight and a one-shot check agree over more random chained schemas..." (9.2 s, 6.7k samples): retain/release stencil glue ~1.3k (20%), memmove/memset/memcmp ~640, `buri_rt_free` + `buri_rt_alloc_zeroed` ~330, `core_list$contains` 236, then database engine planning (`mostSpecific`, `boundVariables`, `appendUnique`).

## Opportunities, ranked by cold saving

1. **Build programs in parallel in `buri build`.** Saves ~60–75 s on release cold (177 → ~100–115 s), ~5–7 s on debug project-cold. Evidence: the release timeline's single-core stretches and the debug main thread's 5.8k serial `build_target` samples against idle workers. Files: cli/src/commands/build.rs (the loop), cli/src/build/actions.rs (`Session` sharing). Risk: medium; output order of diagnostics and progress must stay deterministic, and memory peaks go up (1.1 GB now). E2e: a project with several binaries builds every output, prints the same diagnostics in the same order as a serial build, and a failing binary still fails the command.
2. **Start the cross runtime build at once and overlap it.** Saves ~15 s on debug cold (it's 29 s of 45.7 s and now starts only at link selection, `select_cross` in cli/src/build/link.rs:847). Evidence: debug cold vs project-cold, 45.7 vs 16.5 s. Files: cli/src/build/runtime_cross.rs, link.rs, commands/build.rs. Risk: low; it's the same build, only earlier. E2e: a cold cross build produces a working linux-x86_64 output, and an error in the cross build still surfaces with its message.
3. **Build only the runtime features a program uses.** Saves part of the remaining cross cost, likely 10–20 s cold when no program paints, since `paint` pulls in taffy, cosmic-text, tiny-skia, skrifa, swash and ttf_parser. Evidence: manifest features at cli/runtime/manifest.toml:376; not yet measured with and without `paint`, so measure first. Files: runtime_cross.rs (cache key and feature list), cli/runtime/manifest.toml. Risk: medium; the key must include the feature set, and a program that paints must still get it. E2e: a cross build of a non-painting program and a painting one both run correctly.
4. **Index the test filesystem.** Saves ~320 s of test CPU, about 25–30 s of cold `buri test` wall. Evidence: 18% of the cluster profile in `fs_parents`, `fs_read` and the memcmp/memmove around them. Swap the `Vec` scans for a `BTreeMap<String, Vec<u8>>` plus a directory set and prefix ranges for readDir, rename and parent checks; hand out shared bytes instead of clones. Files: cli/runtime/testing.rs. Risk: low; same observable behavior, but readDir order must match today's. E2e: tests that write, rename, list, append to and remove files in nested directories see the same results and listing order; plus a guard that counts filesystem operations rather than wall time.
5. **Longest-first test scheduling.** Saves up to ~50 s of the 70 s tail (280 s actual vs 187 s ideal). Evidence: fewer than 12 processes from 230 s to 300 s while the 1782 s cluster suite finishes. Order by the last run's durations kept in `.buri`, falling back to test count. Files: cli/src/commands/test.rs (`plan`, `drive`). Risk: low; output must stay in a stable order regardless of run order. E2e: results print in the same order whatever the schedule; a guard checks that the recorded slowest suite starts first.
6. **Split big suites across processes.** The cluster suite alone is 1782 s of test time over 69 tests, and its 135 s test sets the floor. If one suite runs as one process, sharding its tests over several processes lets the 12 slots pack evenly. Saves a share of #5's tail and stacks with it. Files: cli/src/commands/test.rs. Risk: medium; per-process setup cost and test isolation. E2e: a suite run sharded reports the same passes, failures and order as unsharded.
7. **Hardware CRC32C.** Saves ~190 s of test CPU, ~15 s of cold test wall. Evidence: `crc32cByte` at 10.7% of the cluster profile. Back `crc32c` with a runtime intrinsic using the arm64 `crc32c` and x86 SSE4.2 instructions, falling back to a table. Same results, so no semantics change. Files: crates/stdlib/.../hash.buri, runtime intrinsic glue in cli/runtime. Risk: low with a known-answer check. E2e: crc32c of empty, short, long and unaligned inputs matches published vectors on both targets.
8. **Don't let idle test processes hold a slot.** Up to ~390 slot-seconds, about 30 s of cold test wall. Evidence: eight applier.buri tests at exactly 30, 90 or 120 s, waiting on actor `sendMessage`'s 30 s timeout. The timeout is semantics and stays; the scheduler could start one more process when a running one has used no CPU for a while. Files: cli/src/commands/test.rs. Risk: medium; oversubscription must not slow CPU-bound runs. E2e: a suite that blocks on a 30 s timeout doesn't delay other suites; guard by checking concurrency, not wall time.
9. **Run generators alongside compiling.** Saves ~5–7 s on every cold command. Evidence: buri idle for the first 7 s of project-cold while 63 bun processes run in two waves. Start compiling targets that don't depend on generated sources at once, and run the generator waves wider. Files: cli/src/build/generators.rs, actions.rs. Risk: medium; dependency tracking must be exact. PERFORMANCE.md already measured overlap for tests, so check §6.76–6.87 first. E2e: a target that uses generated code sees the generated output; one that doesn't builds while generators are still running, with the same results.
10. **Compile shared stdlib and library units once per build.** 1996 objects vs 349 distinct names; release LLVM is 945 s CPU. Saving could reach tens of seconds on release cold, but how much of each unit is truly identical across programs isn't measured. Files: cli/src/build/actions.rs (`codegen_key`, unit prefix), the monomorphizer. Risk: high; the prefix exists for symbol-collision reasons documented at actions.rs ~1835. Measure byte-identical units first. E2e: two binaries sharing generic stdlib code both run correctly, built cold and warm.
11. **Cheaper retain/release.** Retain/release glue is ~20% of the authorization profile and shows up in the cluster one too. Find which glue functions are hot (several type-specific ones) and whether the middle passes could drop paired retain/release or inline the glue. Saving unknown, probably 5–10% of test CPU. Files: crates middle passes and stencil glue. Risk: medium-high; a refcount bug is a memory bug, so run `BURI_RT_HEAP_CHECK=1 cargo test -p buri-rt-tests`. E2e: values shared across closures, lists and actors still free exactly once.
12. **Cut serial per-program overhead.** `linker_for` takes 632 samples per build and lint ~1k, both serial on the main thread; check runs once per program (25.6 G instructions in debug). Cache the linker choice per build and lint once per package. Saves ~1–2 s per cold debug build. Files: cli/src/build/link.rs, commands/build.rs. Risk: low. E2e: lint findings print once per package, not once per program, with the same text.
13. **core_orderedmap and string compare hot paths.** 6% plus ~3% of the cluster profile. Check `seek` for avoidable copies and comparisons. Saves ~5 s of cold test wall. Files: crates/stdlib/.../orderedmap.buri. Risk: low. E2e: ordered map insert, lookup and iteration order over random keys.
