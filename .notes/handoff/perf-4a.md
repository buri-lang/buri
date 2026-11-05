# Perf 4a: where a cold `buri test` of the code repo spends its CPU

This phase only measured and audited. The compiler didn't change. The binary was a release build with `--features backend-llvm` at d2a8f28ea. The code repo was a pristine copy with 1371 `.buri` files. The machine is a 12-core M3 Pro.

Other agents kept the load at 23–54 throughout. Under load, CPU seconds come out about 5% high: phase 2 measured 1759 s of test CPU on a quiet machine, against 1844–1898 s here. Instructions retired don't move with load, so they're listed next to every CPU figure. Track them in follow-up work.

## Method

```sh
rsync -a --delete /tmp/perf4a/src/ /tmp/perf4a/run/ && rm -rf $BURI_HOME $XDG_CACHE_HOME
BURI_PROFILE=1 /usr/bin/time -l buri test            # cold, three runs
buri test //<suite> --force --verbose                # each of the 132 suites alone, cold-ish
buri test //<suite> --filter=<test> --force --jobs=1 # one hot test; `sample` on the runner
/usr/bin/sample <buri pid> 70 1                      # the compiler, first 70 s of a cold run
```

For the test profiles, the host probe was turned off so that runners link with symbols. The compiler profile counts runnable threads too, so its shares are scaled to the 30 s of CPU that the process actually used.

## Headline split

| Cold `buri test` | Run 1 | Run 2 |
|---|---:|---:|
| Wall [load] | 411.6 s [24→35] | 470.5 s [47→23] |
| Total CPU (user + sys) | 1896.8 s | 1889.4 s |
| buri itself | 30.0 s, 176.6 G inst | 28.0 s, 177.4 G inst |
| Test binaries | 1844.1 s, 13,827 G inst | ≈1877 s, 13,830 G inst |
| Generators | 18.4 s | ≈18 s |
| `ld` for runners | 2.2 s, 16.2 G inst | 2.2 s |

Test binaries use **97.4%** of the CPU. The compiler uses 1.6%, and generators use 1.0%.

buri's CPU by phase in run 1:

| Phase | CPU | Instructions | Busy thread-s |
|---|---:|---:|---:|
| lex+parse | 0.21 s | 1.5 G | 0.3 |
| check | 3.03 s | 15.8 G | 9.2 |
| monomorphize | 2.12 s | 10.5 G | 5.4 |
| middle | 4.93 s | 21.1 G | 14.8 |
| emit (stencil codegen) | 8.76 s | 74.9 G | 25.6 |
| link | 0.55 s | 4.0 G | 6.8 |
| run (scheduling) | 0.72 s | 1.5 G | — |
| other | 7.93 s | 43.4 G | — |

- The cross runtime never starts during `buri test`. Native tests don't need it.
- A cold run built 132 suites from 2400 objects, made 13 images and 6 links (1 host and 5 runners), and launched 660 test processes.
- The shared build took 22.5 s of wall.
- The 5 runners are the groups over about 128 MB, which fall back with "a branch inside the image is out of reach".
- The `other` phase is spread out. No single item in it is bigger than 2% of compiler CPU. The parts: style graphs (`semantics::styles`, 2.9%), image linking (`build::image::link`, 2.1%, mostly memmove), `build::programs::settle` (1.6%), action bookkeeping, and sha256 (0.5%).
- The compiler spent 81–140 s **runnable but not running**. Test processes crowd it out, so build wall is long even though build CPU is small.

Each suite alone, sorted by build CPU. Each solo build re-checks the suite's dependencies, so the solo builds sum to 50.8 s against 30 s batched.

| Suite | Build CPU | Instructions |
|---|---:|---:|
| //libs/store/client/online | 3.80 s | 25.3 G |
| //libs/store/simulator/protocol | 3.31 s | 23.2 G |
| //libs/store/server/node | 3.25 s | 21.9 G |
| //apps/sample/served | 1.45 s | 10.5 G |
| //apps/sample/visual | 1.43 s | 11.1 G |
| //libs/store/server/server | 1.35 s | 9.4 G |
| //libs/store/simulator/cluster | 1.34 s | 5.6 G |
| //apps/store_simulator | 1.29 s | 9.5 G |
| //apps/sample/backend/feed | 1.17 s | 8.5 G |
| //libs/store/client/ui | 1.14 s | 8.2 G |
| //apps/store_server | 1.04 s | 7.2 G |
| //libs/finance | 1.00 s | 8.0 G |
| //libs/finance/store/connect | 0.94 s | 7.5 G |
| //apps/sample | 0.91 s | 6.5 G |
| //libs/app_backend/session | 0.88 s | 5.9 G |

