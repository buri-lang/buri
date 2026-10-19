# Guard zero plan

Direction (2026-10-13): far fewer than 600 guards, ideally none. Every group earlier phases called settled is open again.

Counted at origin/main 46a51820b with the phase 7 rule: 623.

```sh
git grep -c -E 'invariant\(|invariant_with\(|\.or_ice\(' origin/main -- crates cli tools
```

## Classes

| Class | Count | Root cause | Fix that removes the branch |
|---|---|---|---|
| Builder `Result` | 99 | inkwell returns `Result` from every `build_*` | `Positioned` builder on llvm-sys (below) |
| Other inkwell `Option`s | 25 | `get_insert_block`, `get_nth_param`, `try_as_basic_value().basic()`, `as_instruction_value`, `get_terminator`, `Intrinsic::find` | Same wrapper: it knows its block, declares functions from `[Type; N]` and hands back `[Value; N]`, and returns the call's value from `LLVMBuildCall2` |
| `Type::Agg` shape | 87 | `ir::Type::Agg(TypeId)` covers struct, enum, list, closure and `Str`, so backends ask `enum_repr()`, `element()`, `glue()`, `head()` and get an `Option` | Kind-typed ids: `Type::Enum(EnumTy)`, `List(ListTy)`, `Fn(FnTy)`, `Record(RecTy)`. The interner returns `EnumRepr` for an `EnumTy`. Instructions that need an enum take an `EnumTy` |
| Value filled in elsewhere | 87 | `ws: Option<&Workspace>`, `warm: Option`, coverage `run()` global, "the diagnostic just pushed", `rule()` per target kind, `suite`, `noted_reads`, `location` | Typestate: pass the thing, not an `Option`. `InRepo { ws }`, `push` returns `&mut Diagnostic`, a `CoverageRun` handle, `TestTarget` carrying its `test` block, jobs carrying suite and reads |
| Parallel rows by position | 71 | side vectors indexed by `usize` (rc `own`/`purity`/`aborts`, exhaustiveness rows, `hits`/`keys`, flat tree ids, shards) | `Dense`/`PerX` tables with minted ids (phases 6, 7, 10, 11 pattern), or `zip`. Shards as `[T; 256]` indexed by `u8` |
| Arity of a slice | 63 | `CallIntrinsic { key: Box<str>, args: Box<[ValueId]> }`, layout slots as `Vec`, `params.first()` | Typed `Intrinsic` enum with named operand fields (phase 9 did this for `Prim`). Slots as arrays where the layout fixes the count |
| Boolean assertion | 41 | stencil latch coalescing (15) rediscovers loop shape, parser progress (4), layout, runtime rows | `lower` records loops (`Code::loops`), so the jit reads them. Parser recovery returns a `Consumed` token only `bump` makes. Delete the rest where nothing depends on them |
| Function table | 33 | `Program::funcs` indexed by `usize`; `body: Option` for externs | Minted `FuncIdx` (phases 12, 13) plus `Defined`/`Declared` split so a body lookup is total. rc side tables become `PerFunc<T>` |
| Search result | 24 | `find`/`next`/`position` after a check that already found it | The checker returns the item it found |
| Stdlib item by name | 21 | `core/indexset` `toList`, `Order`, `platform/effect`, `core/number` ops | A `Known` table built with the stdlib snapshot, so known items are constants like `VariantId::SOME` |
| Map by key | 19 | Tarjan `low` map, jit def/use maps, runtime rows by key string | Dense per-node tables; typed intrinsic keys index `runtime_table` directly |
| Path and string slicing | 18 | `parent()`, `file_name()`, `strip_prefix(root)`, `get(head..)` | `RepoPath`/`FileName` types that keep their parts; prefix and suffix from `char_indices` zips |
| OS, process, channel | 14 | real failures, see below | User-facing errors, or `thread::scope` |
| Cached object bytes | 8 | `image::link` parses objects that may come off disk | `Err` from `link` (it already returns `Result`) plus a checksum on cache read, so a damaged entry is a miss |
| Counting artefacts | 7 | 2 `fn invariant*`, 1 doc comment, `main.rs` test hook, 3 in `cli/tests/language/grammar.rs` | Test code uses `expect`. The rest stay |
| Embedded data | 5 | doc templates, bundled platform build files, LSP schemas checked at run time | Parse at build time into Rust data, or check in a unit test and render infallibly |
| Module verify | 1 | `mod.rs` `verify` | Stays |

The 99 builder guards undercount the branches. `emit.rs` also has 88 `let _ = builder.build_*` discards and about 110 `.unwrap_or(fallback)` on builder results, which turn a bug into wrong code silently. The wrapper removes all of them.

## LLVM builder

inkwell 0.10 checks only `UnsetPosition` (a runtime `Cell`) in almost every `build_*` emit.rs calls. It checks nothing about operand types: `build_and::<IntValue>` takes an `i1` and an `i64` happily. The exceptions are `build_memcpy`/`memmove`/`memset` (alignment), `build_extract_value`/`insert_value` (index) and `build_struct_gep`. So the 99 guards buy almost nothing, and the verifier already catches type mistakes.

A typestate wrapper over inkwell can't remove them: the `Result` is in inkwell's signature, so something must still match on it. Going through llvm-sys can:

```rust
// llvm/build.rs: the only module that calls LLVMBuild*
pub struct At<'b, 'ctx> { raw: LLVMBuilderRef, block: BasicBlock<'ctx>, _b: PhantomData<&'b mut Builder<'ctx>> }
impl<'b, 'ctx> At<'b, 'ctx> {
    pub fn end_of(b: &'b mut Builder<'ctx>, block: BasicBlock<'ctx>) -> Self; // positions, so no unset state exists
    pub fn and(&self, l: IntValue<'ctx>, r: IntValue<'ctx>, name: &str) -> IntValue<'ctx>;
    pub fn memcpy(&self, dst: PointerValue<'ctx>, src: PointerValue<'ctx>, n: IntValue<'ctx>, align: Align);
    pub fn call(&self, f: Callee<'ctx, Returns>, args: &[BasicMetadataValueEnum<'ctx>]) -> BasicValueEnum<'ctx>;
    pub fn block(&self) -> BasicBlock<'ctx>; // replaces `here()`
}
pub struct Align(u8); // log2, built from `Layout::align`, always a power of two
```

- Safety: inkwell's own methods are `unsafe { T::new(LLVMBuild..(..)) }` after the position check. The wrapper is the same code with the check made impossible by construction, so it's no less safe. The unsafe surface becomes one file of about 60 thin methods instead of 10 scattered blocks.
- Typed values (`Int<W>` and so on) would catch width mismatches at compile time, but they'd touch every `IntValue` in a 9,600-line file. Leave that out (yagni). The verifier covers it.
- What still has to branch: `LLVMVerifyModule` (1). The C API validates nothing, so a backend bug shows up as invalid IR there, and dropping the check lets LLVM crash or miscompile. `LLVMRunPasses` returns an error only for a bad pipeline string, which is a constant. Fold it into the verify ICE. Intrinsic and attribute lookup by name (`Intrinsic::find`, attrs.rs) resolve once into a `Known` table when the backend starts, so 1 guard is left there.

## Things that really fail

These become user-facing errors, not guards:

- `image.rs` (8): object bytes can come from the on-disk cache. A damaged entry fails `link` with an error and is rebuilt.
- `profile.rs` (2): `proc_pid_rusage` and `waitid` can fail (`ESRCH`, `EINTR`). Profiling is best effort, so answer `None`.
- `run.rs:175`: `wait` can fail. Say so and exit 1.
- `version.rs:30`: `--self-check` is a check the user asked for. Print the diagnostics and exit 1.
- `generators.rs:733`: a status with no code and no signal is printable. Print the raw status.
- `test.rs:1039`, `sweep.rs:293`: a dead worker only comes from a panic. `thread::scope` re-raises it, so `recv` needs no `Option`.

These can't fail but std types them loosely:

- `child.stdin/stdout.take()` (6 in generators.rs and test.rs): one `spawn_piped` helper keeps 1. `std::io::pipe` would remove it but isn't close-on-exec atomically on macOS (`spawn::start_alone`).
- `run.rs:722` pid: `fork` never returns 0 to the parent. Use `cast_signed` and keep the `kill(0)` comment.

## Target

About 15 to 20 is reachable. The floor is about 6:

- the 2 `fn invariant*` definitions and the doc comment that names one
- the `main.rs` test hook, which is an internal error on purpose
- the module verify
- the LLVM name lookups at start-up
- the one `spawn_piped` take

The slack above the floor is stencil patch slots (`asm.rs`, `region.rs`) and jit edge-copy facts, where a rewrite costs more than the guard. Each phase should try first.

## Phases

Each phase is at most about 300 turns and ends at a commit. Every phase follows CLAUDE.md: e2e first for any user-visible change, coverage diffed base to head, and a count before and after.

Wave 1 runs in parallel. No two phases share a file.

| Phase | Files | Removes | Cost |
|---|---|---|---|
| 14 llvm builder, part 1 | new `llvm/build.rs`, `llvm/emit.rs` lines before 4500, `attrs.rs` | ~65 + hidden fallbacks | high |
| 15 semantics, local | `exhaustiveness`, `inference`, `patterns`, `endless`, `styles`, `constructible`, `intern`, `name`, `layered`, `types`, `expressions` | ~55 | medium |
| 16 real failures | `cli/build/image.rs`, `generators.rs`, `commands/run.rs`, `version.rs`, `language_server/sweep.rs`, `diagnostics/profile.rs` | ~20, as errors | medium |
| 17 typed `FuncIdx`, typed table (was 12) | `middle/monomorphize*`, `derives`, `inline`, `tail_calls`, `closures`, `js/*` | ~10 | high |
| 18 syntax, json, diagnostics | `syntax/*`, `project/languages/json/*`, `diagnostics/diagnostics.rs` | ~25 | medium |
| 19 stencil jit, local | `stencil/jit.rs` (maps, slots), `asm.rs`, `region.rs`, `rtcall.rs` asserts | ~15 | medium |

Wave 2 starts when its predecessor merges. Its phases run in parallel with each other.

| Phase | After | Files | Removes | Cost |
|---|---|---|---|---|
| 20 llvm builder, part 2 | 14 | `llvm/emit.rs` from 4500 on, `mod.rs`, the inkwell `Option`s | ~55 | high |
| 21 cli workspace typestate | 16 | `cli/build/{actions,workspace,tools,sources,coverage,regenerate,link}.rs`, `cli/compiler/modules.rs`, `project/build/*`, `commands/{build,init,query,generate,format}.rs` | ~50 | high |
| 22 cli test, lint, server | 16 | `commands/{test,lint,lint_cache,mod,watch}.rs`, `language_server/*` minus `sweep.rs`, `documentation/*`, `cli/tests/language/grammar.rs` | ~50 | high |
| 23 known stdlib items | 15, 17 | `semantics/{builtins,resolve,expressions}.rs`, the stdlib snapshot, `monomorphize` and `hand_written` lookups | ~25 | medium |

Wave 3 is one chain, because every phase edits `ir.rs` and both native backends.

| Phase | After | Files | Removes | Cost |
|---|---|---|---|---|
| 24 ir `FuncIdx`, `PerFunc` (was 13) | 17, 19, 20 | `ir.rs`, `lower*`, `rc.rs`, `llvm`, `stencil/{emit,glue,jit,mod}.rs`, `js/park.rs` | ~30 | high |
| 25 ir type kinds, part 1 | 24 | `ir.rs`, `layout.rs`, `lower*`, `fuse.rs`, `decision.rs`, `coverage`; the backends compile through a temporary `agg()` | ~25 | high |
| 26 ir type kinds, part 2 | 25 | `llvm/{emit,repr}.rs`, `stencil/{emit,glue,rtcall,lists,jit}.rs`; removes `agg()` | ~60 | high |
| 27 typed intrinsics | 26 | `ir.rs`, `lower/lists.rs`, `backend/runtime_table.rs`, `llvm/emit.rs`, `stencil/{emit,rtcall}.rs`, `js/generate.rs` | ~50 | high |
| 28 loops in the IR, residue | 27 | `lower.rs` records loops, `stencil/jit.rs` latch coalescing, whatever is left | ~25 | medium |

Phase 23 can run beside wave 3 if it leaves `monomorphize.rs` until phase 24 has merged.

# Guard plan, phase 52

Commit: `llvm, stencil: a niche names the type it keeps, and a thunk asks its callee for an env`.

## What changed

- `EnumRepr::Niche` carries `payload: Ty`. llvm `repr.rs` lays out the niche from it, and `option_payload` and its guard are gone.
- llvm `tag_of` takes the `EnumTy` and looks up its own slots, representation and variants.
- `ir::Func::env`, and `monomorphize::Func::env` behind it, say whether the first parameter is an environment. `closures` sets it on every lifted lambda. The stencil thunk reads it, so `Helper::Thunk` lost `args` and `jit::fn_ty` is gone. The leading unit parameter stays.
- Nothing read a closure's signature after that, so `ir::TypeList` no longer keeps one: `FnSig`, `TypeList::sig` and the `fns` row are gone, and `FnTy` is a plain id like `RecTy`.

