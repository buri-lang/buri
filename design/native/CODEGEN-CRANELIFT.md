# The Cranelift backend

The dev backend: `buri build`, `buri run`, and every `buri test` on a native
platform. It is chosen for `(Linux | Macos, Debug)` (ARCHITECTURE.md §4), and it
is chosen because compile time is the only thing that matters in that quadrant.

Facts about Cranelift in this document were checked against
`cranelift-codegen` 0.134.3 (2026-07-31) and the wasmtime 36 LTS line
(`cranelift-* 0.123.x`), which is what this design pins.

## 1. The IR it consumes

`middle::ir`, produced by `middle::lower` (ARCHITECTURE.md §2.1). One function is:

```rust
pub struct Func {
    pub sig: Sig,                  // flattened scalars, per VALUE-MODEL.md §5.1
    pub blocks: Vec<Block>,        // blocks[0] is the entry
    pub unit: u32,                 // the codegen unit (ARCHITECTURE.md §5)
    pub facts: Facts,              // per-parameter own/borrow, purity, nounwind
}

pub struct Block {
    pub params: Vec<(ValueId, Type)>,
    pub insts: Vec<Inst>,
    pub term: Term,
}

pub enum Term {
    Jump(BlockId, Vec<ValueId>),
    Branch { cond: ValueId, then: (BlockId, Vec<ValueId>), else_: (BlockId, Vec<ValueId>) },
    /// A dense discriminant switch. `default` is `None` where the middle end
    /// proved the table total, which for an enum is always.
    Switch { on: ValueId, cases: Vec<(u64, BlockId, Vec<ValueId>)>, default: Option<BlockId> },
    Return(Vec<ValueId>),
    Unreachable,
}
```

Block-argument SSA, no phi instruction, every value defined once. That is
**Cranelift's own IR shape** — `cranelift/docs/ir.md`: "Cranelift does not have
phi instructions but uses BB parameters instead" — which is not a coincidence.
The middle IR was designed to be a transliteration into CLIF and a mechanical
lowering into LLVM's phis (CODEGEN-LLVM.md §2), rather than a compromise between
the two.

## 2. Constructing CLIF

### 2.1 `FunctionBuilder`, and never a `Variable`

`cranelift-frontend`'s `FunctionBuilder` runs the Braun et al. algorithm over
`declare_var`/`def_var`/`use_var` and inserts block parameters for you
(`cranelift/frontend/src/ssa.rs` cites the paper). We do not use any of it.

The IR is already in SSA. Every value is either a block parameter or an
instruction result, both of which are `ir::Value` directly. So the lowering:

1. creates every block up front with `create_block()`,
2. appends every block's parameters with `append_block_param(block, ty)`,
3. calls `seal_block` on each one,
4. then fills bodies with `switch_to_block` + `ins()`.

