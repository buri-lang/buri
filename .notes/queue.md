# Work queue

Ordered by the standing priority: bugs and planned issues, then Valgrind, then coverage, then performance. Lower-tier work waits here, and no agent is started for it while higher-tier work is in flight.

## Running
- **Coverage:** `crates/stencil` coverage.
- **Coverage:** `language_server` coverage phase 2.
- **Cleanup:** restore two `ir.rs` verifier tests and remove `StructuralCmp`.

## Waiting on the user
- Nothing now (Valgrind arm64 and `native::` decisions were made 2026-10-12; see Queued).

## Done
- Coverage phase 1 for stencil and LLVM (merged to main; stencil and llvm commits).
- Same-name top-level declaration is an error (404aa2f6a); an unreachable match arm is always an error (73d492c4c).
- `Style` literals extracted only where a style reaches a UI node (bc652b02b, 94b6805ff).
- In-process loader phases 2-3 and follow-ups (through eb86a91f4).
- Valgrind excluded packages (067a2dc20).
- Coverage of docs (e08bc82d9, 2cce04ce4), stdlib MC/DC (e9273fbba, 9b5cf3b82), `cli/test` (9b5cf3b82), `language_server` (b862ae299) and middle phase 4 (c71cf7fc3).

## Queued (start only when no higher-tier work is waiting)
- **Suite timing (CLAUDE.md 5-minute rule):** idle full-speed run in /tmp/buri-stencil. Queued (agent ended).
- **Coverage CI speed:** `nix develop` costs 1.5–2.4 min per coverage job before cargo starts; add a nix store cache. Two tests take ~4 min each on arm64 under the instrumented debug buri (`repositories::test_suites`, `stencil::a_runaway_recursion_faults_at_the_guard`) and set the critical path; make them faster before adding shards.
- **Coverage (stability, DONE 2026-10-11, CI branch `fuzz-stable`):** the fuzz suite draws mutation seeds from a corpus that includes `cli/tests/reject`, so adding any reject case redraws it and can drop branches that only the old draw reached (the gate failed on this 2026-10-10). Make each existing seed's draw independent of corpus additions, for example by seeding per file from the file's path hash, so new cases only add to what's reached.
- **Coverage/tests:** recreate the cachegrind `run/printer` workload (2000 structs plus derives printed once, in its own repo under cli/benches/instructions/printer/) and a parser-errors workload, then bless only their lines from a CI branch run. The first attempt was lost when its worktree was removed.
- **Coverage:** Guard reduction phases 12–13: minted function index for `Program::funcs` (typed table first, then ir). See `.notes/handoff/guard-reduction.md` Next.
- **Coverage:** next phases per .notes/handoff/compiler-coverage.md (syntax, js, semantics; stencil, `language_server` and middle are done or running). Raise coverage toward 100%, starting with the compiler and type checker (syntax, semantics, middle, backends), then the rest of the CLI (lint, format, LSP and so on), plus stdlib MC/DC, with generated, checked-in end-to-end tests. The invariant helper landed (f136f6195, 2026-10-11 11:10 AM); start these once the bug-tier agents (bounds, language decisions, styles, reset) free slots.
  - Decided 2026-10-11 for the next build-driver phase (build-coverage-4 merged as c0ff3d88c):
    - Remove `RuntimeArchive::Omitted` and the per-link scan. No Buri program reaches it (yagni). Its three in-process native tests go with it, since that behaviour is no longer wanted.
    - Delete `Backend::forks_read_shared_mask` and the `WithRuntime` wrapper use (touches stencil, llvm, js).
    - The held-cache-lock test waits 10 s by design, which adds suite time. Make the wait shorter or event-driven, without timing flakiness.
    - The 27 sides still uncovered are listed in .notes/handoff/build-coverage-4.md.
  - Signature help's module-scope fallback (from 7d712b134) no test reaches; it was rewritten without `let`-`else` to pass the gate. Either reach it with an e2e test or turn it into an `invariant`.
- **Coverage:** language_server coverage phase 4: try the reachable gaps listed in .notes/handoff/coverage-language-server.md: a code action when `buri gen` has nothing to write, more than 8 cached queries, a query on a file no rule claims, and completion after a receiver in an unknown file. It's at 90.09% after a35e245eb.
- **Performance:** split release machine code with use-list order preserved (§6.78; needs a C++ shim).
- **Performance:** the first edit after a cold run in a 10× repo still launches 2 (part of the work_counts agent's queue).
- **Workspace nextest is over 5 minutes:** 301 s of tests plus about 45 s of build (invariant-helper agent, 2026-10-11). Tell the user and suggest compiler or runtime speedups first; this belongs to the standing test-time hunt.
- **Performance: shared compiled stdlib across tests.** Each e2e test is a fresh `buri` process in its own cold `.buri/cache`, so it re-checks the stdlib (§6.28: 3–11% of CPU in recovery, checking and fuzz tests) and re-generates its objects. Estimated over 2 hours (user asked, 2026-10-11): sharing the cache changes the `N cached` counts recorded in golden reports, and an on-disk checked-stdlib snapshot needs serialization. Design first.
- **Semantics test-only code:** move into tests (user decision 2026-10-12, "move into tests if possible"): the two `ws`-is-`None` checks and the language server's `Scope::Files` path. Start after the semantics bugs agent finishes.
- **arm64 Memcheck:** decided 2026-10-12 to leave the arm64 runtime tests out of Memcheck for now. Revisit later.
- **Root library testing label `///testing`:** user said leave it for now.
- **Coverage, weakest areas:** cli/documentation 62.9%, cli/serve+watch 60.7%, cli/run 72.7%, crates/docs 79.9%, cli/lint 83.2%, cli/other 86.1%. Also syntax (25 gaps), stencil phase 7, and language_server phase 3.
- **Coverage:** Stdlib MC/DC phase 17: private intrinsics `charOf(U32): Char` and `fromUtf8Trusted`. See the phase 16 compiler sketch in coverage-stdlib-mcdc.md. They remove 7+1 conditions that are never shown. Starts after main's red coverage fix lands.