Each suite alone, sorted by run CPU. These sum to 1860 s.

| Suite | Run CPU | Instructions | Processes |
|---|---:|---:|---:|
| //libs/store/simulator/cluster | **1329.6 s** | 9790 G | 14 |
| //libs/store/simulator/protocol | 93.6 s | 558 G | 17 |
| //libs/store/server/registry | 74.0 s | 597 G | 15 |
| //libs/store/client/online | 59.0 s | 425 G | 26 |
| //libs/store/simulator/raft | 55.0 s | 439 G | 14 |
| //libs/finance | 49.7 s | 368 G | 3 |
| //libs/authorization | 48.4 s | 400 G | 25 |
| //libs/store/server/server | 29.9 s | 161 G | 29 |
| //libs/store/engine/view | 22.6 s | 155 G | 15 |
| //libs/finance/store | 17.1 s | 161 G | 3 |
| //apps/sample/served | 14.2 s | 119 G | 1 |
| //libs/store/server/subscription | 14.2 s | 119 G | 19 |
| //libs/store/client/ui | 5.8 s | 27 G | 20 |
| //apps/store_simulator | 5.8 s | 42 G | 12 |
| //apps/sample/visual | 5.5 s | 60 G | 1 |

Cluster alone uses **71%** of all run CPU. Its slowest test is `run.buri` "billing changes to hundreds of tenants…", which took 305 s of wall under load.

## Where the test binaries spend it

These are self samples. Four of the profiles come from the suites holding 79% of run CPU.

| Category | billing (cluster) | deleted (cluster) | agreement (authz) | raft | import (online) |
|---|---:|---:|---:|---:|---:|
| Program code | 28.2% | 30.5% | 27.9% | 41.0% | 39.4% |
| Glue release | 17.0% | 16.7% | 19.0% | 17.3% | 5.5% |
| Glue retain | 14.5% | 12.6% | 16.3% | 10.8% | 1.3% |
| Stdlib `core_*` | 11.5% | 13.1% | 9.8% | 11.6% | 2.6% |
| Alloc, free, zeroing | 8.1% | 8.2% | 7.9% | 5.0% | 1.5% |
| memmove | 4.7% | 4.7% | 4.6% | 5.8% | **30.0%** |
| Other glue (elems, eq, hash) | 5.2% | 4.1% | 3.2% | 2.2% | 1.1% |
| Stencil helpers | 3.8% | 3.2% | 4.0% | 4.7% | 2.3% |
| Derived impls | 2.0% | 2.3% | 1.4% | 0.4% | 11.4% |

Test binaries retire about 7.5 G instructions per CPU second, an IPC of about 2.4. That means they're bound by instruction count, not memory. Doing fewer instructions is the lever. Data layout isn't.

## Opportunities, ranked by CPU seconds saved

Savings are for one cold run, out of about 1890 s.

1. **Glue dispatch: a jump table instead of a compare chain. Saves 100–250 s; about 60 lines in `crates/stencil/src/compiler/backend/stencil/glue.rs`.**
   - `walk_through` and `walk_rc` turn each `Site::Tagged` into a linear chain of `eq_u64_fi` stencils. Each arm stores the tag into the frame, reloads it, compares and branches (`str x9,[x0,#0x18]; ldr x8,[x0,#0x18]; cmp x8,#k; b.ne`).
   - The hottest glues in agreement have 11–22 arms and 400–840 instructions. A value in arm k pays about 4k instructions before any work starts.
   - Retain and release glue together take 28–35% of every hot profile except import, which is about 550 s across the run.
   - A jump table indexed by the tag, or keeping the tag in a register for the whole chain, would cut most of the dispatch. Expect 20–45% of glue time.
   - This overlaps Perf 3C, retain/release elision, which is in flight. Elision removes calls, and this makes the calls that remain cheaper. Measure after 3C lands.
2. **`list.contains` allocates, and derived `eq` is slow. Saves about 80 s. Perf 3E is in flight.**
   - In billing, `core_list$contains` takes 4.3% self. Another 2.1% is `buri_rt_alloc`/`alloc_zeroed` called from inside `contains`.
   - In import, `$derive$eq$TripleValue` takes 11.3%.