Step 1 before step 4 is what makes step 3 unconditional: `seal_block` means "all
predecessors are known", and after the whole CFG exists they are. There is no
sealing discipline to get wrong, which is the failure mode the `FunctionBuilder`
docs warn about ("forgetting to call this method on every block will cause
inconsistencies in the produced functions").

> **Correction, from the implementation (wave 2a).** Step 3 cannot be step 3.
> `SSABuilder` does not learn a block's predecessors when the block is created;
> it learns them at each branch, and `declare_block_predecessor` asserts
> `!is_sealed(block)`. Sealing every block before filling any body therefore
> trips that assertion on the first jump in the function. The landed backend
> seals in exactly one place — `seal_all_blocks()` after the body is complete —
> and the substance of this section is unchanged, because sealing exists only
> to drive the variable-to-phi construction the paragraph above declines to
> use. Steps 1, 2 and 4 are as written, and there is still no sealing
> discipline to get wrong.

`append_block_param`'s own doc comment says it "has to be called at the creation
of the `Block` before adding instructions to it, otherwise this could interfere
with SSA construction". Appending all parameters in step 2 satisfies that by
construction, and never touching a `Variable` means the SSA constructor has
nothing to interfere with.

The alternative — bypassing `cranelift-frontend` entirely and driving
`cranelift_codegen::cursor::FuncCursor` — is rejected. It saves one crate that is
already in the dependency closure via `cranelift-module`, and it puts the
lower-level, less-documented API on the maintenance path across Cranelift
version bumps.

### 2.2 Signatures

`CallConv` is the **platform default** — `SystemV` on Linux, `AppleAarch64` on
macOS — obtained from `isa.default_call_conv()`. Not `CallConv::Tail`. §3.3 gives
the reasons.

Parameters are the flattened scalar leaves of VALUE-MODEL.md §5.1, so a `Str`
parameter is three `AbiParam`s of `I64` and nothing in a signature is an
aggregate. Neither `StructReturn` nor `sret` appears anywhere: a function
returning an aggregate returns its leaves as multiple results, and Cranelift's
`enable_multi_ret_implicit_sret` (default `false`) is left alone because it never
applies.

Zero-sized parameters — every context built from `core/host`
(VALUE-MODEL.md §8) — are dropped by the layout pass before a signature is built.

## 3. Lowering the interesting constructs

### 3.1 `Switch` and decision trees

`middle::decision` (ARCHITECTURE.md §2.2) has already turned the arm list into a
tree over discriminants, so this backend never sees an arm chain. A `Term::Switch`
over a dense range lowers to `br_table` and over a sparse one to a balanced
comparison tree, chosen by density:

```
dense  (max - min < 4 * cases):  isub  v, min
                                 br_table v, default, [b0, b1, ...]
sparse:                          a balanced binary tree of  icmp / brif
```

`cranelift-frontend`'s own `Switch` helper does exactly this partitioning and is
used rather than reimplemented.

For an enum, `default` is `None`: exhaustiveness is proved
(`exhaustiveness.rs`), and `Profile::defensive_aborts` (`generate.rs:54`) decides
whether an unreachable default block calling `buri_rt_abort_unreachable` is
emitted anyway. On
the Cranelift path `defensive_aborts` is on, because this is the debug backend
and the belt is cheap.

Pattern *bindings* are projections, not tests: once a block is entered, the
payload fields are `load`s at known offsets from the enum's payload area
(VALUE-MODEL.md §6), with the tag load hoisted to the switching block.

### 3.2 Closures and indirect calls

A `Closure` is `{ code, env }` (VALUE-MODEL.md §7). A call through one is

```
    v_code = load.i64  v_clo+0
    v_env  = load.i64  v_clo+8
    call_indirect  sig, v_code, v_env, args...
```

`call_indirect` needs a `SigRef`, which is `func.import_signature(sig)` — one per
distinct flattened signature, interned per function.

The middle end has already rewritten a call through a *known* closure into a
direct `call`, and `ExprKind::FnRef` into a closure with a null environment
(VALUE-MODEL.md §7), so `call_indirect` appears only where the callee is genuinely
a value.

### 3.3 Tail calls: the middle end does it, and this backend does not

**Status of `return_call`.** It is production. `return_call`/`return_call_indirect`
and `CallConv::Tail` landed in cranelift 0.93 / wasmtime 6.0.0 (2023-02-20);
x86-64 lowering in 0.98 / wasmtime 11; aarch64 and riscv64 in 0.99 / wasmtime 12;
s390x in 0.111 / wasmtime 24. It is the unconditional Wasm calling convention in
wasmtime since 24, so it has heavy production mileage, and it works with
`cranelift-object` — a direct `return_call` to a colocated symbol emits an
ordinary branch plus an ordinary call relocation
(`Reloc::X86CallPCRel4` / `Reloc::Arm64Call`), which `cranelift-object` maps to
`R_X86_64_PLT32` / `R_AARCH64_CALL26`.

**We do not use it.** Four reasons, in order of weight:

1. **There is nothing left for it to do.** `middle::tail_calls` rewrites a
   self-recursive tail call into a loop and a mutually-recursive group into one
   dispatching function (`tail_calls.rs:9-16`). What remains is a tail call to a
   function *outside* the caller's tail-call SCC — and after SCC merging the
   tail-call graph is a DAG, so any such chain is bounded by the DAG's longest
   path, a compile-time constant. Constant stack (SPEC 8.3) is delivered by the
   middle end, not by the backend, on every backend including JavaScript.
2. **It is viral and incompatible with the runtime.** `CallConv::Tail` must match
   on both sides (`verifier/mod.rs::typecheck_tail_call`: "callee's calling
   convention must match caller"), and `cli/runtime` is a C-ABI archive
   (VALUE-MODEL.md §10). Adopting `Tail` would mean a C-ABI shim in front of all
   203 intrinsics.
3. **`CallConv::Tail` is documented as not ABI-stable.** Objects produced by two
   Cranelift versions could not tail-call each other, and this design caches
   objects across toolchain versions by key (ARCHITECTURE.md §6.2). An unstable
   ABI in the cached artifact is a stale-cache bug waiting for a version bump.
4. **A latent panic on x86-64.** `isa/x64/inst/emit.rs` asserts
   `info.flags.preserve_frame_pointers()` before emitting a tail call, and that
   flag defaults to `false`. Wasmtime forces it true, so the trap is invisible
   upstream and would be found here, at run time, in release mode.

What we lose: an *indirect* tail call — a closure tail-calling itself through a
value — is not eliminated. `tail_callees` collects only `ExprKind::CallFn`
(`tail_calls.rs:82`), so this is already the state on the JavaScript backend and
is not a native regression. It is a middle-end gap, and the fix, when someone
wants it, is a middle-end one: an indirect tail call in a function whose only
indirect callee is itself is a loop.

### 3.4 Effects and context calls

There is nothing to lower. `monomorphize::resolve_trait_call`
(`monomorphize.rs:709-741`) has already turned every effect method into a direct
call, and a context of zero-sized implementations is dropped entirely by the
layout pass (VALUE-MODEL.md §8). A context that does carry state is an ordinary
struct parameter, flattened like any other.

`$host_*` intrinsics become `call` to an imported `buri_rt_host_*` symbol. The
prefix is `buri_rt_` for every runtime entry without exception (VALUE-MODEL.md
§10), and `cli/runtime/lib.rs`'s module comment is the ABI contract: a `Str`
argument is three parameters, an aggregate result leaves through an
out-pointer, and a `Result` returns `-1` for its success arm and the error
variant's index otherwise.

### 3.5 Reference counting

`incref` and `decref` are open-coded (MEMORY.md §5.1), not called:

```
    ;; incref v_p                          ;; decref v_p, cold path in b_free
    v_rc  = load.i64  v_p-16               v_rc = load.i64 v_p-16
    v_1   = iconst.i64 1                   brif (v_rc == IMMORTAL), b_done, b_live
    v_n   = uadd_sat.i64 v_rc, v_1     b_live:
    store  v_n, v_p-16                     brif (v_rc == 1), b_free, b_dec
                                       b_dec:
                                           v_d = isub v_rc, 1
                                           store v_d, v_p-16
                                           jump b_done
                                       b_free:
                                           call drop_T(v_p)
                                           jump b_done
```

The `b_free` block is marked with `set_cold_block`, which moves it out of the hot
path in the final layout — `LLVM-tips.md:4`'s cache-locality instruction, and
Cranelift gives it directly.

> **Correction, from the implementation (wave 2a).** `uadd_sat` is a Cranelift
> instruction and is **vector-only**: its controlling type set requires `lanes
> >= 2`, so `uadd_sat.i64` is rejected by the verifier outright. The scalar
> spelling of a saturating increment is `iadd_imm` plus a compare against
> `IMMORTAL` plus a `select` — three nodes rather than one, still branchless,
> and `IMMORTAL` still needs no cold path.

`drop_T` is a generated per-type function; the null tests in front of both
sequences are emitted only where the layout says the pointer is nullable, which
is only a niche-encoded `Option` (VALUE-MODEL.md §6).

### 3.6 `I128`

Cranelift supports `I128` arithmetic on x86-64 and aarch64. Where an operation on
`I128` is not natively lowered on the target, the middle end legalizes it into a
pair of `I64`s with explicit carry — a legalization pass in `middle`, so the
LLVM backend gets the same rewrite if it ever needs it, and both backends produce
the same answers. `I128` division and remainder always go to the runtime
(`buri_rt_i128_divmod`), on both backends, because that is a hundred
instructions nobody should inline. Its operands cross as **pairs of `I64`s, low
half first**, rather than as `I128`: the contract says a parameter is a scalar
leaf, and passing a pair means neither backend has to agree with the platform
ABI about how a 128-bit integer is classified.

### 3.7 Aborts

`buri_rt_abort(msg_ptr, msg_len)` is `noreturn`, and so is each of the fixed
messages beside it — `buri_rt_abort_div_zero`, `buri_rt_abort_shift`,
`buri_rt_abort_bounds`, `buri_rt_abort_unreachable` — which exist so that a
message pinned by `cli/tests/crash/` lives in the runtime rather than in two
backends' string tables. Cranelift has no `noreturn`
attribute, so the call is followed by `trap(TrapCode::UnreachableCodeReached)`
and the block terminates. Division by zero (`runtime.js:44-47`, SPEC 6.2) is a
`brif` on the divisor into a cold block that calls it.

## 4. Settings

Cranelift's `settings::Flags` defaults are wrong for this use in three places, so
all three are set explicitly rather than inherited:

| Flag | Default | Ours | Why |
|---|---|---|---|
| `opt_level` | `none` | `none` | Correct by accident. This is the dev backend; the middle end already optimized. |
| `enable_verifier` | `true` | `cfg!(debug_assertions)` | The verifier checks *our* lowering. It is worth its cost in a toolchain built with assertions and is pure overhead in a release toolchain. |
| `is_pic` | `false` | `true` | Every artifact is PIE. |
| `preserve_frame_pointers` | `false` | `true` | §5 — backtraces come from frame pointers because there is no DWARF. |
| `unwind_info` | `true` | `false` | §5. |
| `enable_alias_analysis` | `true` | — | Only effective at `opt_level != none`, so inert here. |

`regalloc_algorithm` (`backtracking` vs `single_pass`) is the more useful
compile-speed dial than `opt_level` at this setting, and is set to `single_pass`
where the pinned version offers it. It is the one knob whose value should be
re-measured rather than assumed, because it trades compile time for spills in a
profile where the generated code still has to be usable.

The aegraph mid-end (GVN, constant folding, ISLE rewrites, LICM, alias analysis
with redundant-load elimination) is entirely skipped at `opt_level = none` —
`context.rs` gates `egraph_pass` on it. That is intentional: `middle` has already
done constant folding and inlining, and the remainder is what `--release` and
LLVM are for. It is also what `rustc_codegen_cranelift` does — `opt_level = "none"`
for debug builds — so the shape is battle-tested.

`Context::inline` (0.123 / wasmtime 36) exists and is not called. Inlining
happened in the middle end over an exact call graph (`monomorphize.rs:8-10`);
re-deriving it here with less information would cost compile time in the profile
that has none to spend.

## 5. Debug info and backtraces

**No DWARF in v1.** `cranelift-object` emits none, and producing it means
building `.debug_*` sections with `gimli` by hand — which is what
`rustc_codegen_cranelift/src/debuginfo/` is, a seven-file subsystem. That is a
wave of its own and it is not this one.

**No `.eh_frame` either.** `ObjectBuilder::unwind_info(bool)` landed in
cranelift-object 0.133.0 and emits `.eh_frame` on ELF and COFF; on Mach-O it is
**not implemented and `finish()` panics**. Since the language has no exceptions —
an abort is a write to stderr and `_exit`, not an unwind (SPEC 6.10,
`generate.rs:326-334`) — there is nothing to unwind, and a feature that panics on
one of the two target platforms is not one to build on.

What replaces both: `preserve_frame_pointers = true`, and `buri_rt_abort` walking
the frame-pointer chain, resolving each return address against a **symbol table
the compiler emits itself** — a sorted `(address, name)` array in a
`.buri_symbols` section, written from the `ObjectProduct.functions` map. That is
about eighty lines and gives an abort a stack trace with function names, which is
the whole of what a debug build needs. Line numbers wait for DWARF.

The escape hatch when someone needs a real debugger is the JavaScript backend,
which keeps names and structure (`generate.rs:44-46`), and `--release`, which
will get DWARF from LLVM for free when CODEGEN-LLVM.md §7 lands.

## 6. Object emission

One `ObjectModule` per codegen unit (ARCHITECTURE.md §5) — the shape
`rustc_codegen_cranelift` uses, one module per CGU, written out with
`product.object.write_stream`.

```rust
let mut builder = ObjectBuilder::new(isa, unit_name, default_libcall_names())?;
builder.per_function_section(true);
builder.per_data_object_section(true);
let mut module = ObjectModule::new(builder);
// declare every function in the unit and every symbol it imports, then define
let product = module.finish();
let bytes = product.emit()?;
```

`per_function_section(true)` puts each function in its own `.text.<name>`
subsection so the linker's `--gc-sections` can drop what nothing reaches. It is
the right lever for fine granularity, and it is the reason **one object per
function is not**: a per-function object would make every intra-unit call an
`Import`, which is non-colocated, which turns a direct `call rel32` into a
GOT-indirect call. Per-function sections give the dead-code granularity without
the codegen regression or the thousand-member link.

`Linkage` per function:

| Function | Linkage | Effect |
|---|---|---|
| the entry point, `_start`-adjacent | `Export` | |
| a function called from another unit | `Hidden` | visible to the link, not to a dynamic symbol table; `is_final()` is true so calls stay colocated and direct |
| a function called only within its unit | `Local` | |
| a `buri_*` runtime entry | `Import` | resolved from the archive |

`Hidden` rather than `Export` for cross-unit calls is the load-bearing choice:
`Linkage::is_final()` is true for `Local | Hidden | Export`, and that drives
`colocated` and therefore direct rather than GOT-indirect calls. `Preemptible`
is never used — nothing in a Buri artifact may be interposed.

On Mach-O, `set_subsections_via_symbols()` is set by `cranelift-object` already.
`LC_BUILD_VERSION` is set by `cranelift-object`'s `macho_build_version(triple)`;
without it `ld` warns about an object with no platform, so it matters that this
happens and it does.

### 6.1 Reproducibility of the object bytes

Two things are checked rather than assumed, because ARCHITECTURE.md §7 compares
these bytes:

- **Symbol and section order is a function of declaration order**, and declaration
  order is the middle end's function order, which is source order
  (`monomorphize.rs:247-248`). Nothing in the emission path iterates a `HashMap`.
- **No timestamps.** `object::write` writes none for ELF or Mach-O relocatable
  objects. The archive step (§7) is where a timestamp could enter, and it is
  zeroed there.

## 7. Linking, and what "incremental" honestly means

This is the part where the plan meets the research and changes.

### 7.1 Neither mold nor lld does incremental linking

**mold has no incremental mode and will not get one.** There is no `--incremental`
flag; `docs/design.md` has a "Rejected ideas" section that gives three reasons,
and the third is the one that settles it here:

> It's not reproducible, so your binary isn't going to be the same as other
> binaries even if you are compiling the same source tree.

That is a direct contradiction of this toolchain's central claim
(`build.rs:128-133`, `TODO.md:1097-1101`). An incremental linker and
`--check-reproducible` cannot both be right. The author's conclusion — "I wanted
to make full link as fast as possible, so that we don't have to think about how
to work around the slowness of full link" — is the plan.

**LLD has none either**, and it is a documented non-goal; its design is "do less
rather than do it efficiently", plus parallelism.

The only shipping incremental linkers worth naming are MSVC's `/INCREMENTAL`,
which pads code and inserts thunks and which Microsoft says not to ship, and
which is defeated by *any object file added or removed* — a condition a
monomorphizing compiler violates constantly; GNU gold's, which is unfinished,
disables `.eh_frame_hdr`, and whose null incremental link of Chrome took about
thirty seconds; and Zig's in-place binary patcher, which is real and impressive
and whose **Mach-O backend is still not done** (`ziglang/zig#21165`, unchecked as
of 2026-08-03). `wild`, the Rust linker designed around incremental linking,
states in its own README that "the plan is to eventually make it incremental,
however that isn't yet implemented".

### 7.2 So the granularity is re-compile, not re-link

"Swap only the object files that changed" is delivered, and it is delivered
above the linker rather than inside it:

- A codegen unit whose IR hash is unchanged is **not recompiled**. Its object
  comes out of the content-addressed cache (ARCHITECTURE.md §6.2). This is where
  the seconds are: LLVM codegen of a unit is measured in hundreds of
  milliseconds, Cranelift's in tens.
- The link is **always full**, and it is fast enough that the distinction does
  not matter at this scale. mold links MySQL 8.3 in 0.46 s and Chromium in 1.5 s;
  a Buri artifact is one to two orders of magnitude smaller than either.
- If **every** unit's key is unchanged and the artifact exists, the link is
  skipped entirely, because the `link` key covers the ordered unit keys
  (ARCHITECTURE.md §6.2). The fastest link is the one that does not run, and that
  is the case a watch loop hits on every keystroke in a comment.

This is also what rustc does, and rustc is the closest comparable: 256 codegen
units in incremental mode, 16 otherwise, per-CGU object reuse driven by the
dep-graph, and a **full external link every single time**. Its own incremental
linking issue has been open since 2016.

### 7.3 Linker selection

The link is driven through the platform C compiler (`cc`, or `$CC`), never by
invoking the linker directly. The driver is what knows where `crt1.o`,
`libc`, and `libSystem.tbd` live, and reimplementing that is reimplementing the
part of a toolchain that changes with every OS release.

**Linux**, in order: `mold`, `ld.lld`, the system default.

```
cc -fuse-ld=mold -o <artifact> <units...> libburi_rt.a -static-pie \
   -Wl,--gc-sections -Wl,--build-id=none
```

mold is a drop-in for GNU ld and accepts its options; it is chosen first because
it is 3-10x faster than lld on the benchmarks its README publishes, with the
honest caveat that the advantage is core-count-dependent — mold saturates every
core and lld often does not, so on two cores the gap is much smaller.
`--build-id=none` because a build id is a hash of content we are about to compare
byte for byte, and one fewer thing in the way.

**macOS**: the system `ld`. `ld64.lld` when `--linker=lld` names it or when no
system linker is found.

```
cc -o <artifact> <units...> libburi_rt.a \
   -Wl,-no_uuid -Wl,-dead_strip -Wl,-platform_version,macos,<min>,<sdk>
```

mold is **not** an option on macOS: it is ELF-only, has no `macho/` directory,
and fails with "mold does not support macOS". The Mach-O fork, `sold`, was
open-sourced in March 2024 and its repository **archived in November 2024**, with
the author's own note recommending Apple's linker instead. `ld64.lld` is
production-quality and actively maintained — the LLVM `lld/MachO` tree is under
heavy current development — and remains the choice when hermeticity across
machines matters more than matching the platform.

The system linker is the default anyway because Apple's is what every macOS SDK
assumption is built around, it closed most of the historical speed gap in Xcode
15, and it is guaranteed present wherever a C toolchain is.

`-no_uuid` is not optional: ARCHITECTURE.md §7 compares linked artifacts byte for
byte, and `LC_UUID` is the one field that would differ every time.

**Fallback.** With neither mold nor lld present, `cc` uses whatever the system
provides and everything works, more slowly. There is **no** case in which the
build fails for want of a fast linker, and no flag that has to be set to get a
working build. That is the whole of the fallback story, and it is short because
the design does not depend on the linker being any particular one.

### 7.4 The manifest

`.buri/link/<link-key>/manifest` is one line per unit:

```
core_list      3f9a1c2b8d4e...  cached
core_str       71c0aa38f5b1...  cached
lib_money      c40e19b7ad22...  run
main           8b2e01f4c7a9...  run
```

It is written by the link step and read by `--explain`, which prints one
`codegen` line per unit in the existing format (`cache.rs:238-268`). It is the
answer to "which objects changed", and it is the thing that makes the claim in
§7.2 observable from outside — which is the standard the rest of this build
system already holds itself to (`arguments.rs:76-79`: the build system's claims are
about which actions run, and a claim nothing can observe is not one anybody can
hold the toolchain to).

## 8. Version pinning

```toml
cranelift-codegen   = { version = "0.123", default-features = false, features = ["std", "unwind", "all-arch"] }
cranelift-frontend  = "0.123"
cranelift-module    = "0.123"
cranelift-object    = "0.123"
cranelift-native    = "0.123"
```

**0.123.x is the wasmtime 36 LTS line**, supported for 24 months, MSRV 1.86.0.
Cranelift versions in lockstep with wasmtime — a semver-major every month on the
20th, with cranelift's minor equal to wasmtime's major plus 87 — and a non-LTS
line goes unsupported after two months.

The reason to pin the LTS rather than track latest is specific to this design:
`Backend::identity()` enters every `codegen` cache key (ARCHITECTURE.md §3, §6.2),
so **a Cranelift bump invalidates every cached object in every repository**. At
the monthly cadence that is churn nobody asked for; at the annual LTS cadence it
is a deliberate, reviewable event that lands with a toolchain version bump, which
every repository already pins by hash (`build/toolchain.rs`).

What the LTS costs, named so nobody rediscovers it: `ObjectBuilder::unwind_info`
(0.133) — not wanted, §5 — and whatever `regalloc_algorithm` values the newer
lines offer. `Context::inline` is in 0.123 and is not used (§4). `declare_var`'s
current signature is in 0.122 and is not used either (§2.1).

`all-arch` rather than the default `host-arch`, so that the ISA is selected by
triple and the refusal to cross-compile (ARCHITECTURE.md §9) is a decision about
the runtime archive rather than a limitation of the backend. `cranelift-native`
supplies `infer_native_flags` for host CPU features.

`object` is **not** a direct dependency: `cranelift-object` depends on it and
re-exports it, so version skew is a compile error rather than a runtime surprise.