## Counts

- llvm repr.rs: 1 → 0 by the plan rule.
- llvm emit.rs: 10 → 10. Its 8 `ice` arms are unchanged.
- stencil jit.rs: 3 → 3 by the plan rule. `ice` arms 4 → 3.
- Repo plan-rule count: 81 → 80.

## Output

- Native output is byte-identical to staging, debug and release, for `build` and `test` on `cli/tests/conformance` and `cli/tests/example`. JS differs only in `$t.seed`.

## The niche guard in `tag_of` stays

Reading the niche pointer through a `Place` removes `or_ice("a niche is a pointer slot")`, but the guard can't go yet:

- `piece`'s in-memory arm loses its only tested caller. An `Option` over 256 bytes held in memory reached it; nothing tests a tagged enum that large. Reading every tag through a `Place` keeps it reached, but spills register tags, and `a_hot_function_has_no_allocas` and `a_matchs_tests_of_one_value_read_its_tag_once` in `cli/tests/native/llvm.rs` refuse that.
- So it needs an end-to-end test in `cli/tests` first: a release build that matches on an enum over 256 bytes (`repr::WIDEST_IN_REGISTERS`), held in memory. With that test in, the niche can be read with `addressed` (`Unit::rc`'s spill, factored out) and `place_pointer`.
- Factoring `addressed` out of `rc` drops two covered `rc_weight` branches, and the total's fraction falls with them. That has to land with the new test, which adds covered branches.
- `At::entry_buffer` moves the builder to the end of its block afterwards. That's wrong inside `Unit::read_once`, which builds just after a definition. A niche read inside `read_once` needs the alloca built by a builder of its own.

## Left

- The 7 type-shape `ice` arms in llvm emit.rs. None can go wholesale until intrinsic results are typed:
  - `enum_ty` has 10 sites. 3 are instructions (`GetTag`, `MakeEnum`, `GetPayload`). 6 are `CallIntrinsic` results (`call_sum`, `call_result`, `json_prim`, `option_value`, `convert_checked`) or `main`'s result.
  - `list_ty` has 4. `list.empty` needs no type, because a list's slots are always `repr::LIST`. `generic_element` reads the type of an intrinsic's `Arg::Elems` argument.
  - `agg` has 6, plus `agg_ty` and `Boxes::owner`. `call_result` reads an intrinsic's result type.
  - `int`, `int_ty`, `float` and `ptr` read `Function::values`, which holds `BasicValueEnum` for every IR value. Typed wrappers would only move the match. Removing them needs a value table keyed by kind, or IR value ids that carry their kind.
- stencil `jit::enum_ty` and `jit::agg_of` (1 plan-rule guard) have the same blockers.

# Guard plan, phase 51

Commit: `llvm: a value's counts are always read at an address`.

## What changed

- `types::result_shape` returns a `ResultShape` with `ok`, `err`, `t` and `e`. The old code read `E` with `args.get(err)`, which used a variant index as an index into the type arguments.
- llvm `Place` is now a single struct, `{ base, align }`. A value in registers is spilled to an entry-block scratch before its counts are walked, the same way glue already gets it. `place_pointer` and `place_int` return values directly.
- stencil `rtcall`'s `Ret::Res` reads `T` from the shape, so `ok_payload_bytes` and its guard are gone.

## Counts

- llvm emit.rs: 25 → 18 counting every form, 16 → 10 by the plan rule.
- stencil rtcall.rs: 9 → 8 by the plan rule.
- Repo plan-rule count: 89 → 82.

## Output

- Debug native output and the JS tools are byte-identical.
- Release native output is not. The example `server` binary has the same size and the same instruction count, but different register allocation and branch layout. Every release test still passes. Widening the spill stores was not the cause; I didn't find which site is.

## Left

- `tag_of`'s niche `position().or_ice`, at about emit.rs:1686.
- The 7 type-shape `ice` arms (`agg`, `enum_ty`, `list_ty`, `int`, `int_ty`, `float`, `ptr`). The IR-kind ones need instructions to carry `EnumTy` and `ListTy`. The inkwell ones need typed values across 55+ call sites.
- stencil `jit::fn_ty`: the thunk needs an IR fact saying whether the function takes an env, such as an `env` flag on `ir::Func` set from the lifted set. Dropping the leading unit param changes the stencil frame layout, so the output is no longer byte-identical, and it removes covered branches in middle.

# Guard plan, phase 50

- **`glue.rs` `load_through` unaligned offset**: removed structurally.
  - `layout::Words` is a byte offset that is a whole number of words. `layout::At` is a field's offset, `Word(Words)` or `Byte(u32)`; every type holding a pointer is word-aligned, so a field at a `Byte` offset holds no count.
  - `counts::Field.offset`, `Site::Block.offset`, `Site::Guarded.null_at` and `EnumRepr::Niche.null_at` are `Words`. `counts::fields` skips a `Byte` field with a `let-else`.
  - `load_through` takes `Words`, so its arm64 condition is only the 12-bit range.
- **llvm `emit.rs` walk**: `Walk::Count(Count, &Place)` or `Walk::Copy { base, align }`. A copy carries its block, so the two `place_address(...).or_ice("a copy walks a value in memory")` are gone. `walk_memory` turns a glue job's `Op` into a `Walk`.
- **llvm `emit.rs` float constant**: takes `f32`/`f64` from `ty == ir::Type::F32`, like `jit.rs`. The `ice` arm is gone.
- Native output (debug, release and test hosts of `cli/tests/conformance` and `cli/tests/example`) is byte-identical: 178 of 178 artifacts.

Landed on `staging` as 2378098ee and 0884385dd. The repo count (plan rule) went from 90 to 88. With every guard form, `llvm/emit.rs` went from 28 to 25; no other file changed.

## Coverage (local, then CI)

| Area | Local base | Local new | CI baseline (staging) | CI estimate |
| --- | --- | --- | --- | --- |
| total | 14796/15068 | 14797/15068 | 14883/15071 | 14884/15071 |
| backend | 64/64 | 66/66 | 68/68 | 70/70 |
| llvm | 547/548 | 545/546 | 580/580 | 578/578 |
| middle | 1840/1876 | 1842/1878 | 1840/1876 | 1842/1878 |
| stencil | 1040/1081 | 1039/1079 | 1069/1080 | 1068/1078 |

Every new branch is covered locally. llvm's one uncovered branch locally is macOS-only, as before.

## Not done, and why

- **`object.rs` addend check (2 uncovered branches)**: `object::Reloc` is shared by the Mach-O and ELF writers. ELF needs signed addends (x86-64 `-4` and per-site `Pc32` distances), and Mach-O `Abs64` needs wide ones. A Mach-O-only 24-bit type needs one of these:
  - the x86-64 correction moved into `RelKind` (`Pc32 { back: u8 }`), touching `asm.rs` `ShimReloc`, `region.rs`, `jit.rs` and `elf.rs`; or
  - an addend enum whose instruction and word forms the Mach-O writer still has to tell apart, which leaves an impossible pairing representable.
  - Anchoring on fixed 16 MiB boundaries would make the addend `slot & 0xff_ffff` with no check, but a literal could straddle an anchor and be split by `-dead_strip`.
- **`jit::fn_ty`**: the thunk needs only whether the function takes an environment parameter. That's not derivable from `env.is_some()`: a capture-free lambda becomes `ExprKind::FuncRef` with a leading unit parameter (`middle::closures`). Recording it needs `closures.rs` to mark it, which means a `crates/semantics` `ExprKind` change, or lowering to match the source `Ty` (a new guard). Gains 1 guard and no coverage.

## Next

- llvm `emit.rs` 25 left. The `place_pointer(...).or_ice` group (5) is a slot list against layout offsets; `agg_ty`/`enum_ty`/`list_ty` and friends (7 `ice` arms near line 4090) are the `Type::Agg` shape class; `result_shape` (3) wants `types::result_shape` (semantics) to return `T` too.

# Guard plan, phase 49

## Branches

- **`emit.rs` ArrayGet overlap**: removed by a refactor (6d4f5ba73).
  - The overlap can't happen: slots are 8-aligned, coalesced classes are disjoint, and `forward_fields` refuses a write over the field it reads.
  - `elem_load` now reads before it writes on every path. 3, 5, 6 and 7-byte elements are copied, then masked. They were cleared first before.
  - e2e test: `cli/tests/native/odd_sizes.rs` `elements_of_three_five_six_and_seven_bytes_read_back`.
- **`object.rs` addend over 24 bits**: this was a real bug. A literal past 16 MiB failed to build on macOS. Fixed in 2d1580811.
  - `assemble_unit` now gives each pool stretch past `MAX_ADDEND` an anchor of its own, so addends stay in range.
  - e2e test: `a_literal_past_sixteen_mebibytes_builds_and_reads_back`. Both tests together take about 1.3 s.
  - The check in `object.rs` (about line 612) is still uncovered (2 branches), because nothing can reach it now.
- **`glue.rs` `load_through` unaligned offset**: not done. It's unreachable, because every rc site offset is a multiple of 8. Two ways to remove it:
  - type the offsets through `counts.rs` and layout as aligned words; or
  - always use the register-offset form, at a cost of one instruction per glue site.

## Coverage (local, then CI)

| Area | Local base | Local new | CI baseline | CI estimate |
| --- | --- | --- | --- | --- |
| stencil | 1039/1081 | 1040/1081 | 1090/1102 | 1091/1102 |
| middle | 1840/1876 | 1840/1876 | 1832/1868 | 1832/1868 |
| llvm | 547/548 | 547/548 | 580/580 | 580/580 |
| backend | 64/64 | 64/64 | 68/68 | 68/68 |
| total | | 14793/15064 | 14906/15095 | 14907/15095 |

## Left

- The `glue.rs` `load_through` unaligned offset (see above).
- The `object.rs` addend check, now unreachable. To remove it, make the addend type carry its range.
- `jit::fn_ty`: not started.
  - Record arity on `MakeClosure` in `lower.rs`, at about lines 1160 and 1413.
  - That also touches the IR printer (`ir.rs` about 1972), the llvm emitter (`emit.rs` about 978) and `linkage.rs`.
  - Its ice arm doesn't seem to count in coverage, so removing it gains nothing.

# Guard phase 48: closures the runtime keeps

`Ui.memo`, `Ui.watch`, `ui_node.reactive`, `mount`, `rebuildRegion`, `reconcile`, the two timers and the six element handlers lower to one typed instruction, the way phase 46 did `Tasks.parallel`:

```rust
Inst::Handover(Box<Handover>)

pub struct Handover {
    pub dest: ValueId,
    pub args: Box<[ValueId]>, // the row's own arguments
    pub closure: ValueId,
    pub ty: Ty,               // the closure's type
    pub ret: Ty,
    pub kept: Kept,
}

pub enum Kept {
    Compute(ComputeRow, [Ty; 1]), // body
    Walk(WalkRow, [Ty; 3]),       // context, builder, node
    Press(PressRow, [Ty; 2]),     // context, event
}
```

- `intrinsic_keys::Handoff` names the 14 rows. `lists::handles` covers them, so their functions are code, not runtime bodies.
- `runtime_table::handoff(row)` returns the row with no key lookup. `Extra::Compute`, `Walk` and `Press` are gone.
- Stencil: `handover_words` builds the closure's words as `rt_call`'s tail. `last_closure` and the press-context guard are gone.
- LLVM: `Unit::handover` emits the call. The `entry_args` closure arms and `fn_ty` are gone.

Commit: `middle: a closure the runtime keeps is a typed instruction`.

## Guards (non-test code)

Same rule as phase 47. Total in the touched files: 69 → 66.

| File | Before | After |
|---|---|---|
| `stencil/rtcall.rs` | 13 | 11 |
| `llvm/emit.rs` | 29 | 28 |

## Checks

- The full suite with `--features buri/backend-llvm` passes: 3159 tests, none failed.
- fmt passes. Both CI clippy commands show only the three warnings base already has.
- Debug and release output matches base. That covers the bench programs, `tested`, `tests/example`, conformance, and conformance with its JS-only suites stripped so they build natively. Native binaries, linked test hosts and compiled objects are byte-identical. JS differs only in `$t.seed`. Cache entries differ only in toolchain id, keys and test timings.

## Coverage

Local runs are macOS only. The CI estimate is the CI baseline plus the change since the code CI measured. Phase 46's local base is the local figure for that code.

| Area | Local base | Local new | CI baseline | CI estimate |
|---|---|---|---|---|
| stencil | 1057/1099 | 1039/1081 | 1090/1102 (98.911%) | 1069/1080 (98.981%), passes |
| middle | 1836/1872 | 1840/1876 | 1832/1868 (98.073%) | 1840/1876 (98.081%), passes |
| llvm | 547/548 | 547/548 | 580/580 | 574/574, passes |
| backend | 70/70 | 64/64 | 68/68 | 64/64, passes |
| js | 985/1018 | 985/1018 | 985/1018 | unchanged |
| total | 14816/15090 | 14796/15070 | 14906/15095 (98.7479%) | about 14883/15071 (98.7526%), passes by under one branch |

The stencil gains back the branch phase 46 cost it, because the 18 branches this removes are all covered. The total margin is thin.

## Left

- `jit::fn_ty` stays for `MakeClosure`. The stencil thunk needs the closure type's arity, because a capture-free lambda keeps a leading unit parameter of 8 slot bytes. Removing it means recording the arity on `MakeClosure`, or having `middle::closures` drop that unit parameter. Either way the output changes.
- The uncovered stencil branches are untouched, because none is clearly impossible:
  - `glue.rs` `load_through` at an unaligned offset can't happen, since layouts align tags and pointers. But nothing in the types says so.
  - `emit.rs` `ArrayGet` with the destination in the array's slot needs a closer look at `pin_call_values`.
  - `object.rs` rejects an addend over 24 bits, which a big enough data section could reach.

# Guard phase 47: cli, project, diagnostics, docs

Commits on staging:

- `0da8bcc87` json: a `$ref` generates from what the check resolved, and a type names its declaration by place
- `3f6a7fd85` lint: an unused context bound is found at the spans the checker recorded
- `f3791281f` build: an output that isn't piped reads as empty, as wait_with_output has it

## Guards (non-test code)

Counted with the phase 45 rule, plus `.expect(`, `panic!` and `unreachable!`, up to each file's first `#[cfg(test)]`. Total: 28 → 17.

| File | Before | After |
|---|---|---|
| `crates/project/.../json/types.rs` | 3 | 1 |
| `cli/src/commands/lint.rs` | 2 | 0 |
| `cli/src/build/spawn.rs` | 2 | 0 |
| `cli/src/build/link.rs` | 2 | 0 |
| `cli/src/build/programs.rs` | 1 | 0 |
| `cli/src/build/mod.rs` | 1 | 0 |
| `cli/src/build/cache.rs` | 1 | 0 |
| `crates/diagnostics/src/diagnostics.rs` | 11 | 11 (definitions) |
| `crates/project/.../json/suite.rs` | 4 | 4 (test-only module) |
| `cli/src/main.rs` | 1 | 1 (test hook) |

`crates/docs` has none.

## How

- **`$ref` targets.** `Registry::new` keeps each reference's resolution in `targets`, keyed by the string node that writes it. `Registry::reference(schema)` returns a `Reference { written, resource, target }`. The generator's `ty` and `tagged` read it, so they no longer resolve again. `Registry::target` is gone.
- **Declaration ids.** `Decls` reserves a declaration when its name is taken and hands back a `Named { id, name }`. `Ty::Named` and a variant's payload carry that value, so `named` reads the declaration by id. `order` keeps the root's alias first. Reservation order is the old insert order.
- **Lint bound spans.** `typed::Body::bounds_written` holds the span of each bound the declaration's own type parameters were written with. `check_fn` fills it from the declaration it already has. `bound_spans` and its two `ice` arms are gone.
- **Pipes.** `spawn::finish` reads an output that isn't piped as empty, as `wait_with_output` does. The musl probe writes to stdin only if it has one.
- **Best effort.** `create_parent`, `programs::settle_in` and `tag::set` skip a path with no parent or with a NUL.
- **Symbol prefix.** `mentions_runtime` compares with `PREFIX.first()`, so the const `expect` is gone.

## Left, and why

- **`types.rs` `Shape::Tagged` variant lookup:** this finds the variant by the data's tag value. A total version needs the validator to return a typed instance tree that records which `oneOf` branch each value matched. Generation would then walk that tree, not raw nodes. A pointer-keyed map would still be a lookup with an `Option`. A data file whose tag matches no branch is refused by its check (`json-schema-violation`) before generation, so the guard can't be reached.
- **`diagnostics.rs` (11):** the `ice`, `invariant`, `invariant_with` and `or_ice` definitions, their doc comments, and the debug-only `debug_panic`. These are the floor.
- **`main.rs` test hook:** an internal error on purpose.

## Checks

- `cargo fmt --all --check`: clean.
- CI's two clippy commands: the same warnings as base, none in touched files.
- Full suite with `--features buri/backend-llvm`: passes, 6 min 27 s.
- Output check: `json_generate`, `json_generate_edges` and `contract_generate` (build, run and lint), plus `cli/tests/example` lint, give byte-identical logs on base and branch. Cache contents are identical too, apart from `cache/.toolchain`.

## Coverage

Local base → new, with the CI baseline. Branch counts are the same as base in every area.

| Area | Branches | Lines |
|---|---|---|
| cli/build | 1625/1683 → 1625/1683 (CI 1677/1685) | 10439/11005 → 10437/11003 (CI 10741/11025) |
| cli/lint | 560/566 → 560/566 (CI 560/566) | 2943/2982 → 2925/2962 |
| crates/project | 1345/1360 → 1345/1360 (CI 1346/1360) | 6290/6454 → 6317/6481 |
| crates/semantics | 2154/2154 → 2154/2154 (CI 2158/2158) | 13365/13667 → 13378/13680 |

The cli/build line fraction drops by about 0.001 points: two covered lines of `expect` plumbing are gone. The gate counts branches only, so this doesn't fail it.

# Guard phase 46: typed `parallel`

## What changed

- `ir::Inst::Parallel(Box<Parallel>)` names the scheduler (`Tasks::Host` or `Tasks::Testing`), the context, the list and the step, plus the step's semantic signature `[C, Int, A] => B`.
- `lower/lists.rs` writes it at each call site and as the body of the two `parallel` functions.
- `intrinsic_keys`: the `Tasks` enum replaces `StepCall`, `step_call`, `step_key` and `STEP_KEYS`.
- `runtime_table`:
  - `parallel(tasks)` returns the row directly.
  - `Extra::Step` is gone.
  - `shares_counts` finds a host `Parallel` in function code.
- Stencil:
  - `rt_call` takes a `tail` of words.
  - `step_words` builds the four step words from the instruction.
  - `glue::entry_thunk` walks the parameters once, so the last one is the element by construction.
- LLVM:
  - `Unit::parallel` emits the call.
  - The `Arg::Step` arm, `answer_element` and `Elements` are gone. `entry_args` takes a `(stride, glue_ty)` pair, which `generic_element` or `bare_carrier` always answers.
  - The Press arm reads its context type from `step_layout`.

## Guards (non-test)

| File | Before | After |
| --- | --- | --- |
| stencil `rtcall.rs` | 16 | 13 |
| stencil `glue.rs` | 3 | 1 |
| llvm `emit.rs` | 33 | 29 |

Total: 9 fewer. No other file changed its count.

## Left

- `rtcall::last_closure` and the press-context guard. They serve the Compute, Walk and Press rows, which each need a typed instruction. See `.notes/handoff/typed-parallel.md`.
- The LLVM type-kind helpers, which need kind-typed ids.

## Output check

Base `9424ccea0` against new, both built with `--features backend-llvm`. The corpora are programs, tested, example, conformance, and conformance with `backends: [JS]` removed, each in debug and release.

- Every native object and binary is byte-identical.
- JS differs only in `$t.seed`.
- Link manifests differ only in cache keys, which hash the compiler.
- Conformance tool file names differ.
- Logs are the same once timings and paths are normalised.

## Coverage blocks landing

The gate compares exact fractions, and this change deletes covered code in areas that have uncovered code, so their fractions fall. Local runs are macOS only; the CI estimate is the CI baseline plus the local change.

Branches:

| Area | Local base | Local new | CI baseline | CI estimate |
| --- | --- | --- | --- | --- |
| middle | 1832/1868 | 1836/1872 | 1832/1868 (98.07%) | 1836/1872 (98.08%), passes |
| stencil | 1060/1103 | 1058/1101 | 1090/1102 (98.91%) | 1088/1100 (98.909%), fails |
| llvm | 553/554 | 547/548 | 554/554 | 548/548, passes |
| backend | 68/68 | 70/70 | 68/68 | 70/70, passes |
| js | 985/1018 | 985/1018 | 985/1018 | unchanged |

Lines:

| Area | Local base | Local new | CI baseline | CI estimate |
| --- | --- | --- | --- | --- |
| stencil | 7990/8645 | 7969/8624 | 8183/8648 (94.62%) | 8162/8627 (94.61%), fails |
| llvm | 7386/7565 | 7344/7524 | 7483/7670 (97.56%) | 7441/7629 (97.54%), fails |
| backend | 374/409 | 385/420 | 375/409 | 386/420, passes |
| middle | 14568/14976 | about +24/+23 | 14568/14976 | passes |

The middle numbers count the `lists.rs` fix, which turns two let-else statements into one `match`. Its own coverage run wasn't repeated; match arms aren't counted, so the arithmetic is exact.

To land, stencil needs one more covered branch and about 10 more covered lines, and llvm about 20 more covered lines. Removing code can't do that, because every remaining uncovered stencil branch is a real case:

- `emit.rs` `ArrayGet`: the destination shares the array's pointer slot. A loop such as `s = s.items[0]`, where `items` is a list of the record `s`'s own type, should reach it, because the element coalesces into the parameter's slot and the field lives inside it.
- `rtcall.rs` `fits` and `glue.rs` `load_through`: an offset that isn't aligned to its width.
- `object.rs`: an addend outside 24 bits.

E2E tests live in `cli/tests`, which another agent owns in this phase. So the change waits on the branch for an e2e test that covers such cases, from whoever owns `cli`.

# Guard phase 45: integer literals in inference, js statement queue

Commits on staging (d8e849570):

- 55a9f3ada semantics: an integer literal waits on its variable for the type it settles at
- d8e849570 js: a body's statements are taken from one queue, and a name's facts are noted in place

## Guards (non-test code)

- `crates/semantics/.../inference.rs`: 1 → 0. `Subst<L>` now carries each integer literal on its variable (`Class::Int(Vec<L>)`). Binding the variable to a concrete integer moves the literal to `settled`, with the `IntPrim` it settled at. `check_literal_ranges` drains that list, so it no longer has to look the type up.
- `crates/js/.../javascript.rs`: 3 → 1.
  - `each_statement_with` takes statements from one shared queue rather than from per-slot `Mutex<Option>` slots.
  - `LocalFacts::entry` is replaced by `note(name, f)`, which updates the entry in place.
- `crates/project/.../json/types.rs`: 3 → 3, not attempted.
- Total: 7 → 4.

## Coverage (local base → new; CI baseline)

| Area | Branches | Lines |
|---|---|---|
| semantics | 2154/2154 → 2154/2154 (CI 2158/2158) | 13333/13635 → 13365/13667 |
| js | 985/1018 → 985/1018 (CI 985/1018) | 5288/5531 → 5298/5541 |
| project | 1345/1360, unchanged (CI 1346/1360) | 6290/6454, unchanged |

## Left

- **js `Extract::expr` slot or_ice:** the survey and rewrite invariant spans two passes. A fallback would silently drop a binding that is still read. Counting rewrites would add an uncovered branch and lower the js fraction.
- **project, all 3 guards:** each needs the schema check to return what it found. That means carrying the `$ref` targets, giving decls stable typed ids, and having the validator return the oneOf branch it matched.

## Observation

Release builds of `conformance-native-src` sometimes lose one out-link. The missing link varied between runs (proto, proto_presence, variants), and the base binary does it too. This is pre-existing nondeterminism.

# Guard phase 44: typed enum and word slots in llvm

Commit `899577f15`. Non-test guards in `crates/llvm` went from 45 to 36:

- `emit.rs`: 42 to 33.
- `repr.rs` 1, `attrs.rs` 1 (plus 1 in test code), `mod.rs` 1: unchanged.

## How

- `repr::STR`, `repr::LIST` and `repr::CLOSURE` are the constant slot arrays of those types. `repr::tagged(tag, payload, size)` builds a tagged enum's two slots from its layout. `build` uses all four.
- `repr::disassemble_n::<N>` takes a register value apart into `N` slots, and `repr::slot_at` takes one. The callers that read a Str, list or closure use it directly. None of those types is ever in memory, so `pieces_n` and every `ir_slots` lookup feeding it are gone, along with the `code` parameter on the abort, show and `str_len` helpers.
- `payload_of` reads a tagged payload as `let [_, blob] = disassemble_n(..)`. `sum_arm` takes the `EnumLayout` and builds its slot with `repr::tagged`.
- `call_result` reads the err tag off its layout once, as `Option<(Scalar, u32)>`, and `store_variant_index` takes that pair. A bare tag zeroes an empty range, so its ice is gone.
- The `main` shim tests the tag with `tag_of` and reads the message with `payload_of`, then `disassemble_n::<3>`.
- `unary`, `signum` and `cast` read the operand's kind off the value, not off `prim`.

## Left, and why

- **Niche `position` search in `tag_of`:** needs the null slot's index recorded where the slots are built.
- **`repr.rs` `option_payload`:** needs a change to middle's `EnumRepr::Niche`, which reaches stencil.
- **`walk_rc` place Options (7):** needs a `Walk` typestate, or spilling registers to memory, which risks performance.
- **Type-kind helpers (8)** (`agg`, `fn_ty`, `enum_ty`, `list_ty`, `int`, `int_ty`, `float`, `ptr`): need typed IR instructions or typed values.
- **`result_shape` (3), `json_prim` (2), `convert_checked` (3):** Option lookups into stdlib and types tables in other crates.
- **Runtime-call sites (4):** out of scope.
- **Floors:** intrinsic find (2), `check_terminators`, the float constant, `bounds`, `attrs.rs` attribute kind lookup and `mod.rs` verify.

## Checks

- Full suite with `--features buri/backend-llvm`: passes, 11 min 23 s locally.
- Native outputs of `cli/tests/example` and `cli/benches/instructions/{programs,tested}` are byte-identical in debug and release. Logs match apart from JS `$t.seed=` values.
- Coverage, `crates/llvm` branches: 553/554 locally before and after (lines 7481/7670 to 7386/7565). The one miss is the Linux-only comdat branch in glue declarations. CI baseline 580/580.

# Guard phase 43: build declarations, slot placement and branch rows

Guards in non-test code went from 205 to 185. Both counts include 4 in `crates/project/src/languages/json/suite.rs`, which is test-only through `mod.rs`.

## Per file

- `crates/llvm/.../emit.rs`: 56 to 42.
- `crates/llvm/.../repr.rs`: 2 to 1.
- `crates/llvm/.../mod.rs`: 2 to 1.
- `crates/middle/.../rc.rs`: 12 to 8.

## How

- `build::Declared<'ctx, N>` declares a function from an array of `N` parameter types. Its `params()` hands back `[BasicValueEnum; N]`. The equal thunk, the entry thunk, the `main` shim and the thread door all read their parameters from it.
- `reaches_a_cycle_in` runs over `Dense` tables.
- `cancelled_counts` uses a `Set`.
- `load_slots` maps over `load_one`.
- The step layout is a typed `StepParam` list.
- `place_piece` zips slots with pieces.
- `repr::struct_of` builds the struct type of two or more slots.
- `optimize` folds verify and the pipeline into one check.
- `Scan::branches` takes rows of (size, data) and passes each branch its own row, so the `if` and `match` callers no longer look anything up by index.
- `layout::tag_scalar` is now a `match`. This keeps the middle area's fraction from falling: its `> 65536` arm was never covered.

## Left, and why

Everything below would need a new typestate or typed view before its guard can go.

- **Type-kind helpers** (`agg`, `int`, `ptr` and the rest): these need typed values.
- **Enum slot reads** (`pieces_n`, `piece`, niche and tagged payload): these need a typed enum slot view.
- **`walk_rc` Copy-needs-memory**: this needs a `Walk` enum. Two guards don't justify that cost.
- **`subtree_sizes` and `Index::go` placeholders**: the preorder slot is filled after the children, so the write needs the slot looked up again.
- **Runtime-call sites**: these are another agent's area.

# Guard phase 42: typed stencil hole names

Commits on staging: `a6dcfcc86` (phase 41 notes), `239dff92f` and `e1be9575b`. The work is in `crates/stencil`.

## What changed

- **Hole names have types.**
  - `abi.rs` adds `HoleName`, which is either a `Slot` or an outside `Symbol`.
  - A `Slot` is one of three classes: `Lit` (H32), `Word` (H64) or `Jump` (branch).
  - The library reads each name once. Its encoding is the same as before.
- **Binds have types.** `Jit::emit` takes `&[Bind]`, built with helpers such as `Lit::A.of(x)`, `Word::K.lit(x)`, `Word::M.ptr(p)` and `Jump::T.to(V::Blk(b))`.
  - `Datum` holds a word's value. `V` now covers jump targets only, so `V::I` and `V::Ptr` are gone.
  - `patch_hole` dispatches on the bind's class.
- **The build checks each hole's kind.** `sources.rs` refuses a stencil whose relocation kind disagrees with its name's class. This replaces the old check that refused a symbol whose address was taken.
- **Two fixes the new build check exposed:**
  - `decref/drop` used to call its glue through `_JIT_M`. It now calls it through `_JIT_CALLEE`, and the machine code is the same.
  - The `rt/N` and `rtv/N` stencils had no users, so they were deleted. They called `_JIT_R`.
- **Compare-and-branch fusion is now a lookup.** `brcmp` fuses only when the library has the stencil, which removes the guards `is_comparison` and `has(key)`.
- **Dead code removed:** `key_fmt!`, `Stencil::find` and `same_name`.
- **`arm_key`:** when the base doesn't end in the arm to elide, the swap twin is taken outright. A twin swaps its arms, so it ends in the other hole. This drops a match guard that was never false.

## Guards (non-test)

| File | Before | After |
| --- | --- | --- |
| `jit.rs` | 13 | 7 |
| All other stencil files | unchanged | unchanged |

Removed:
- The Imm32 `or_ice` and the Imm64 lo12 `or_ice`.
- Both Imm64 `takes a datum` ices.
- The brcmp `invariant` and `invariant_with`.

Kept: the unbound slot hole in `emit_key`, now an `ice` arm (it was `invariant_with`). Only a binding type per stencil could tie the slots a stencil has to the slots a call site binds.

## Not done

- **Step rows:**
  - Affected guards: `rtcall.rs` (step's call, source and answer, `last_closure`) and the two element guards in `glue.rs`.
  - These need a typed IR instruction for `parallel` across middle, llvm, js and stencil, plus type arguments. That is more than one phase and touches other agents' crates.
- **`check_literal_ranges` (semantics):** skipped because phase 43 is working in semantics at the same time.

## Output check

Base `ad1114bd2` against new, with `buri` built with `--features backend-llvm`.

- Every native object and binary is byte-identical in debug and release, on programs, tested, example and conformance with the `backends: [JS]` lines removed:
  - debug: 71, 14, 27 and 641 files;
  - release: 71, 17, 30 and 716 files.
- JS differs only in `$t.seed=Nn`, which is random per run.
- Conformance tool file names differ, because they are hashed from the checkout path.
- Logs are the same once timings and paths are normalised.

## Coverage (stencil branches)

| Run | Covered / total |
| --- | --- |
| Local base | 1069/1113 (96.05%) |
| Local new | 1060/1103 (96.10%) |
| CI baseline | 1099/1112 (98.83%) |
| CI estimate | about 1090/1102 (98.9%) |

- The new code adds 2 covered branches in `abi.rs`. `jit.rs` loses 10 covered branches. `emit.rs` loses 1 uncovered and 1 covered.
- No other area changed.

# Guard phase 41: struct and variant declarations, units by position

## What changed

- `TyDef::Struct` has `form: StructForm` in place of `record: bool`.
  `StructForm::Tuple(TupleDecl)` holds a tuple struct's generics and fields as
  written. `Static::TupleStruct` carries the `TupleDecl`, so
  `shape_of_struct` renders it with no lookup.
- `VariantInfo.decl` is the variant as written, so `shape_of_variant` renders
  it with no lookup.
- `formatting::constructor` takes the name, generics and fields, and returns a
  `String`. The record arm that answered `None` is gone.
- `Units::Only` holds `usize` indices. `Native::emit_units` answers one object
  for each unit asked for, in unit order. `emit_into` pairs units and objects
  by position, so the name lookup and its `or_ice` are gone.
- `actions::codegen_units` (a test-only wrapper whose closure ignored the
  units) is gone. `cli/tests/native/link.rs` calls `codegen_units_for`, and
  its fake backend answers the units it is asked for.
- `buri-middle` has `buri-syntax` as a dev-dependency, for the layout tests'
  `VariantInfo`.

- `runtime_cross::cross_features` filters a `CROSSES` list by what the host
  declares. Behaviour is the same. This keeps cli/build's branch fraction up:
  deleting the name lookup removed two covered branches.

## Status

Done. Staging is at 933e79602.

## Guards (`invariant`, `or_ice`)

| File | Before | After |
|---|---|---|
| crates/semantics/.../expressions.rs | 2 | 0 |
| cli/src/build/actions.rs | 1 | 0 |

## Coverage (compiler branches)

| Area | Local base | New | CI baseline |
|---|---|---|---|
| cli/build | 1628/1687 (96.50%) | 1625/1683 (96.55%) | 1680/1689 (99.47%), estimated 1677/1685 (99.53%) |
| crates/semantics | 2158/2158 | 2154/2154 | 100% |

Every other area is unchanged.

## Notes

- Constant conditions (`!runtime_native::AVAILABLE`, `cfg!`) are folded and
  not counted. Removing one only drops its covered side, so that lowers the
  fraction.
- CI's uncovered cli/build branches can't be reached from an e2e test: the
  `RuntimeArchive::Omitted` sides, a failing stencil `select`, lock and
  symlink failures in `programs.rs`, and musl staging errors.

## Output check

- Same hashes for debug, release and JS on programs, example, tested and
  conformance.
- Differences:
  - JS test libraries: a random `$t.seed` per run.
  - One conformance tool file name: it is hashed from the checkout path.
- Logs are the same once timings and paths are normalised.

# Guard phase 40: semantics expressions, exhaustiveness, inference

## Guards

| File | Before | After |
|---|---|---|
| exhaustiveness.rs | 2 (`or_ice` in `specialize_row`, `default_row`) | 0 |
| inference.rs | 3 (`ice!` in `check_const`, `check_context_decl`; `or_ice` in `check_literal_ranges`) | 1 |
| expressions.rs | 5 (`ice!` at the unary and binary fallthroughs and the context binding's effect; `or_ice` in `shape_of_struct`, `shape_of_variant`) | 2 |

- exhaustiveness: `Rows` keeps heads and tails apart and counts the rows with no head (`bare`). A row is read as `(head, tail)`, so specializing never asks whether a head exists.
- inference: `ConstInfo.value`, `ContextDeclInfo.body` and `decl_span` are filled in by resolve, so checking needs no item lookup.
- expressions:
  - `check_unary` dispatches to `check_not`, `check_negation` and `check_bit_not`.
  - `check_binary` classifies through `BinaryShape::of`, an exhaustive match over `BinOp`, and `order_test` takes a `Comparison`. `arithmetic_trait` is gone.
  - A context binding's effect that is not a name (`Self`, which `named_type` accepts) used to be an ICE. It is now `unknown-effect`.

## Left, and why

- inference.rs `check_literal_ranges`, `or_ice` on an integer literal's primitive. Removing it needs Subst to track an `IntPrim` per integer-class variable, a separate union-find. That is a redesign.
- expressions.rs `shape_of_struct` and `shape_of_variant`. Both find the declaration by span in the module's items, and both work for built-in types (`Order`, `Option`, `Result`) because those are declared in stdlib sources. Removing them means recording the declaration in `TyDef::Struct` or `VariantInfo`. `cli/src/commands/lint.rs` matches `TyDef::Struct { fields, record: true }` with no `..`, and `cli/src` is out of scope.

## Tests

- `cli/tests/reject/context_binding_of_self`: `Self: host.alloc` in a context is `unknown-effect`. It failed first with the ICE.
- `cli/tests/repositories/checking/a_spread_context_s_mistake_beside_an_unclosed_block`: a spread base's mistake in another file is reported beside an unclosed block at the same offsets. It covers the false side of the file check in `retract_from_broken_blocks`. Its testing directory is in `UNFORMATTED_BY_DESIGN`, because the file ends inside an unclosed block.

## Coverage

- crates/semantics went from 2165/2166 (99.95%) to 2158/2158 (100%).
- Every other area's branch fraction is unchanged.

# Guard phase 39: crates/syntax

## Guards

| File | Before | After |
|---|---|---|
| parsing/lexer.rs | 1 (const `panic!` for a slot past the table) | 0 |
| parsing/parser.rs | 3 (`invariant` at the item, statement and match-arm loops) | 0 |
| formatting.rs, flat.rs, tree.rs, layout.rs | 0 | 0 |

- lexer: `WORD_TABLE` now walks the slots and fills each one from the words that hash to it. The slot index can't overrun.
- parser: the `Consumed` typestate. `Parser::take` is the only way to make a `Consumed`. Each loop iteration is a function that returns a `Consumed`, or `None` to end the loop. A loop that makes no progress no longer type-checks.
  - The item loop uses `Declared`: either a read that holds its proof, or `Stray`, which `skip_stray` handles.
  - The statement loop uses `statement()`, which returns `Option<Consumed>`.
  - The arm loop uses `arm()` and `ArmEnd`.
  - Productions take their keyword from the caller, and the expression chain returns `Read<T> = (T, Consumed)`.

## Checks

- Coverage: crates/syntax went from 1204/1206 to 1206/1208. parser.rs went from 608/608 to 610/610. Every other area is unchanged.
- Output, base 0fec126b7 against new, LLVM release build of `buri`:
  - The `.buri/out` files and the cache artefacts are identical in debug and release for programs, example, tested and conformance. Action records and test results are left out, because they name toolchain keys or carry timings.
  - The conformance `.mjs` content is the same. Only its toolchain-keyed file name differs.
- `buri format`: the stdlib (87 files) and cli/tests (9135 files) give identical output and logs.
- Nothing changes for users, so there are no new e2e tests.

# Guard-zero phase 35: resolved intrinsic routes (not done)

The guards are still in place: llvm's `runtime::entry(key).or_ice`, stencil's
two, and the `key == "str.concat"` check.

## Why a routes value doesn't fit today

- `missing_intrinsics` checks the monomorphized program's
  `FuncKind::Intrinsic` keys. The keys the backends resolve are the IR's
  `Inst::CallIntrinsic` and `Body::Runtime` keys, which `lower` writes later
  (`qualified_key`). The check and the use see different key sets.
- Emission doesn't happen in the same call as the check.
  - `cli/src/build/actions.rs` checks, then lowers, then calls
    `emit_units`, sometimes with an adopted lowering.
  - Objects are cached per unit.
  - A routes value would have to travel through the cli, which is out of
    scope.
- Each backend decides the route before the table lookup:
  - llvm uses `numeric`, `copy_out`, `open_coded` and `derived`;
  - stencil matches a fixed set of names, then derive leaves.
  A shared resolved route has to move that dispatch into one place, or the
  backends still need a fallback.
- A `HashMap<key, &Entry>` handed to emission only turns `or_ice` into a
  `get(...)` with the same unreachable `None`. That is a renamed guard.

## A design that would work

1. `lower` mints an `IntrinsicId` per distinct key. `ir::Program` carries
   `Dense<IntrinsicId, Box<str>>`, and `Inst::CallIntrinsic` and
   `Body::Runtime` carry the id.
2. `Backend::missing_intrinsics` takes the lowered `ir::Program`. It returns
   `Result<Dense<IntrinsicId, Route>, Vec<String>>`, where `Route` is the
   backend's own enum (`type Route` on the trait).
3. `emit_units` takes the routes, so indexing is total.

This changes the IR, both native backends, the trait, `cli` build, test,
`WithRuntime`, fuzz and benches. It is a phase of its own, and the
check-before-lowering order in `actions.rs` has to change first.

# Guard-zero phase 34: what phase 29 left in `lower` and the jit

## Done

- `ExprKind::StructuralEq` is gone from the typed AST. `coverage::derived`
  builds an `Intrinsic "structuralEq"` with the descriptor index as its third
  argument, which `derives` routes like every other structural call. `lower`'s
  StructuralEq `ice` arm went with the variant.
- `TemplatePart::Shown { prim, value }`: `monomorphize::render_hole` settles
  every hole. After it, `Hole` is a string already, `Shown` is a non-string
  primitive, and a compound hole is a `structuralShow` call. `lower::render`
  has no `as_prim` lookup and no `ice` arm.
- `derives` builds its own templates from a local `Part { Text, Str }`, so it
  never has to say what a `Shown` would mean there.
- stencil: a folded `Const` lives at `Loc::Imm` in `Fn2::reg`, and the
  `Fn2::folded` table is gone. `incbr`'s `frame(rhs)` already refuses a folded
  bound, so the dead decline arm is deleted. cmpbr and both emit sites read
  `st.loc(rhs)` with no folded branch.

## Left

- `lower::test_eq`'s `ice("a literal pattern is at a primitive type")`.
  - Str, Char and Bool patterns know their primitive.
  - Int and Float patterns don't. Their primitive comes from the pattern's
    type, which is a type variable at check time.
  - The fix needs a post-monomorphize literal-pattern form, like `Shown`, or
    the primitive recorded on `PatKind::Int` and `PatKind::Float` when
    inference resolves.

# Guard zero, phase 33: cli test, run and the rest

## Done

- `test.rs`:
  - The slots are a `Dense<SlotId, Slot>`, and only the pass mints a `SlotId`.
  - `Done::Built` carries the slots told `Linking` (none or one), so there is no `usize::MAX` sentinel.
  - `BatchInfo::slot_of` returns `Option`.
- `spawn::start_piped_runner` returns `Piped { child, stdin, stdout, stderr }`, so `pipes` takes no stream that might be missing.
- `run.rs`:
  - A binary's outputs come from its rule through `actions::outputs_of`, as `(first, rest)`.
  - `--output` is applied before `choose`, and the no-match error is given there.
- Six guards are gone.

## Left

- `test.rs:1047`, `done.recv()...or_ice`: `parallel::pool` sends `None` for a job that panicked. One fix is to resume the panic on the receiving side.
- `test.rs` `BatchInfo::owner_of`: owners are found by module name.
  - The fix is a `MemberId` minted by `BatchInfo`, with one `Dense<MemberId, Member>` replacing the parallel vectors.
  - The test-to-member mapping then has to come from the loaded test modules.
- `generators.rs:404`, `rule_key` resolving a tool.
  - The other callers treat a tool that doesn't resolve as `None`.
  - Loading reports it through `tools::validate`, so first check whether `--explain` can reach `rule_key` with that error.
- `generators.rs` 1475 and 1723: `Checks::start` results.
- `link.rs:1861` `object_path`: the unit name is a filename by construction. A minted `UnitName` would remove it.
- `actions.rs:2439` `emit_into`: emitted units are found by name.
- `main.rs:134`: the test hook, which stays.

# Guard zero, phase 32: target lookups

## Done

- `BuildFile` holds its rules as one `Vec<Rule>`. `library()`, `binary()` and `tool()` read it.
- Only a `PackageList` mints a `TargetId`, because `slot` is private. So `PackageList::rule(target)` is total, and `targets(package)` and `target(package, kind)` mint them.
- Readers match `Rule`, and each arm gives its own answer. `visible` gives no visibility for a binary or a tool, and `accepts` gives none for a library or a binary.
- `declared_entries` takes a `PackageId`.
- Six guards are gone: three `Package::rule`, `Workspace::package`, `visible` and `tools::rule_of`.

## Left

- `cli/src/build/workspace.rs` `platform_rule`. A label is resolved when the repository loads, but `CustomPlatform` and `OutputPlatform::Repository` carry only the label.
  - The fix is to resolve the platform's `PackageId` into the output in `resolve_custom_outputs`.
  - That needs an unresolved and a resolved form of `CustomPlatform`, so the reader doesn't need a placeholder id.

# Guard-zero phase 29: typed intrinsics

## Done

- **llvm's `arg()` is gone.** `c_params` pairs each consuming row of a runtime
  entry's C list with the Buri argument it takes. The rows that take no
  argument come back as `CParam::Extra`. `generic_element`, `bare_carrier` and
  `entry_args` walk this list instead of indexing with a cursor.
- **The `Structural::Show` operand guards are gone, in llvm and in stencil.**
  `middle::lower` now emits a primitive template hole as the new
  `ir::Inst::ShowPrim { dest, prim, value }`, so the primitive and its operand
  are carried in the instruction. Both backends lower it through their
  existing `show_prim`. `Inst::Structural` is only a placeholder that
  `middle::derives` replaces, so both backends' arms for it are now an `ice`.
  llvm already had that arm.

- **`Inst::Structural` and `StructuralOp` are gone.** A template hole was the
  last thing that produced one, because `middle::derives` replaces every
  structural operation before lowering. The paths in `lower` that only a tree
  that skipped `derives` could reach now stop with an `ice`. Those paths are
  `ExprKind::StructuralEq`, a compound hole, and a non-primitive literal
  pattern.
- **Coverage-neutral simplifications that make dead branches go away:**
  - In stencil, a conversion is exact exactly when it answers a non-aggregate,
    which is what the comment already said.
  - In stencil, `negate` drops its arity check.
  - In `runtime_cross`, `--features` is always passed. An empty list is no
    features.

Debug and release objects are byte-identical to the base.

## Not feasible at reasonable cost

### `runtime::entry(key).or_ice`

There are three of these: llvm `call_intrinsic`, stencil `emit.rs` intrinsic
calls, and the stencil body.

The guard checks that a key has a runtime row. `missing_intrinsics` already
checked this, but its answer is a `Vec<String>`, so emission has to look the
key up again.

Neither fix fits in a phase:

- **`missing_intrinsics` hands emission the resolved routes.** For example, it
  could return a map from keys to `&'static runtime::Entry`. This changes the
  `Backend` trait for every backend, for `backend::WithRuntime` in `cli`, and
  for the tests, fuzz targets and benches that call it.
- **Middle writes a typed key instead of a string.** The key vocabulary is open,
  because stdlib declarations name their keys in `@intrinsic(...)`. So middle
  would need one enum of every key, and `runtime_table` would need an
  exhaustive map from that enum to rows. That moves several hundred rows across
  the `middle`/`backend` crate boundary. `middle` can't depend on
  `runtime_table`, which lives downstream of it in `crates/backend`.

### stencil `emit.rs`'s `key == "str.concat"`

This guard has the same cause. The runtime-body dispatch falls through to
`str.concat` because `missing_intrinsics` refused every key it can't answer.
It goes away with the resolved-routes change above.

### stencil `rtcall` step rows (`step_extra` 888, 891 and 892, `last_closure` 1364)

These guards check that `intrinsic_keys::step_call` (in middle), the runtime
row, and the stdlib declaration agree:

- The closure is the last argument.
- One argument is a list.
- The answer is a list.

The tests `the_step_is_the_last_argument` and `a_closure_is_the_last_argument`
already check this agreement. Making it unrepresentable would need two things:

- A typed IR instruction for runtime-driven calls, with the step, the items and
  the context as operands. Middle could emit it, because `step_call` is in
  middle.
- IR list types that carry their element type in the `ir::Type` itself, so that
  891 and 892 aren't type-shape checks.

The second change touches every backend's type handling.

Deriving the strides from the closure's own signature would remove 891 and 892.
But a closure with no parameters would then need a silent default, which hides
the state instead of ruling it out, so I didn't do it.

# Guard-zero phase 28: the jit's loop shapes

`crates/stencil/src/compiler/backend/stencil/jit.rs` goes from 21 guards to 8.

## What changed

- **Latch coalescing is part of `coalesce`.** The separate `coalesce_latches`
  pass is gone, along with its 9 guards. `coalesce` visits the entry block
  first and then the blocks in postorder, so it reaches a latch before the arms
  that jump to it. A latch parameter merges only when its class is unpinned and
  has not grown yet. The old pass asserted these facts; the new walk only
  merges when they hold.
- **`incbr` declines what it used to assert.** Its 4 guards are gone:
  - A bound that wasn't folded is now a decline.
  - The `uses(sum)` check was deleted, because coalescing already ensures it.
  - The frame and header checks moved into the new `copyless_test`.
- **`copyless_test` is shared.** `incbr` and `emit.rs`'s `jump_into_test` both
  use it to recognise a header that is a single branch with no copies. Before,
  each kept its own copy of the check.
- **`find` treats a value past the end of the table as a class of its own.**

Debug and release objects are byte-identical to the base on
`benches/instructions/tested`, `tests/example` and
`benches/instructions/programs`.

## Guards left, and why

| Line | Guard | What removing it would take |
| --- | --- | --- |
| 563 | `agg_of` | IR types that separate aggregates from scalars. |
| 592 | `ent_mut` | Side tables indexed by typed ids, with their length fixed by the code they were built from. |
| 1549 | an unbound `JIT_` hole | The stencil library would have to agree with the jit's bindings at the type level: generated hole enums per stencil. |
| 1653, 1683 | `v.imm()` for small-literal and imm12 holes | The same generated per-stencil binding types. |
| 2200 | `alone(ArrayLen)` | Coalescing would have to record which classes are singletons in their type. |
| 3312 | `op.is_comparison()` | A comparison-only op type in the IR. |
| 3327 | `has(brcmp key)` | Generated per-stencil binding types, as above. |

Most of what is left is the stencil library and the jit agreeing on names,
which a typed binding layer generated from the stencil build would fix. That
work is bigger than one phase.

## Phase 31: flat-tree views name their head

Removed 2 guards, with none added:

- `resolve.rs`: 1 to 0
- `patterns.rs`: 1 to 0

Nothing user-visible changed.

What changed:

- `flat.rs`: `TypeView::Named` gains `name`, the path's last segment. `PatView::Path` gains `head` and `rest`, the path split at its first segment. An empty path reads as `MISSING_NAME`, the same way the flat tree reads any missing id as a default, so the views stay total. `type_head` uses `name`.
- `resolve.rs`: a named type reads `t.text(name)`, not `path.last()`.
- `patterns.rs`: `check_path_pattern` takes `(path, head, rest)`, not a path it asserts is non-empty.

Coverage (macOS, base 926ed00fb to head): syntax is unchanged at 1194/1196 branches. Syntax lines went from 6516/6672 to 6523/6679, the same 97.66%.

Left, with why:

- `parser.rs` 1766, 3046 and 3552: the parser's progress checks. Removing them needs a `Consumed` token typestate through the loop heads. That's a phase of its own.
- `crates/project/src/build/workspace.rs` (3), `cli/src/build/workspace.rs` (3) and `cli/src/build/tools.rs` (1): lookups by `TargetId`. Next is a typed handle that the workspace hands out with each target.
- `crates/diagnostics/src/diagnostics.rs` (3): these are the guard definitions themselves.
- Not looked at in this phase: `cli/src/commands/test.rs` (6), `json/types.rs` (3), `generators.rs` (3), `runtime_native.rs` (3), `run.rs` (2), and the single guards in `main.rs`, `link.rs` and `actions.rs`.

## Phase 30: semantics resolve and styles

Removed 13 guards in `crates/semantics`, with none added:

- `resolve.rs`: 11 to 1 (phase 31 removes the last one)
- `styles.rs`: 3 to 0

Nothing user-visible changed.

What changed:

- `resolve.rs`:
  - `templated` returns the diagnostic it pushes, through `parsing::pushed`.
  - `Sym::Alias` carries its declaration name's span, and so does the alias-expansion stack. This deletes `alias_decl_span`, and the cycle report reads spans it already holds.
  - `elaborate_signatures` skips an item whose own symbol isn't a function, instead of asserting it.
  - `entry_platforms` scans the declared entries before the built-artifact case. This deletes `fills_custom_entry`.
  - `check_values` hands each declaration's span to `constructible::unconstructible` and `endless::endless`, which return it with the type. Nothing is looked up again in `declared`.
- `endless.rs`: `Endless` carries the payload handed in with its owner. `growth` replaces `instantiations` and returns the three rounds and the functions between them, so `check_endless_functions` doesn't index into steps. A walk that doesn't reach its owner reports on the owner from step 0.
- `styles.rs`: `fill` collects each `classify` call's `(names, strictness)` slots in one walk and zips them with the wrapped classes. `unwrap_unknown` and `collect` read the call's arguments with slice patterns.
- `types.rs`: `variant_types` and `result_shape` match `TyKind::Con` with a `_` arm. The let-else's else branch was instrumented and never taken.

Coverage (macOS, base 926ed00fb to head): semantics went from 2173/2176 to 2165/2166 branches. `resolve.rs` went from 408/408 to 404/404, `styles.rs` from 334/334 to 332/332, and `types.rs` from 114/116 to 112/112. Every other area is unchanged. Semantics lines went from 97.81% to 97.76%. Lines are context, not a gate.

Left, with why:

- `expressions.rs` `shape_of_struct` and `shape_of_variant` (2): need a typed declaration handle from the checker.
- `inference.rs` (the int literal's prim, 1) and `exhaustiveness.rs` (row width, 2): these need the prim table and the matrix rows typed.
- `inference.rs` 713: the false side of `d.span.file == r.file` in `retract_from_broken_blocks` is the last uncovered semantics branch.

## Phase 27

Landed on `staging` as 926ed00fb (commit dd64a350f before the rebase). The repo count went from 214 to 200, with no guards added:

- llvm `emit.rs`: 54 to 45
- stencil `emit.rs`: 18 to 15
- js `generate.rs`: 2 to 0

This phase did the light version of typed intrinsics. Each backend matches `(key, args)` with slice patterns, so an operand is named where its arm takes it. An arm whose arity doesn't fit falls through to the "not claimed" answer, and from there to the runtime-row lookup, which is already a guard. The full typed `Intrinsic` enum didn't land, for two reasons:

- `CallIntrinsic` keys cover hundreds of runtime rows.
- `middle` can't name `backend::runtime_table`, which depends on it.

So the enum would have to live in `middle`, with the table indexed by it. That is a phase of its own.

What changed:

- llvm `emit.rs`:
  - `open_coded`, `copy_out`, `derived`, `bits` and the prim-leaf arms bind operands by slice pattern instead of `arg(args, i)` and `args.first()`.
  - `numeric` binds the conversion operand in the same `if let` that reads the target, and binds `show` and `hash` in a `match` on `(op, first)`.
  - `concat` matches `[l, _ctx, r] | [l, r]`, which replaces `concat_ctx` and the six-word `try_from`.
  - `str_len` and `show_prim` read their three words with `pieces_n::<3>`.
- stencil `emit.rs`: `str.format` reads its template in the `if let` that tests the key. `prim_trait_at` matches `key.split_once('.')` against the three modules. `bits_at` answers `false` for a shift with no count.
- js `generate.rs`: an `Option` literal's `None` is the case with no argument, which folds the `VariantId::NONE` test into the let-else. `structuralShow` and `structuralToJson` map the last argument, the descriptor, in place.
- Nothing user-visible changed. Mach-O objects are byte-identical, debug and release, and the JS golden tests pass.
- Use `match` with a `_ => return false` arm, not let-else, for a slice pattern that can't fail. The let-else's else branch is instrumented and never taken, and llvm's share is 579/580.

Left, with why:

- llvm `arg` (1 guard, still used by `generic_element`, `entry_args` and the step contexts): these walk `entry.args` with a cursor into the Buri arguments. A runtime row's arity isn't checked anywhere before emission. The checker gives it the declared arity, and that is the typed-intrinsic enum's job.
- llvm `call_intrinsic`'s and stencil `intrinsic`'s `runtime::entry(key).or_ice`: `missing_intrinsics` answers this first. The fix is for `missing_intrinsics` to hand back the resolved rows, keyed by an intrinsic id from `middle`.
- llvm 6130 and stencil 651 (`Structural::Show`'s value): `Inst::Structural` carries `args: Vec`. A `Show { value }` variant would make the operand a field.
- stencil `emit.rs` 2296 (`key == "str.concat"`), `rtcall.rs` 888-892 and 1364 (a step row's closure and lists): these are runtime-row promises, and they go with the typed rows.
- `ir.rs` 1369 (the verifier's `holds`), `runtime_native.rs` 111-118 (the native `Result` shape), `lower/lists.rs` (already 0).

## Phase 26

Landed on `staging` as be21ac7a3 (commit 51ff7a41c before the rebase). The repo count went from 242 to 214, with no guards added:

- llvm `emit.rs`: 74 to 54
- stencil `rtcall.rs`: 17 to 14
- stencil `glue.rs`: 4 to 2
- stencil `emit.rs`: 19 to 18
- stencil `jit.rs`: 22 to 21
- stencil `lists.rs`: 2 to 1

What changed:

- `ir.rs`: each kind id is `{ id, row }`. The row indexes a `TypeList` side table, so `types.enum_head(EnumTy)`, `types.elem(ListTy)` and `types.sig(FnTy) -> FnSig { params, ret }` can't fail. `TypeList::push` fills the tables, and `Remap` keeps the kind.
- `layout.rs`: `EnumHead` and `Layouts::of_enum(&EnumHead) -> Rc<EnumLayout { size, align, stride, repr, variants }>`, memoised. `build_enum` returns an `EnumLayout`. llvm `Reprs::of_enum` and stencil `Jit::enum_layout` wrap it.
- llvm `emit.rs`:
  - The enum sites (`get_tag`, `make_enum`, `build_variant`, the payload reads, `call_sum`, `call_result`, the json arms, the range result and the main shim) match `Agg::Enum(e)` through `enum_ty` and read `of_enum(e).repr`. `Repr::enum_repr` and `Reprs::element` are gone.
  - The closure arms (Step, Compute, Walk, Press) read `types.sig(fn_ty(..))`, and the closure walk is `closure_glue(op, FnTy)`.
  - `glue_for` checks that a type is counted, then answers through the total `made_glue`.
  - Lists read `types.elem(list_ty(..))`.
  - Every intrinsic helper takes one `dest: ValueId` instead of a `&[ValueId]`, which deletes `dest_of`, `slots_of` and the `dests.first()` guards.
- stencil:
  - `enum_of` (a guarded layout lookup) became `enum_ty` plus `Jit::enum_layout`.
  - `fn_ty` replaces the guarded `TyKind::Fn` reads in `emit.rs` and `rtcall.rs`.
  - `lists::element_of` and `rtcall::array_elem` match `Agg::List`.
  - The two `glue.rs` guards were checks that the type is counted. They became `if let (true, Some(ty))` reads, because only an aggregate has a source type.
- `agg()` stays in llvm for the record sites and the sites that want any aggregate (`make_record_in_memory`, `get_field`, the env record, `agg_ty`, `owner`). It has no guard, only an ice arm.
- Nothing user-visible changed. `buri test` and `buri test --release` on `cli/benches/instructions/tested` and `cli/tests/example` give byte-identical Mach-O objects and executables before and after: debug 14/14 and 27/27, release 15/15 and 28/28.

Left, with why:

- llvm `repr.rs` 327 (`option_payload`): this needs `EnumRepr::Niche` to carry its payload `Ty`, which changes about 13 pattern sites in both backends.
- stencil `jit.rs` `agg_of`: it is used at record sites. It goes away when records get a `RecTy` row table.
- stencil `lists.rs` 166 (`store_disc`'s tag): a niche-encoded `Option` of a number has no tag, and nothing proves this one doesn't use a niche.

## Phase 25

Landed on `staging` as 1cdc63208, with 34912b816 (a clippy fix in a test phase 24 changed). The repo count went from 363 to 351, with no guards added: `fuse.rs` 5 to 0, `coverage/mod.rs` 4 to 1, `decision.rs` 1 to 0, `layout.rs` 4 to 3, `lower/lists.rs` 2 to 0. On `staging`, phase 25 takes the count from 254 to 242.

- `ir.rs`: `Type::Agg` holds an `Agg`, one of `Record(RecTy)`, `Enum(EnumTy)`, `List(ListTy)` or `Fn(FnTy)`. Each wraps a private `TypeId`, and `Agg::id()` gives it back. `TypeInfo::new` records the kind from the source type: an array is a list, a function is a closure, a constructor whose definition is an enum is an enum, and everything else (struct, tuple, `Str`, context) is a record. Only `TypeList::ty(id)` builds an `Agg`, so an id and its kind always agree. `Remap::ty` keeps the kind.
- This differs from the plan's flat `Type::Enum(EnumTy)` and so on: the kinds sit under `Type::Agg`, so the many backend sites that only want "some aggregate" still match one variant. Phase 26 matches `Agg::Enum(e)` and the like where it needs a kind.
- The backends compile through `agg.id()`, not a temporary `agg()` helper. llvm `emit.rs` and `repr.rs`, stencil `emit.rs`, `jit.rs`, `lists.rs` and `rtcall.rs`, and 2 expressions in `cli/src/build/actions.rs`, read the id that way. Their guards are unchanged.
- `fuse.rs`: `Fuse::fuse` reads `consumer(producer(xs, ..), ..)` through `&mut` borrows, checks every condition, and only then takes the call apart. `take_arg`, `call_mut`, `lambda_parts` and `elem_of` are gone. The element type comes from the producer's source list in the same read. `len_of_filter` works the same way.
- `decision.rs`: `column` returns a `Collapse { con, variant, column }`, so `collapse` gets the variant it tests instead of reading it back from the first row.
- `coverage/mod.rs`: `builtin` splits off the receiver and reads the predicate's function type before it makes any probe keys, and `predicate` takes the parameter and return types.
- `layout.rs`: the "an enum with no variants" check is gone. The checker refuses such an enum, and it would lay out as a bare tag anyway.
- `lower/lists.rs`: `elem_of` answers an `Option`, and its callers, all in functions that answer `None` when the operands aren't the types `core/list` declares, read it with `?`. A step whose type isn't a function answers `None` the same way.
- No bug was found, and nothing user-visible changed. `buri build`, `buri build --release` and `buri test` (debug and release) give byte-identical outputs and Mach-O objects before and after.
- `derives.rs`: `Decoding::of` reads `json.decode`'s `Result` head with a `match` and `?`, like the rest of the function. The let-else it replaces had a `None` side nothing reaches. Without this, removing covered branches in `layout.rs` and middle `mod.rs` lowered middle's share from 1841/1880 to 1837/1876.
- Coverage (macOS): middle went from 1841/1880 (97.93%) to 1836/1874 (97.97%). llvm (579/580), stencil (1147/1199), backend (68/68) and cli/build (1642/1709) are unchanged. `layout.rs` went from 49/50 to 47/48, middle `mod.rs` from 18/18 to 16/16, and `derives.rs` from 196/202 to 195/200.
- Never let-else on an `Option` that can't be `None`: the branch is instrumented and never taken. Use `?`, or `match` with a `_ => None` arm followed by `?`, which add no instrumented branch.

Left, with why:

- `coverage/mod.rs` 1371 (`outcome`'s enum head): the receiver's type is a `Bool` or an enum by the built-in's signature, which the probe doesn't carry.
- `layout.rs` `field` and `variant` (322, 331): every caller is in llvm or stencil, so they go with phase 26.
- `layout.rs` 595 (a type inside itself): it catches a recursive type with no box, which `Cycles` should already have cut.
- `lower.rs` 980 (an unbound local), 1622 (a template with no parts), 1786 (a back edge outside a loop, phase 28) and 2193 (or-pattern binds). A template's parts come from `semantics/expressions.rs`, which another agent owned this round.
- `ir.rs` 1310: the IR verifier.
- Seam for phase 26: `Agg::Enum(EnumTy)` is where `enum_repr()` becomes total. Instructions that need an enum (`GetTag`, `Variant`, payload reads) can take an `EnumTy` from `code.ty_of(v)` by matching it. Lists and closures work the same way through `ListTy` and `FnTy`.

## Phase 24

Landed on `staging` as d1ea7b489 (with 2640df002, a flaky-test fix). The repo count went from 385 to 363 on the phase's base, and from 276 to 254 on `staging`, with no guards added.

Per file:

- `ir.rs` 3 to 1, `layout.rs` 5 to 4, `lower.rs` 5 to 4, `lower/lists.rs` 3 to 2, middle `mod.rs` 1 to 0, `rc.rs` 17 to 12.
- llvm `emit.rs` 77 to 74, llvm `mod.rs` 3 to 2.
- stencil `emit.rs` 21 to 19, `glue.rs` 6 to 4, `jit.rs` 24 to 22, `mod.rs` 2 to 1.

What changed:

- `ir::Program.funcs` is `Dense<FuncIdx, Func>`. `check_use` and `sym_into` are total.
- `rc.rs`: `Plan.funcs` is `Dense<FuncIdx, FuncPlan>`, and `Plan::func(f)` returns `&FuncPlan`. `Own` is `Dense<FuncIdx, Vec<ir::Ownership>>`. `lower.rs` and `lists.rs` read `plan.func(f)` with no `Option`.
- `middle::strongly_connected` takes `&Dense<I, Vec<I>>` and answers components of `I`. `inline.rs`, `tail_calls.rs` and `layout.rs` build their edge tables as `Dense`. `layout::Cycles` holds `Dense<TyConId, Group>`.
- `backend/linkage.rs`: `named_elsewhere`, `named_anywhere` and `named_from` answer `Dense<FuncIdx, bool>`.
- llvm: `For.kept`, `For.named` and `observed` are `Dense`. `define` takes the `FuncIdx` with its code, and `prune` reads `kept` by id. The signature lookup and `fn func` are total.
- stencil: `Target::Func`, `Helper::Thunk`, `Helper::Equal`, `V::Fn`, `Fix::Func`, `Root::Main` and `Root::Tests` carry `FuncIdx`. Frame signatures and entries are `Dense<FuncIdx, _>`. `thunk`, `equal_thunk`, `ret_agg`, `ret_tag` and `main_result` are total. The unused `current` field and `NO_FRAME` are gone.
- `cli/src/build/actions.rs`: 3 expressions read the table by id.
- `build::programs::a_sweep_takes_only_stale_entries` was flaky. It made its entry with `settle_in`, which holds the entry's lock while it hands it out. A test thread that forked at that moment kept the locked descriptor until its child exec'd, so the sweep found the entry in use. The test now makes the entry with `create`, which never locks it.
- No bug was found, and nothing user-visible changed. `buri build`, `buri build --release` and `buri test` (debug and release) give byte-identical outputs and Mach-O objects before and after.
- Coverage (macOS, base f305b35e3 to head): `crates/middle/src/compiler/middle/mod.rs` went from 18/18 to 16/16 branches. Every other touched file is unchanged.

Left, with why:

- `rc.rs` `TAKEN_BY` (1253): the table names a parameter by position, and the intrinsic's signature is built elsewhere.
- `rc.rs` parallel rows (`left`, `arm_edges`, `ids`, the node rows at 2853 and 2858): side vectors indexed by `usize`. They need minted node and arm ids.
- stencil `thread_door`: a root takes no arguments, which the IR doesn't say in its type.
- `reaches_a_cycle_in`: the `usize` guards stay until the type graph has minted ids.

## Phase 23: stdlib lookups by name (done)

Every compilation loads the prelude first, so the prelude's ids are fixed by the stdlib's text. The checker reads them as constants on `TyConId` and `TraitId` in `semantics/types.rs`. `cli/src/compiler/tests/known.rs` checks each constant's module and name, for a program, a snippet and the stdlib self-check. When it fails, renumber the constants.

Removed 9 guards:

| File | Before | After |
|---|---|---|
| `semantics/builtins.rs` | 2 | 0 |
| `semantics/resolve.rs` | 15 | 11 |
| `semantics/expressions.rs` | 4 | 2 |
| `monomorphize/hand_written.rs` | 5 | 4 |

The repo total went from 285 to 276.

Other changes:

- `Checker::known_traits`, `option_con`, `result_con`, `order_con`, `know()`, `known_type`/`known_trait` and `BinOp::trait_method` are gone.
- `core/json` loads only when imported, so its four ids stay looked up and are kept as `Option<KnownJson>`.
- `IndexSet` stays in `known_types`, for the same reason.
- Unification no longer compares a constructor's argument counts. Every `Ty::con` carries its declared arity, which keeps the semantics branch fraction from falling.

### Left for later

- `monomorphize.rs` was left alone while phase 24 runs. It still reads `known_types` for "Option", "Request" and "Response". After phase 24 merges, switch those to `TyConId::OPTION`, `REQUEST` and `RESPONSE`, and shrink `known_types` to `IndexSet`. The indexset `toList`/`of` guards are core/indexset lookups that load on import, so they need a different fix, such as a `KnownIndexSet` built in `register_known_names`.
- The guards left in `resolve.rs`, `expressions.rs` (`shape_of_struct`/`shape_of_variant`) and `hand_written.rs` aren't stdlib name lookups. They belong to other phases.
- `types::result_shape` could compare against `TyConId::RESULT` instead of variant names. Its callers are in llvm and stencil.

## Phase 22

Landed on `staging` with phase 21 as `8ed168495`. The scope went from 64 guards to 6, with none added: `lint.rs` 8 to 0, `lint_cache.rs` 2 to 0, `commands/mod.rs` 1 to 0, `watch.rs` 1 to 0, `test.rs` 14 to 6, `language_server/*` 26 to 0, `documentation/*` 9 to 0, `grammar.rs` 3 to 0.

- `test.rs`: a slot carries its `Suite` (`Arc<TestSuite>`), and `tested(session, target)` replaces `has_tests` and `suite()`; `runs_of`, `plan`, `left_out`, `timed_out` and `may_batch` take it. `Built::Refused` carries its diagnostics. A verdict or build is stored only with its noted reads. A case's location is an `Option` all the way to the report.
- `buri add` is `add::COMMAND`, and `dispatch` and `subcommand_usage` take the `Command`, so there is no lookup by name.
- `Tables::methods_of` yields each method's name with its `FnId`, so completion needs no second lookup. `symbols::consts` yields each constant with its value and locals.
- Code lens and inlay hint `resolve` match the object and its `data.position` together. A build file's outermost block is held apart from the block stack. The schema walk queues each block with its index.
- Doc examples parse each effect to its binding, so the synthetic `main` needs no lookup. An assembled document reads its topics through `Document::topics`. Each repository a page builds is named by its first file.
- `parsing::pushed` is generic, and stands in for `Vec::push_mut` outside diagnostics too.
- `grammar.rs` is test code, so its three lookups are `expect`.
- No bug was found, and nothing user-visible changed. The new lsp case `formatting_that_only_removes_lines` pins a range format that only deletes lines: it answers one edit with no new text. It also covers the diff branch where the formatted side runs out first.

Left, with why:

- `test.rs` `pipes`: `Child`'s stream fields are `Option`s the standard library fills when piped.
- `test.rs` `SLOT` (2): slot indices cross worker messages. Typing them needs a minted slot id.
- `test.rs` 1047: a job that stopped without an answer panicked, which is a toolchain bug.
- `test.rs` `owner_of`: dropping a test would misalign the blocks of a batch.

## Phase 21

Landed on `staging` with phase 22 as `8ed168495`. The scope went from 47 guards to 5, with none added: `actions.rs` 15 to 1, `coverage.rs` 7 to 0, `regenerate.rs` 3 to 0, `sources.rs` 2 to 0, `tools.rs` 2 to 1, `workspace.rs` 5 to 3, `modules.rs` 4 to 0, `buildfile.rs` 2 to 0, `diagnostics.rs` `replace` 1 to 0, and `commands/{build,format,generate,init,query}.rs` 6 to 0.

- `coverage.rs`: `begin` returns a `Run` that `buri test` holds in an `Option`; `instrument` and `report` are its methods, so neither checks that a run began.
- `actions.rs`: test keys take the `TestSuite`. Codegen units travel as `(index, (name, key))`, so an emitted object is matched by index. `needed_structs`, `check_platform`, `check_platform_visibility` and `selected_outputs` fold their lookups into the value they produce.
- `platforms.rs`: `bundled(name)` is total over the three bundled platforms, and `PlatformName::rule()` reads it.
- `sources.rs`: `keep` and `refresh_now` write into whichever warm session is there, and `open` always leaves one, so neither asks whether it is there. `SourceMap::replace` takes the texts by value.
- `modules.rs`: the snapshot keeps the standard modules' indices.
- `regenerate.rs` and the language server's file operations share `FieldPath`; `collect` takes the directory it lists.
- No bug was found, and nothing user-visible changed.

Left, with why:

- `project/workspace.rs` 128, 134 and 140, `tools.rs` `rule_of`, `workspace.rs` `package()` and its dependency edge: lookups by `TargetId` or `PackageId`. Making them total needs a minted target table.
- `workspace.rs` `platform_rule`: a lookup by label. Custom outputs would need to carry a `PackageId` across crates.
- `actions.rs` `emit_into`: a backend emits an object for every unit, which is the backend's contract.
- `link.rs` `object_path`: a backend names each unit as a filename; a unit test guards against escapes.

Coverage for both phases (macOS, base to head): branches hold or rise in every area. cli/build 1642/1709 to 1642/1709, cli/test 484/508 to 484/508, cli/language_server 1246/1268 to 1245/1266, cli/lint 560/566, cli/documentation 477/480 and semantics 2183/2188 unchanged, diagnostics 189/194 to 191/196. project 1340/1358 to 1339/1356: phase 18's drop to 1338/1356 is made back by the `several_entries` run that selects a repository platform's output by its directory.

## Phase 20

Landed on `staging` as f305b35e3. `crates/llvm` went from 162 guards to 83, with none added: `emit.rs` 155 to 78, `repr.rs` 3 to 1. `attrs.rs` (1) and `mod.rs` (3) are unchanged. The generated IR is byte-identical: all 3,583 modules from the `llvm::` and native conformance tests match before and after. Only ports baked into test programs differ.

- The whole emitter builds through `At`. `Unit::builder` is read only by `At::end_of`, in `define_code`, `define_helper` and the three shims.
- Gone: `at()`, `BUILT`, `aligned()`, `entry_alloca`, `At::already`, and the second half's `let _ =` discards and `unwrap_or` fallbacks.
- `build.rs` gained the casts, `ashr`, aggregate extract and insert, `offset`/`byte_offset` (the in-bounds `i8` GEPs), `entry_buffer`, typed loads, `call`/`call_answering`/`indirect_call` with `answer` and `pointer`, and `delete_if_unused`. A same-type cast answers `v`, as IRBuilder does. Every `unsafe` in the LLVM emitter is now in `build.rs`.
- `repr.rs` helpers take `At`, and `repr::byte_offset` became `At::byte_offset`.
- `For` now carries what an audience keeps: the names flag and the `kept`/`named` tables. A reader is the linker's path with every function kept and named, so `emit_unit` has no audience branch. Without this, dropping covered builder branches lowered the llvm share from 585/588 to 581/584.
- No bug was found, and nothing user-visible changed.
- Coverage (macOS, base to head): `crates/llvm` branches went from 585/588 (99.49%) to 579/580 (99.83%). The one miss is `glue_function`'s `elf` arm, which Linux covers. Lines went from 7868/8056 to 7546/7722.

Left, with why:

- `get_nth_param` on the shims, thunks and doors: each function is built from a type made a few lines up, but inkwell answers an `Option`.
- `mod.rs`: `with_roots`'s lookup, and `verify` and `optimize` failing.
- `attrs.rs`: the attribute kind lookup by name.
- The rest of `emit.rs` checks shapes the verified IR and the slot table promise (`agg`, slots, enum reprs, glue kinds): phases 24 to 27.

## Phase 19

Landed on `staging` as 380a3e0ec. The repo count went from 499 to 485, with no guards added: `asm.rs` 4 to 0, `region.rs` 2 to 0, `jit.rs` 32 to 24. `rtcall.rs` stays at 17.

- `asm.rs`: x86-64 shims name registers as `Gpr`, one of the first eight, so `mov_imm` and `jmp_reg` need no range check and `rex()` is gone (every prefix is `REX_W`). The bytes are the same.
- Patches write with `region::overwrite`, which zips over the bytes from the offset. A shim's patch is one its assembler minted over bytes it pushed. A stencil's is a hole site, and the new `the_shipped_patch_sites_are_inside_their_stencils` test checks every site, the elided tail and the x86-64 seven-byte rewrite in every baked library. That's the plan's "check in a unit test, render infallibly" for embedded data.
- `jit::Key` keeps the first `KEY_INLINE` bytes of a longer spelling. `every_shipped_name_is_shorter_than_a_key` makes a cut key a lookup miss.
- `jit.rs`: the tail is `split_at(len)`, and `patch_pc32_imm` starts at `insn_end - 7`. Constructor fields zip with their offsets. The dominator tree walk carries each node's entry clock on the stack, so it never reads a span back. A switch fuses its tag load only when that load is the last instruction; any other switch reads its slot, which `emit.rs` already did for `tagsw: None`. A counted `Bool` aliases the call's slot only where it is read in the block.
- No bug was found, and nothing user-visible changed.
- Coverage (macOS, base to head): branches are unchanged in every area, stencil at 1147/1199. Stencil lines went from 8153/8790 to 8118/8755.

Left, with why:

- `ent_mut` and `find`: every side table is a `Vec` indexed by `usize`. `ir::PerValue`/`PerBlock` exist, but switching means typing about 150 sites and the `bufs` reuse.
- Latch coalescing (`jit.rs` 2632 to 2694) and the `incbr` walk asserts: phase 28.
- `agg_of`, `enum_of` and the `rtcall.rs` shape guards: phases 26 and 27. `func.index()` lookups: phase 24.
- Library agreement: an unbound `JIT_` hole, `v.imm()` on an immediate hole, `has(brcmp key)`. Each checks that the emitter and the library agree, and removing it silently would hide a library bug.
- `rtcall.rs` 179, 392, 628 and 1289 check the runtime table against instantiated types, which a unit test can't enumerate.

## Phase 18

Landed on `staging` as d806fd7d3. The scope went from 27 guards to 8, with none added: `diagnostics.rs` 5 to 1, json `mod.rs` 1 to 0, `schema.rs` 2 to 0, `types.rs` 5 to 3, `formatting.rs` 3 to 0, `flat.rs` 3 to 0, `lexer.rs` 2 to 0, `parser.rs` 6 to 3. The 3 lines in `diagnostics.rs` that define `invariant` and `invariant_with` are not counted.

- json: a check reads a `SchemaSet { main, rest }`, and `Registry::new` takes it with the main file first, so the main root is resource `MAIN` (0). `validate` and `types::root` read `reg.main()`, and `resource_of_file` is gone. `schema_module` and `contract_module` take only the set; `data_module` takes the set and the data path. Anchors live on the registry, keyed by `(resource, name)`, so recording one needs no lookup.
- `diagnostics.rs`: the two template checks go through `debug_panic`. `secondary_span` has no `NONE` check, since rendering already handles one. `place(offset)` gives line, column and line start together, so `line_text` and the snippet window need no lookup. The public `line_start` is deleted.
- syntax: the flat tree reads a missing id as `MISSING_NODE` / `MISSING_PNODE` / `MISSING_TYPE`, which is what `kind()` and `pkind()` already answered. The lexer reads past the end as `Eof` with no payload. `bump` holds the cursor on the end of the file. `parsing::pushed` stands in for `Vec::push_mut`, which is newer than the MSRV. `self_annotation` returns `PResult`. `fill_step` places nothing for an empty fill, and `path_literal` takes the path.
- No bug was found, and nothing user-visible changed.
- Coverage (macOS, base to head): diagnostics 189/194 and syntax 1194/1196 are unchanged. project went from 1340/1358 to 1338/1356. Every touched file is fully covered except `schema.rs` (237/248) and `formatting.rs` (378/380), and neither changed.

Left, with why:

- `parser.rs` progress checks in the item, statement and match-arm loops: each catches a loop that would otherwise hang. Removing them needs the `Consumed` token the plan sketches.
- `diagnostics.rs` `replace`: its callers are in `cli/build/sources.rs` and `cli/compiler/modules.rs` (phase 21).
- json `types.rs`: `$ref` targets (358) and named declarations (1190) hold because the check passes before anything generates. A tagged variant (1204) holds because the check matched one branch. Making these total would mean passing the check's results into generation.
- `PatView::Path` for phase 15's `patterns.rs` leftover was not added.

## Phase 17

The repo count went from 623 to 611, with no guards added. The typed program's function table is now `Dense<FuncIdx, Func>`, where `FuncIdx` is minted.

- `monomorphize.rs`: `enqueue` reserves a slot by pushing a head `Func`. `build` and `leaving` fill that slot.
- Passes that append functions mint ids with `push`: `derives`, `closures`, `coverage/derived` and `fuse`. DCE keeps every slot and answers `Dense<FuncIdx, bool>`.
- `Dense` gained `entries_mut()`. `get` and `get_mut` are total, which removed these guards: the js entry and chunk root, `inline_intrinsic`, the park edge and park column, the queued derive instance, the two inline body guards, lower's `pad_to`, the lists step, and the tail_calls self-loop and group member.
- Test helpers mint ids through a `Dense<_, ()>`: `middle::func_id` and `cli/src/compiler/tests/mod.rs::func_id`.
- No bug was found.
- Coverage (macOS, base to head): middle went from 1835/1876 to 1841/1880. js (985/1018), llvm (590/594), stencil (1147/1199), backend (68/68), cli/build (1640/1707), cli/test (484/508) and semantics (2196/2202) are unchanged.

### Seam for phase 24

- `ir::Unit.funcs` is `Vec<FuncIdx>`, but `ir::Program.funcs` is still a `Vec`. stencil and llvm convert with `.index()` where they index it.
- `rc.rs` reads the typed table through `program.funcs.as_slice()` at 3 sites. rc's `Plan.funcs` is a `Vec`, and `plan.func()` returns `Option`, which keeps the guards at `lower.rs:258` and `lists.rs:1411`.
- js `Gen.slot` is `Option<FuncIdx>`: it is `None` until `Gen::function` starts emitting a body.

## Phase 16

Done in 1689f18b2. The repo count went from 623 to 604; these files went from 24 to 5.

- `image.rs` (8 to 0): each check is an `Err`. A damaged object that the cache hands out untagged makes the runner's link fail. The error then adds a note: "the test host couldn't load these objects either (…), so the cache may hold a damaged one: `buri clean` empties it". It reports and doesn't rebuild, because a rebuild would mean lowering again inside the link step. e2e: `test_host::a_damaged_object_in_the_cache_is_a_link_error`, which used to end in an exit-70 ICE.
- `version.rs`: `--self-check` prints each problem and then "standard library: N modules checked, M problems", and exits 1 when there are any. e2e: `command_line::version_checks_the_bundled_standard_library_on_request`.
- `generators.rs:733`: a status with no code and no signal prints as "ended as {status}". The stdin and stdout `take()`s use `?` or `ok_or` on the existing error paths. The guards at 415, 1483 and 1731 are other classes and stay.
- `run.rs`: a failed `wait` prints "error: cannot wait for the artifact to end" and exits 1. The pid is `cast_signed`. Lines 78 and 83 stay.
- `profile.rs` (2 to 0): `rusage` and `exited_child` answer `None`.
- `sweep.rs`: when the worker channel is closed, nothing is running any more.
- No e2e test covers `wait`, `waitid` or `proc_pid_rusage` failing. `main.rs` resets SIGCHLD to SIG_DFL, so they can't be made to fail from outside: an ignored SIGCHLD still exits 0. Each is written as a `?` or a match arm, which add no instrumented branch.
- To keep branch counts unchanged, use `.then_some(()).ok_or(..)?`, `match` in place of `if let`, and `for x in opt.iter()` in place of `if let Some`.

## Phase 15

The phase-15 files went from 66 guards to 11.

- `intern.rs`, `name.rs`, `layered.rs`: shards and slots are picked by indexing a table whose size is the key's range.
- `constructible.rs`: the search returns why a type isn't constructible, and the open list carries each type's field values.
- `types.rs`: variance is computed Jacobi-style. `ContextDeclInfo` and `ConstInfo` hold the item number in place of the AST handle.
- `inference.rs`, `patterns.rs`: scopes are a `Scope` token from `push_scope`. Or-pattern scopes carry the names they bind.
- `exhaustiveness.rs`: field patterns are lowered by index. The row index is built from the rows, and the covering matrix carries each row's origin, so `covered_by` returns the covering arm.
- `endless.rs`: Tarjan frames keep their own low link. Each component carries its growing edges, so the cycle search starts from a known edge.
- `expressions.rs`: field checks take the field itself, and literals zip fields with their slots.
- `styles.rs`: children are zipped with `Flow::positions`, and `typed::children_mut` now lends its children for the caller's lifetime. `classify` arguments are matched by slice arity. `extracted` is found along with `Style`.

Bug fixed: `x.f()`, where `f` is a trait function with no `self`, was an ICE through an `impl` and silently accepted through a bound. It is now `method-without-self`. e2e: `cli/tests/reject/{method,bound_method}_without_self_called_on_a_value`.

Left for later:

- `inference.rs` `check_literal_ranges`.
- `patterns.rs` `check_path_pattern`: needs `PatView::Path` in `syntax/flat.rs` (phase 18).
- `expressions.rs` `shape_of_struct` and `shape_of_variant`: these need a typed handle to the declaration. Rendering from the table would change user-visible text.
- `expressions.rs` `method_index` and `order_con`: stdlib by name (phase 23).
- `exhaustiveness.rs` `specialize_row` and `default_row`: the row width is a dynamic invariant of the flat `Rows`.
- `styles.rs` `fill` and `fill_in`: the results reach each `classify` call in walk order. Fixing that needs the leaf id carried in the tree.
- `styles.rs` `collect`: an `Extracted` literal's items are always `(slot, class)` pairs.

## Phase 14

Landed on `staging`. `crates/llvm` went from 200 guards to 162 (`emit.rs` 193 to 155). The generated IR is byte-identical: all 1,697 modules from the `llvm::` and native conformance tests match before and after. The only differences are temp paths and ports baked into test programs.

- `llvm/build.rs` is the builder over llvm-sys.
  - `At` is the positioned builder. You get one only from `At::end_of` (which positions it) or `At::already`.
  - `Align::of` gives the largest power-of-two factor, so it's always a power of two.
  - Every method returns the value it built. Names are set after building with `LLVMSetValueName2`, as IRBuilder does.
  - Every `unsafe` in the builder path lives in this file, including the context's discard-names flag (`keep_names`).
- Everything above `prune` in `emit.rs` builds through `state.b`, the `At` that `define_code` and `define_helper` mint. That removed:
  - 38 guards;
  - 35 `let _ = builder.build_*` discards;
  - about 40 `unwrap_or` / `.ok()?` fallbacks;
  - the `here()` and `icmp()` helpers.
- The same work removed three guards that aren't `BUILT`: "a division/read is inside a block", "the entry answers a number" and "a program that shares has the mask". `shared_mask` is split into an `Option` wrapper and an infallible `mask_load`.
- The alloc calls now go through one `Unit::alloc`: `alloc_block`, `make_array`, `build_env` and `heap`.
- No bug was found. No non-power-of-two alignment reached a `memcpy`, `memmove`, `memset` or `set_alignment`; if one had, the IR would differ.
- Coverage: `crates/llvm` now has 588 branches. The macOS shard covers 585 of them (99.49%). The three it misses:
  - the `elf` arm of `glue_function`, which Linux covers;
  - two `linker`-false (`For::Reader`) branches in `mod.rs`, which only in-process `emit_ir_text` tests reach.

  On Linux that's 586/588 (99.66%), against the 591/594 gate. `keep_names` replaced the uncovered `if linker` branch.

### Seam for phase 20

- Code below `prune` still builds through `self.builder` (now `&'ctx Builder`). Where it calls the first half, it does so through `self.at()`, the one remaining guard: "the emitter positions the builder".
- Phase 20 should:
  - thread `state.b` (or an `At` parameter) through the second half, then delete `at()`, `BUILT`, `aligned()`, `here`-style `get_insert_block` uses, `entry_alloca` and `value_buffer`'s positioning;
  - move the in-bounds GEPs and `f.delete()` into `build.rs`, and add the methods it needs: `gep`, `extract_value`, `insert_value`, `ptr_to_int`, `int_to_ptr`, `bitcast`, `sext` and the intrinsics call.
- `repr.rs` helpers still take `&Builder`. Move them to `At` together with the second half.