3. **List pushes that copy the whole list. Saves 60–90 s; the lines depend on the cause.**
   - `append_dest` copies the list when `buri_rt_claim_unique` fails. In the cluster and raft profiles that copy is 4.7–5.8% (`list_push`, `list_take`).
   - In import, 25% of samples are memmove under `append_dest` ← `buri_rt_list_push` ← `core_proto$writeUnsigned`. That means the generated proto writer hands `writeVarint` a buffer that is still shared.
   - A plain loop and a `foldCtx` loop over `proto.writeVarint` both stay linear (80k varints in about 4.5 ms), so the trigger is something in the generated codec.
   - Next step: trace one `writeResultRow` call. Find which retain makes `out` shared, then fix it in `codegen_proto.buri` or in rc elision.
4. **Allocation count. Saves 30–60 s.**
   - Alloc, free and zeroing take 5–8% of test CPU. The allocator itself is cheap; the cost is the number of calls.
   - `_tlv_get_addr` is 0.6–1.2% on its own. `buri_rt_alloc` and `buri_rt_free` look up thread-local state through dyld on every call. Caching the heap pointer, or using initial-exec TLS, saves about 10–20 s.
   - Of the zeroing, 1.8% comes from `buri_rt_alloc_zeroed` called by program code (`orderSteps`, `placeFilters`), and from `append_dest`'s headroom `write_bytes`.
5. **Test processes for small suites. Saves 10–20 s; about 20 lines in `cli/src/commands/test.rs` (`queue_members`).**
   - Each extra process on an image costs about 140 M instructions and 45 ms. The host does `fs::read` on the whole image, copies it into `MAP_JIT` and fixes it up, which means 205 MB of RSS and 12.7k page faults for a 101 MB image.
   - //apps/store_server alone uses 1989 M instructions at `--jobs=12` (13 processes) against 305 M at `--jobs=1`.
   - Cap a suite's processes by its expected work, such as test count or the previous run's time. That avoids several hundred host starts.
   - Mapping the image file instead of reading it saves one copy, about 15 ms per start. macOS can't map a file into `MAP_JIT` directly, so the copy into the JIT region stays.
6. **Compiler: drop the linear search in `bind_of`. Saves about 1.4 s; about 15 lines in `stencil/jit.rs`.**
   - `bind_of` takes 4.7% of compiler self samples. It runs `binds.iter().find(|b| b.slot() == slot)` for every hole of every stencil.
   - Build the binds in hole order once per key.
7. **Compiler: code bytes get copied 4–5 times. Saves about 1.5–2 s.**
   - memmove is 12.6% of compiler CPU. The copies happen in `Region::put` growth (realloc of `bytes`, `relocs`), `assemble_unit`, `object::write` and `image::link`.
   - Only assembly calls `Region::reserve_for` (`stencil/mod.rs:802`). Each `Jit` starts from an empty `Region::new()` (`jit.rs:713`) and grows by realloc. Reserve from an estimate, such as the IR's instruction count, before emitting.
8. **Compiler: five typed-tree walks per function in rc analysis. Saves 1–2 s.**
   - `middle::rc` walks the typed tree five times: `consuming_uses`, `collect_consumings`, `ownership_dependencies`, `effects_once` and `loop_variables_taken`. Together with `BodyIndex::go`, they take 12.9% of compiler CPU.
   - One fused walk would collect all five.

Cold-only costs with nothing to remove:

- Generators: 18.4 s of CPU. Each generator runs once, and its output is cached after that.
- `--force` reruns every generator in every suite, which cost 274 s across the solo runs. Only `--force` pays this.
- Batching already saves 21 s of build CPU against building each suite alone.
- Node's applier tests sit idle on 30 s timeouts. That's 240 s of wall and almost no CPU.

## Structural notes

- Retain and release glue is generated per type and called through a stub of 8 instructions that sets up the frame. The 82 hot glue symbols in agreement form a long tail, and the top one is 8.7% of glue. A table-driven walker (`buri_rt` reads a per-type field map) would replace thousands of glue bodies with one loop. It's simpler than the jump-table fix and probably as fast for small types. Try it after item 1, and compare instruction counts.
- In program code (28–41%), every value lives in the frame and is stored, then reloaded. The glue shows the pattern: `str x9,[x0,#0x18]` followed at once by `ldr x8,[x0,#0x18]`. A peephole that drops a reload of the slot just stored helps both glue and program code, without a register allocator.
- `Bind` in `stencil/jit.rs` holds `Datum::Sym(String)`. Dropping `[Bind; 3]` takes 0.4% of compiler CPU. Interning symbol names into a `u32` removes that work.
- Hash maps take 2.9% of compiler CPU, all FxHash through hashbrown. No map stands out. Specialising one wouldn't pay.

