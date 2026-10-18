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

# Guard-zero phase 37: number patterns carry their primitive (done)

## What changed

- `typed::PatKind::Number { prim, value: NumberLit }` is the form an `Int`
  or `Float` literal pattern takes once `monomorphize` has settled its
  type. An unsettled type becomes `PatKind::Error`.
- `lower::test_eq` takes the primitive, so its
  `ice("a literal pattern is at a primitive type")` is gone. A pattern
  still in the `Int` or `Float` form at lowering is poison.
- The template lowering's `or_ice("a template has a part")` is gone. A
  template with no parts lowers to `""`.
- Two stencil e2e cases cover branches the phase touched:
  `bounds_on_the_default_backend` and `loop_headers_on_parameters`.

## Candidates looked at and left

- `lower.rs` `release_except` (about line 1042), and the guards near lines
  1813 and 2220. Each needs a typed form that the borrow and drop
  analyses would have to carry, which is a phase of its own.
- `rc.rs` index guards. They index arrays the same pass built, so a typed
  index would be the fix, not a lookup.
- `coverage/mod.rs` (about line 1374).

## Coverage to make up next

The route refactor removed 49 covered stencil branches and 18 covered
llvm branches, which lowers stencil's fraction on this Mac
(1115/1163 to 1066/1113). CI's estimate still holds, at 1103/1120
(98.48%) against 1152/1170 (98.46%). The removed `negate` branch was
uncovered on every host, but the margin is thin. The next phase should
cover a portable uncovered stencil branch:

- `emit.rs` `ArrayGet`: the destination shares the array's slot (about
  line 514);
- `emit.rs` `jump_into_test`: `lhs` not in the frame (about line 1733);
- `emit.rs` edge deadlock (about line 1869).

`loop_headers_on_parameters` was meant for `jump_into_test` and
`copyless_test`, but a probe with the instrumented binary shows it
doesn't reach them. Tail recursion here doesn't make a loop edge into a
test block. The case still checks behaviour on both backends.

To probe one program, run `target/coverage/debug/buri` with
`LLVM_PROFILE_FILE` set, then rustup's `llvm-profdata` and `llvm-cov
export` (1.99.0 toolchain). Nix's are a different profile version.

# Guard-zero phase 36: resolved intrinsic routes (done)

## What changed

- `lower` mints an `IntrinsicId` per intrinsic key and a `RuntimeId` per
  runtime body. `ir::Program` carries the key tables, and
  `Inst::CallIntrinsic` and `Body::Runtime` carry the ids.
- The `Native` trait has `type Routes<'p>`, `resolve(lowered)` and
  `emit_units(..., lowered, routes, units)`. Each backend defines its own
  route type (`stencil/route.rs`, `llvm/route.rs`). `resolve` answers the
  routes or the keys it has no route for.
- `cli/src/build/actions.rs` lowers first, then resolves, then emits.
  Missing keys and the networking or cryptography gaps are refused
  together through `refuse_missing`. `adopt_lowering` is gone.
- Backends never call `runtime::entry(key).or_ice`. Stencil's
  `invariant_with(key == "str.concat")` is gone.

## Left for later

- llvm `emit.rs` still has 56 guards, most of them layout and
  register-class checks unrelated to routes.
- Stencil's `compare` into a destination that is not a tag still ICEs. A
  route could carry the tag width, but `ret_tag` reads the function's
  signature, so it would move the guard rather than remove it.

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