## Should the compiler use arenas?

No. The data:

- Allocation is a small share of compiler CPU:
  - Allocator plus `drop_in_place` is 9.7% of compiler CPU on a cold build, which is about 2.9 s, or 0.15% of total CPU.
  - buri's own allocator (`cli/src/allocator.rs`: `alloc`, `dealloc`, `refill`, `new_page`) is 4.6%.
  - The system allocator (`xzm_*`) and `_tlv_get_addr` together are 1.3%. Nearly all of that is large-block realloc from `Region` growth, which item 7 covers.
  - `drop_in_place` is 2.6%. Of that, `typed::ExprKind` is 1.6% (`semantics/typed.rs`, 21 `Box` fields), stencil `[Bind; N]` is 0.4%, and `lower::Keys` is 0.1%.
- Lifetimes:
  - Each typed tree lives for one batch, and monomorphize clones it (`ExprKind::clone` is 0.6%). A per-batch arena fits it.
  - Per-function stencil data already reuses buffers (`self.bufs`), so an arena gives it nothing.
  - IR (`ir::Block`, `ir::Inst` Vecs) is per program, and its growth is 0.3%.
- An arena saves at most about 1.5 s: the `ExprKind` drops plus part of its allocations.
  - A typed-tree arena means threading a lifetime through semantics, middle and both backends, thousands of lines.
  - §6.52 rejected the same idea after the allocator from §6.54.
  - The one simplification an arena would buy is that drops vanish, and they're already cheap.
- mimalloc: buri already replaces the global allocator with a size-class allocator in the same class as mimalloc (§6.54). Under `DYLD_INSERT_LIBRARIES`, mimalloc would only take over the system calls, which are 1.3%. I didn't measure it, because it can save at most about 0.4 s.

Locality:

- Cache-miss counters aren't available on this machine:
  - Instruments isn't installed (no `xctrace`).
  - `kpc` needs sudo.
  - A Lima `vz` VM (Ubuntu 26.04, kernel 7.0) exposes no PMU, only software, tracepoint, kprobe, uprobe and breakpoint event sources. I deleted the VM and its 3.4 GB image cache.
- That leaves instructions per CPU second as a proxy. The process averaged 3.10 GHz (92.9 G cycles over 30.0 s), which gives these approximate IPCs:

| Phase | IPC |
|---|---:|
| emit | 2.75 |
| link | 2.3 |
| lex+parse | 2.2 |
| other | 1.8 |
| check | 1.7 |
| monomorphize | 1.6 |
| middle | **1.4** |
| test binaries | 2.4 |

- Middle and monomorphize have the lowest IPC, and they're the phases that chase `Box`ed `typed::Expr` nodes. The likely misses are `consuming_uses`, `collect_consumings`, `ownership_dependencies` and `effects_once` walking `ExprKind` children, and `Layouts::compute`.
- A contiguous typed tree, built from indices the way `syntax::parsing::flat` (`BlockId`, `PatId`) already is, could bring them up to check's or emit's IPC. That saves 1–4.5 s at best, under 0.25% of total CPU. It isn't worth the rewrite for speed.
- The test binaries are already at IPC 2.4, so layout isn't what limits them.

## Phase-2 scheduling machinery under the CPU metric

All four pieces only reorder or overlap work. None of them changes the instructions retired.

- **Slowest-suite-first ranking** (`push_later` rank in `crates/diagnostics/src/parallel.rs`). CPU effect 0 s. It shortens the tail when the slowest suite has many tests. Perf 3B is still working on cluster's late start.
- **Idle tests free their slot** (`aside_while_idle`, 34c8fca1a / 8a95833d4). It adds 243 lines plus 175 lines of tests, and its watcher thread polls `proc_pidinfo` every 100 ms per process, under 0.5 s of CPU in total. It's the main reason the phase-2 tail fell from 68 s to 21 s, because node's applier tests sit idle on 30 s timeouts.
- **JavaScript test ordering** (593d7fb41, +145/−7) and **`starts_of`** (c7a237ce3, +291/−61) change the order only. CPU effect 0 s.

If wall stops mattering, removing all four saves about 790 lines at no CPU cost. While wall still counts, keep `aside_while_idle`, which pays for itself in wall. The other three are the first to cut if someone wants simpler code.
