# The copy-and-patch backend

The dev backend: `buri build`, `buri run`, and every `buri test` on a native
platform. It is chosen for `(Linux | Macos, Debug)` (ARCHITECTURE.md §4), and it
is chosen because compile time is the only thing that matters in that quadrant.
It is behind `backend-stencil`, which is on by default and off on a host with no
C compiler.

**It took that seat on 2026-08-29, and §13 is the record.** Until then it was
compiled in and never selected, because selecting it was a decision about parity
rather than about plumbing, and the backend it was written to replace —
Cranelift — held the quadrant. The gate the seat was behind was correctness
parity and a re-benchmark; both were answered, the decision was taken, and the
Cranelift backend was removed from the tree with its design document. Every
comparison against it below is a measurement taken while it was still there, and
§13 says which numbers those are.

**Parity was met.** Driven through `buri build` and `buri test` — the real build
system, the real per-unit cache, the real batcher and above all the real link —
stencil passes **997 of 997** native conformance tests, refused the same six
packages for the same three reasons the incumbent refused them, and left the
same blocks live at exit on every package. §9 is the list of what it does not
do.

**The benchmark's answer was a trade, and the run side closed most of the way
before the decision was taken.** stencil wins every cell whose time is compiling
— emission of a 121k-line program in 367 units is about 0.43× Cranelift's, and
it is the first backend in this repository to reach `design/PERFORMANCE.md`'s
goal 3. On the run side the four kernels are **1.38×** Cranelift
`opt_level=none`, from 1.86× before §5.1's slots-only `crt` family, and the
geomean against LLVM `-O0` is **0.927** — the paper's own bar, met here for the
first time, though Cranelift cleared it by more. What is left of the gap is one
kernel (`core/list`'s closure surface, 2.9×) rather than the boundary. The two
comparisons the trade turned on were re-taken after the removal — emission at
**0.47×**, and the run side at **1.26×** on a fresh six-kernel series, because
the four programs behind 1.38× and the `-O0` geomean have no harness in this
tree to re-run. `design/PERFORMANCE.md` §6.1 and §6.2 carry both halves.

Target coverage was the largest remaining difference at the time of the trade
and is no longer one: `macos-arm64`, `linux-arm64` and `linux-x86_64` all emit,
link and run. The fourth native triple, `macos-x86_64`, is a combination this
repository builds no library for and does not intend to — §3.2 and §10.3.

**A runaway recursion traps.** The Buri stack has a `PROT_NONE` guard above it
(§8), so a program that recurses past it faults instead of writing into whatever
the linker placed next.

## 0. The IR it consumes

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

Block-argument SSA, no phi instruction, every value defined once. The form was
chosen because it was CLIF's, back when the debug backend was Cranelift and a
transliteration was the cheapest thing to lower — and it outlived that reason
rather than depending on it. A block parameter here is a frame slot the
predecessor writes before it branches (§2), which is the same construct read a
second way, and LLVM's phis are a mechanical lowering of it (CODEGEN-LLVM.md
§2.1). Nothing in the middle IR is a compromise between two emitters, and
nothing in it names a code generator.

Pattern *bindings* are projections, not tests: `middle::decision`
(ARCHITECTURE.md §2.2) has already turned the arm list into a tree over
discriminants, so this backend never sees an arm chain, and once a block is
entered the payload fields are loads at known offsets from the enum's payload
area (VALUE-MODEL.md §6). `Profile::defensive_aborts` (`generate.rs`) decides
whether the unreachable default of a proved-total switch calls
`buri_rt_abort_unreachable` anyway; on the debug path it is on, because the belt
is cheap.

The technique is Haoran Xu and Fredrik Kjolstad's *Copy-and-Patch Compilation*
(OOPSLA 2021). Section references of the form "§4.3 of the paper" are to it;
everything else is to this repository.

The backend was called **cpjit** — copy-and-patch JIT — until it was renamed to
`stencil`, because it emits object files ahead of time and never was a JIT.
`design/PERFORMANCE.md`'s historical sections are the record of that campaign
and keep the old vocabulary; this document, the code, and CI use `stencil`.

## 1. What copy-and-patch is

A **stencil** is the machine code of one C function, compiled ahead of time,
whose literals, frame offsets and jump targets were left as *undefined symbols*.
Code generation is then two operations and no others:

```text
    memcpy(out, stencil.code)          // copy
    for hole in stencil.holes: store   // patch
```

There is no instruction selection beyond choosing the stencil key, no register
allocation beyond a three-register expression file, no scheduling, and no
peephole other than the fallthrough elision that continuation-passing makes
free. That is the whole code generator: `stencil/jit.rs::emit`, about sixty lines.

The stencils are generated and compiled **once, when the toolchain is built**
(§3). At `buri build` time the library is already bytes.

## 2. The frame-threaded calling convention

A stencil is a C function, so it can only take what C can pass. Every stencil in
this library has one of two prototypes:

```c
    uint64_t *st_x(uint64_t *fp, uint64_t r0, uint64_t r1, uint64_t r2,
                                 double g0, double g1, double g2);   // ARGS
    uint64_t *st_y(uint64_t *fp);                                    // ARGS0
```

`x0` is a pointer into the **Buri stack**, and `x1`–`x3`/`d0`–`d2` are the CPS
register file — the paper's Figure 8 pass-through parameters, which cost nothing
because AAPCS64 already has an argument at the same ordinal in the same
register. A stencil ends in `__attribute__((musttail)) return _JIT_CONT(...)`,
which compiles to a single `b`, and the copy elides even that when the
continuation is the next stencil.

Every SSA value gets a byte range in a frame. An aggregate lives **flat** in
that range at its real `middle::layout` offsets, so `MakeStruct`, `GetField`,
`GetPayload` and `GetTag` are frame-to-frame moves and nothing is boxed. A frame
is

```text
    fp + 0            return area   (the callee writes here, the caller reads)
    fp + ret_size     parameters    (the caller writes here before `bl`)
    ...               locals
    fp + frame_size   the callee's frame begins
```

so a call is: write the arguments where the callee will look for them, `bl`, and
read the return area. There is no push and no stack pointer; the callee's frame
address is `fp + frame_size`, a constant the caller knows, and it is one of the
holes in the `call` stencil.

**The width of the register file is three, and that is a measured number.** It
was swept from two to seven — seven is the ABI maximum, since `x0` is the frame
pointer and an argument past `x7` goes on the machine stack and stops the tail
call being a `b` — and the number of register assignments came out *identical at
every width*: demand saturates at two, because a stencil is only ever live-in on
an expression temporary or a loop variable. Seven registers cost 5.5× the
library, 11× the install-time compile and 15× the load, for kernels within ±4 %
of each other with no trend. `stencil/abi.rs::NREGS`.

### 2.1 The two places C has to be bridged

The convention is not the C one, so two things cross the boundary and both are
hand-written rather than emitted from stencils:

* **`main`** — `stencil/asm.rs`, two hand-encoded entry-point shims, and the
  behaviour is the one the removed backend's shims had before it:
  `buri_rt_argv_init`, the root or each `test` block behind
  `buri_rt_test_enter`, `buri_rt_flush`, and the exit convention of
  `cli/runtime/lib.rs` §6. It sets the first argument register to the Buri stack
  and calls a frame-threaded body.
* **a runtime call** — `stencil/rtcall.rs`. §5.

## 3. Where the stencils come from

`cli/build.rs` generates about twenty-three thousand C functions
(`stencil/sources.rs`), compiles them with the host `cc` in twelve parallel
shards, reads the objects back (`stencil/machobj.rs` for Mach-O, `stencil/elfobj.rs`
for ELF), extracts one stencil per exported `st_*` symbol with its relocation
records as holes (`stencil/extract.rs` for arm64, `stencil/x86.rs` for x86-64),
applies four instruction-rewriting folds where the ISA needs them, and writes
the serialized library into `OUT_DIR` for the backend to `include_bytes!`.

It does that **three times**, once per target — §3.2.

That is the paper's §5.3 "stencil library builder", and it is in the build
script for the same reason `libburi_rt.a` is: it is an **install-time** cost paid
once per toolchain build, not a cost inside the loop the rest of this design
spends its effort shortening. A second of `cc` per `buri build` would be paying
for a C compiler in order to avoid one.

Three properties, each a decision:

* **A host C compiler, not a crate.** `cc` is a platform interface in exactly
  the sense the dependency bar means, and it is not a Cargo dependency: nothing
  is added to the lockfile and nothing to `cargo install buri` beyond a tool
  every machine that can *link* a native artifact already has —
  `build/link.rs` shells out to the same one.
* **Degrades rather than breaks.** A host with no `cc` gets three **empty**
  libraries; a host whose `cc` cannot be pointed at another triple gets an empty
  one for each target it cannot reach and a full one for its own.
  `stencil::available_for` reads the emptiness and the backend reports itself
  unavailable *for that target*, exactly as `runtime_native::AVAILABLE` does for
  the archive. The probe is a three-line compile of the real prelude with the
  real flags (`sources::can_build`), not a `--version`: a Nix cross-wrapper
  answers a version quite happily and then fails on the first `#include`.
* **`Backend::identity` is the libraries' hashes, and they are baked.** Not a
  version string, because there is no version to name: the bytes depend on the
  generators *and* on whichever `cc` was on the host. Hashing the library covers
  both, so two toolchains built against different C compilers share no cached
  object.

  All three digests, not the one a given build will use: `Backend::identity`
  takes no target, so the only honest answer is the whole toolchain's stencil
  identity. It costs a conservative invalidation — rebuilding *any* target's
  library invalidates every cached object — and that is the right way round. The
  alternative, naming only the host's, would let a toolchain whose `linux-arm64`
  stencils had changed serve a cached `linux-arm64` object built from the old
  ones, which is a wrong artifact rather than a slow build.

  It is **not computed at run time**. `cli/build.rs` writes
  `stencils-<target>.bin.sha256` beside each blob and `mod.rs` `include_str!`s it.
  Hashing four megabytes cost about **22 ms of every `buri` invocation** that
  reached this backend, and memoising it — which the tree did for a while —
  removes only the *repeats*, which is the wrong half: a `buri` invocation is a
  process, so the first hash is not a repeat. It was 22 ms of a 25 ms no-op
  build, and therefore the whole of the remaining compile-side gap against
  Cranelift.

  The digest is the same string the run-time hash produced, and that is
  structural rather than hopeful: the script and `build::cache::hash_bytes` are
  one source file (`cli/src/build/sha256.rs`, which the script
  `#[path]`-includes), so there is one implementation of SHA-256 and not two
  that could drift. It was also checked directly — `buri test //... --explain`
  prints byte-identical action keys with the baked digest and with the run-time
  hash, in the same session, and every action stays *cached*. A digest that
  differed would have invalidated every cached object in every repository with
  nothing having changed.

### 3.1 The four folds, and why they are not in the paper

On x86-64, the paper's target, every hole is one contiguous field — a 32-bit
displacement or a `movabs` immediate — and patching is a store. On AArch64 no
instruction has a 32-bit immediate field, so a hole is always a **pair** of
instructions and patching is an instruction rewrite:

| hole kind | what clang emits | what the patcher does |
|---|---|---|
| `Imm32` | `adrp Xd, sym@PAGE` + `add Xd, Xd, sym@PAGEOFF` | rewrites the pair into `movz`/`movk`: any value below 2^32, no memory reference |
| `Imm64` | `adrp Xd, sym@GOTPAGE` + `ldr Xd, [Xd, sym@GOTPAGEOFF]` | aims it at the constant pool with a relocation pair (§4) |
| `Branch` | `ARM64_RELOC_BRANCH26` on `b`/`bl` | a signed 26-bit word displacement, or a relocation |

Because clang cannot be *asked* for the operand shapes the emitter wants, four
rewrites in `stencil/extract.rs` recover them: `fold_addressing` puts a frame
offset in a load or store's own `imm12`; `fold_imm` puts a literal in an
`add`/`sub`/`cmp`'s; `fold_cond` makes a conditional branch's `imm19` a hole, so
a two-way branch is two instructions instead of three; and `swap_arms` builds
the twin with the arms exchanged, so the emitter can pick whichever one falls
through. Each stencil goes into the library with up to four twins, and
`Jit::emit` picks the most specific one whose fields the operands fit.

### 3.2 Three targets, one generator

A stencil is the bytes clang emitted for a C function, so it belongs to an
**instruction set** and to a **container**, and a toolchain bakes one library
per pair. `abi::Tgt` is the three, and it is in `abi.rs` rather than anywhere
else for the same reason `NREGS` is: the build script picks a target to compile
*for* and the emitter picks a target to look a library *up* by, and a
disagreement about the spelling would be arm64 bytes inside an x86-64 object.

| target | container | reader | extractor | writer | keys | bytes |
|---|---|---|---|---|---|---|
| `macos-arm64` | Mach-O | `machobj.rs` | `extract.rs` | `object.rs` | 24,364 | 4.19 MB |
| `linux-arm64` | ELF | `elfobj.rs` | `extract.rs` | `elf.rs` | 24,384 | 4.22 MB |
| `linux-x86_64` | ELF | `elfobj.rs` | `x86.rs` | `elf.rs` | 13,874 | 2.25 MB |

`macos-x86_64` is deliberately absent: nothing this repository runs on or ships
to is x86-64 Mach-O, and `mod.rs::supported` refuses it by name. It is now the
**only** native target this toolchain has no debug backend for, which is a
sentence that had no reason to exist while a retargetable code generator was
compiled in beside this one. A macOS/x86-64 host therefore gets `Js` from
`driver::host_platform` and a refusal naming the target from `buri build` — the
same shape a host with no `cc` gets (§7, BUILD-AND-WATCH.md §2) — and no fourth
library is planned; §9 says what building one would cost.

**The two Linux libraries are cross-compiled**, on a macOS host with no Linux
sysroot, and that works for one reason: the generated C includes `<stdint.h>`
and nothing else. Clang ships its own `<stdint.h>` in its resource directory, so
`-nostdinc -isystem $(cc -print-resource-dir)/include` is a complete include
path for it. The single libc function the generators use is `memcpy`, and
`sources::memcpy_decl` declares it for the cross targets instead of reaching for
`<string.h>` — clang recognises it as a builtin from the declaration alone. The
host build still includes `<string.h>`, so the `macos-arm64` library is
unchanged by any of this.

**The `macos-arm64` library did not move.** Adding two targets touched the
generator (`prelude` takes a target now) and split the extractor (`extract.rs`
grew a container-neutral hand-over so both readers feed one arm64 finisher), and
either could have perturbed four megabytes of host stencils without anything
failing. It did not: the encoded library before and after this change is
**byte-identical apart from the `config` string**, which grew from `L12-tag r3`
to `macos-arm64 L12-tag r3` because the target is now part of what a library's
identity names. Every one of the 4,194,581 bytes after it is the same byte. The
only consequence is a one-time cache reseed, which `Backend::identity` moving is
supposed to cause.

Two more cross flags, each load-bearing: `-fno-asynchronous-unwind-tables`,
because the Linux drivers default it *on* where the Darwin one does not and the
result is a `.eh_frame` the size of the code that nothing reads; and the
`-aarch64-enable-collect-loh=false` flag is passed only to the arm64 builds,
because clang rejects it as unused elsewhere.

#### The two arm64 libraries are the same *stencils*, not the same *bytes*

`the_two_arm64_libraries_cover_the_same_operations` asserts the part that
matters: both libraries have **exactly the same 13,904 base keys**, so every
operation the emitter can ask for exists on both and no program compiles for one
and is refused for the other.

The bytes differ, in about half the stencils, and the difference is correct
rather than concerning:

* Darwin's arm64 ABI **mandates a frame record**, so a stencil that makes a call
  opens with `stp x29, x30, [sp, #-16]!` where the Linux one opens with `str
  x30, [sp, #-16]!` — `-fomit-frame-pointer` does not override a platform ABI.
* the two drivers pick different default CPUs (an Apple core against generic
  `armv8-a`) and schedule accordingly.

Both are what a native compiler for that platform *should* emit, and the Linux
ones are marginally the smaller. Fold twins differ by a few dozen keys in both
directions for the same reason — whether `+ifold` applies is a property of the
instructions clang chose — and `Jit::emit` already falls back to the unfolded key
when a twin is absent.

#### x86-64 is the paper's home ISA, and it shows

`x86.rs` is a fifth the length of `extract.rs`, because **three of the four folds
have nothing to do**:

| AArch64 fold | what x86-64 does instead |
|---|---|
| `fold_addressing` | the frame offset is a `disp32` in the load's own `ModRM` |
| `fold_imm` | an ALU op takes a full `imm32` |
| `fold_cond` | clang emits `jcc rel32` **straight to a `musttail` continuation** — `R_X86_64_PLT32` on a `0f 8x`, which arrives in the relocation record. AArch64's conditional displacement is 19 bits, clang will not risk it, and the whole of `fold_cond` exists to recover by hand what x86-64 gives away for free |
| `swap_arms` | the twin of a fold that does not exist |

So the x86-64 library has **one variant per key** where the arm64 ones have up
to six, which is most of why it is half the size.
`the_x86_64_library_has_no_folded_twins` pins that.

What it costs, stated rather than left out: `swap_arms` is genuinely lost — the
emitter cannot pick whichever arm falls through — and no measurement of that
exists, because nothing on this host can run an x86-64 instruction. §10 is what
would have to happen first.

The hole shapes are otherwise cheaper on every count:

| hole kind | what clang emits | what a patch does |
|---|---|---|
| `Imm32` | `lea rD, [rip+disp32]`, 7 bytes | rewrite in place to `mov rD, imm32` (`REX.W C7 /0`), also 7 bytes — or the 5-byte zero-extending `mov rD32, imm32` and two `nop`s where sign extension would not hold |
| `Imm64` | `mov rD, sym@GOTPCREL(%rip)` | retarget the `disp32` at the constant pool: the same single load AArch64 pays, and `movabs` does not fit in seven bytes either |
| `Branch` | `R_X86_64_PLT32` on `jmp`/`call`/`jcc` | store a signed 32-bit byte displacement — the paper's case, and never out of range |

Two details that are easy to get wrong and are not:

* **the addend is the distance to the end of the instruction**, and it is not
  always `-4`. `cmpl $0, sym@GOTPCREL(%rip)` is `83 3d <disp32> <imm8>` and
  carries `-5`; a patcher that assumed four would aim every such reference one
  byte past its target. `x86.rs` records `(instruction end, field)` per site
  rather than a constant anywhere.
* **a relocation against a symbol the object itself defines is not a hole** — it
  is a constant clang spilled into `.rodata`, and there is no value to bind to
  it. Thirty keys of 13,904 do it, all in three families where AArch64 has an
  instruction and x86-64 has a constant: `un/neg/f32` and `un/neg/f64` (`fneg`
  against an `xorps` sign mask), `cvt/u2f` (`ucvtf` against the two-bias
  `unsigned long long` → `double` sequence), and `chk/div/i128`.

  Those keys used to be **dropped**, which cost the corpus `cvt/u2f`. They are
  not any more: the referenced section's bytes travel with the stencil
  (`library::ConstRef`) and `jit.rs` copies them into the emitted unit's own
  constant pool, once per unit, aiming the reference at the copy with an
  `R_X86_64_PC32`. That is what the linker would have done with clang's
  `.rodata`, so nothing is rewritten and nothing is approximated — the
  alternative, writing the negation as an integer XOR in the generated C, would
  have changed all three libraries and been folded back into an `fneg` by
  InstCombine on the one where it mattered.

  Two things it has to get right, and both are asserted: the pool is
  **sixteen-aligned** on this target (`region::POOL_ALIGN_X86_64`), because
  `unpcklps` and `subpd` fault on a misaligned operand rather than running
  slower; and a section asking for more than sixteen is refused rather than
  under-aligned. `the_x86_64_library_covers_what_the_arm64_ones_do` is now the
  assertion — the three libraries cover the same 13,904 operations — and
  `the_spilled_constant_families_carry_their_bytes` is what says the constants
  are actually carried rather than the families having quietly stopped needing
  them.

### 3.3 A stencil is a whole function, and all three targets now say so

A stencil is one contiguous run of bytes copied out of the code section and
patched, and every hole in it names a symbol the generated C **declared**:
`_JIT_A`, a `buri_rt_*` entry point, a compiler-rt helper like `__divti3`. A
compiler that splits a stencil in two breaks both halves of that sentence at
once — the body copied out is no longer the whole function, and the branch left
behind names a symbol the library will never carry.

The G2 reference-counting arm was spelled `__builtin_expect(cap < 0, 0)`, a
1-in-2000 hint. At that ratio **Apple's** clang runs `HotColdSplitting` at
`-O2`, where upstream's does not, and it outlined the atomic arm as
`st_decref_drop.cold.1`. What each target did with that was three different
things, and only one of them was an error:

| target | what the split looked like | before |
|---|---|---|
| `linux-arm64` | a second section, `.text.unlikely.`, 72 bytes | refused — `extract_elf_arm64` allows no section but `.text` |
| `linux-x86_64` | the same section, 83 bytes, and a `PLT32` **against the section symbol** with a `+0x2c` addend | *accepted* — the reader has to allow other sections, because that is where a spilled SSE constant lives, so the cold half was read as a constant and one `jmp` was aimed at the unit's constant pool |
| `macos-arm64` | **no second section at all** — Mach-O has no section prefixes, so the outlined half landed inside `__text` and the body kept an `ARM64_RELOC_BRANCH26` to it | *accepted* — a `Branch` hole named `st_decref_drop.cold.1` shipped in the host library, and no emitter fills it |

Two guards close it, and they are deliberately not the same guard, because the
two containers hide the split in different places:

* **A hole's target must be a name the source could have written**
  (`library::refuse_hole_name`, applied by `extract.rs`'s `finish_arm64` and by
  `x86.rs`). The rule is structural rather than a list: a declared name is a C
  identifier, and every name a compiler synthesises for a piece it carved out
  contains a `.` for the precise reason that a C identifier cannot.
  `library::ARTIFACT_SEGMENTS` names the families it recognises — `cold`,
  `unlikely`, `part`, `isra`, `constprop`, `specialized`, the three `CoroSplit`
  funclets, `llvm`, `lto_priv` — so the refusal can say *what* was carved out,
  and an unrecognised one is refused all the same. `OUTLINED_FUNCTION_<n>`, the
  MachineOutliner's, is the one artifact that *is* a valid identifier and is
  named separately. The two ELF spellings the reader legitimately relocates
  against — an empty section symbol, and a `.L` label on a spilled constant —
  are explicitly not artifacts. The stencil's *own* name is checked the same
  way, which catches a compiler that resolved the branch without leaving a
  relocation behind at all.
* **On x86-64, a second section of _code_ is a spill** (`elfobj::Other::exec`).
  The row above is why the name check cannot do this job alone: the assembler
  relocated against the section symbol, whose name is empty, so there was no
  artifact name to see. This is `extract_elf_arm64`'s refusal narrowed to the
  half of the sections that could be half of a function, and it leaves the
  read-only ones — the sign masks and the conversion biases — alone.

Both were checked against the reproducer rather than argued for: with the G2
source restored to `__builtin_expect(…, 0)` and `CC` pointed at Apple clang,
`macos-arm64` fails with *"st_decref_drop: a stencil reaches
st_decref_drop.cold.1, which is a hot/cold split of it; every hole must be a
symbol the source declared"* and `linux-x86_64` with *"a stencil spilled 83
bytes of code into .text.unlikely.; every hole must be a symbol the source
declared"* — the same 72 and 83 bytes the ELF/arm64 check had been reporting
alone. `stencil::no_shipped_hole_names_a_function_the_compiler_invented` is the
standing assertion over the three baked libraries;
`the_hot_cold_split_that_shipped_is_refused` and
`every_outlining_family_is_named_rather_than_lumped_together` are the unit
checks on the rule itself.

## 4. From a JIT to a backend

The prototype this backend grew out of wrote into an `mmap`ed, `PROT_EXEC`able
region and patched absolute addresses into it, because it executed what it
emitted in its own process. A backend does not. The same emitter now writes into
a plain `Vec<u8>` whose "addresses" are section offsets, and records what it
cannot resolve as a relocation (`stencil/region.rs`).

**Nothing about the emitter had to change for that**, and the reason is the
property that made this backend possible at all: *every stencil is
position-independent*. A stencil's bytes are whatever clang emitted for a leaf C
function; the only addresses in them are the holes, and a hole is a literal, a
pc-relative branch, or a pc-relative load of the constant pool. Emitting at a
virtual base of zero and letting the linker choose the real one is the **same**
patching, and the two addresses a hole cannot know — a symbol in another object,
and the runtime's — are exactly the two the relocation format exists for.

One object per codegen unit, which is the granularity
`build::actions::codegen_units` caches at, and the granularity every backend in
this repository has emitted at. Two sections:

* `__TEXT,__text` — the code. **Every** call is an `ARM64_RELOC_BRANCH26`
  against `ir::Func::symbol`, whether or not the unit owns the callee, which is
  the name both sides already agree on.

  That the intra-unit case is a relocation too is load-bearing rather than
  uniform-for-tidiness. This writer sets `MH_SUBSECTIONS_VIA_SYMBOLS`, which
  tells `ld64` that every symbol begins an independently movable atom, and
  `build/link.rs` passes `-Wl,-dead_strip` on every macOS link. A baked
  displacement is not a *reference*, so nothing reaches the callee's atom, so
  the linker moves it and then deletes it and the `bl` lands on whatever took
  its place. Resolving those branches at emit time — which is what the
  in-process prototype did, because it had no linker — failed **977 of 997**
  native conformance tests through `buri test` while passing every test that
  linked with a bare `cc`. The relocated form costs nothing: the linker
  resolves an intra-section `BRANCH26` to the same instruction, and emission
  measured within 0.5% either way.
* `__DATA_CONST,__const` — the constant pool: string literals, abort messages,
  and every `Imm64` datum. It is a **separate section** and not a tail on
  `__text` because a pool slot may hold an address, and `ld` refuses an absolute
  relocation inside a code section outright ("Absolute addressing not allowed in
  arm64 code").

That in turn means the `adrp`/`ldr` pair reaching the pool crosses a section
boundary, whose distance is the linker's choice — so the pair is **not patched
at all**. It is left exactly as clang emitted it, both immediate fields zero,
with an `ARM64_RELOC_PAGE21`/`ARM64_RELOC_PAGEOFF12` pair naming the slot. Which
is what clang's GOT form was before the prototype retargeted it: the port comes
back to the relocation it started from, once there is a linker to honour it.

The unit that owns `main` also carries a zero-filled `__DATA,__bss` section for
the Buri stack (§8).

`stencil/object.rs` is the Mach-O writer and `stencil/elf.rs` is the ELF one;
§3.2's target table says which reader, extractor and writer each of the three
targets uses. The paragraph that used to stand here said there was no ELF writer
and that `Platform::Linux` was refused because of it. Both halves stopped being
true when the Linux targets landed: `elf.rs` writes objects that `ld.lld`
statically links with every relocation resolving, and what is still refused is
the *link* on a foreign host rather than the emission (ARCHITECTURE.md §9).

### 4.1 Reproducibility of the object bytes

Two things are checked rather than assumed, because ARCHITECTURE.md §7 compares
these bytes:

- **Symbol and section order is a function of declaration order**, and
  declaration order is the middle end's function order, which is source order
  (`monomorphize.rs`). Nothing in the emission path iterates a `HashMap`.
- **No timestamps.** Neither writer emits one for a relocatable object. The
  archive step (§12) is where a timestamp could enter, and it is zeroed there.

`a_cross_emission_is_reproducible` (§10.1) is that stated as a test for the two
Linux targets, and `--check-reproducible` (ARCHITECTURE.md §7) is it for the
host one. Neither half of this subsection is this backend's alone: both were
written for the removed one and are true of both writers, which is why §13 lists
them among what stayed.

## 5. The runtime boundary

Every operation `middle::lower` leaves as a `Body::Runtime` or an
`Inst::CallIntrinsic` with a `buri_rt_*` symbol is **a call into
`libburi_rt.a`** — the same archive, the same contract
(`cli/runtime/lib.rs`) and the same table shape as the other two backends.
`stencil/runtime.rs` is one transcription of that contract and `llvm/runtime.rs`
is the other, key for key and shape for shape, and
`cli/tests/native/conformance.rs`'s companion test is what keeps the two from
disagreeing about which keys exist.

This is the wave's largest single deletion. The prototype had its own
`intrin.rs`: a descriptor-driven helper per operation, written in Rust, living
in the compiler's process. That could not survive object emission — a symbol in
the compiler is not a symbol in the artifact — and it was, in the honest naming,
`libburi_rt.a` written a second time, with every `num.U64.checkedMul` the
language ever adds having to be written twice.

### 5.0 A runtime call is emitted into its caller, not called

The same key reaches this backend two ways — spelled inline it is an
`Inst::CallIntrinsic`, spelled as a method it is an `Inst::Call` to a
`Body::Runtime` function — and **both are emitted at the call site**, which was
the first act of the removed backend's `call` for the same reason.

Making the second a real call cost a whole frame for nothing. The caller copied
the operands into the callee's parameter slots and branched; the generated body
then copied the same words again into its C argument area and branched into the
archive. The second copy is the *same* marshalling from a different address, so
the frame bought nothing: on a matrix multiply whose inner loop is two
`a.get(i)` per element, the two frames were about forty instructions where
Cranelift reaches the entry in ten, thirty-two million times, and emitting at
the call site took the four-kernel total down 11% and that kernel down 17%.

It is sound because the *shape* of a marshalled call is a function of the key
and of the operand and result IR types alone, and those are the same at the two
sites: a `Body::Runtime` function's signature **is** its caller's argument and
destination types. `rtcall.rs` is one implementation of `cli/runtime/lib.rs`
§2's rule and both sites hand it the same list, so a shape refused at one is
refused at the other, with the same sentence.

Two keys are deliberately still called: `core/list`'s closure surface and the
two `deriveArray*` derives, which `lists.rs` open-codes as a **loop** whose step
this function has to be able to see as a `MakeClosure`. Where it cannot, calling
the `Body::Runtime` function is the designed fallback — its body reaches the
same loop through the closure's thunk — so inlining those would replace a
working answer with a refusal.

**That exclusion is now the largest single gap this backend has on the run
side.** With §5.1's slots family landed, three of the four kernels are within
1.1×–1.2× of Cranelift and the fourth — the `core/list` closure pipeline — is
**2.9×**, unmoved by everything this boundary has been given because it never
went through it. Whatever is next for run time is in `lists.rs` and the thunk,
not in `rtcall.rs`.

### 5.0.1 `str.concat`, the one call with no table row

The other two backends open-code MEMORY.md §5.3's three concatenation paths —
in place when the left operand's block is uniquely owned and has room, grown
when it is unique and out of room, exact otherwise. This backend cannot: a
header load, two compares, three arms and a `memmove` are a dozen stencils and
a block layout against one `crt` stencil for a call. It therefore emitted the
*exact* path alone and always allocated, and that was a divergence rather than
a missing optimisation, because `core/alloc`'s `count` and `total` are numbers
a Buri program can read.

So the three paths are `cli/runtime/text.rs`'s `buri_rt_str_concat` and this
backend calls them, which is the shape MEMORY.md §5.3 already gives `[T]`
append. A thousand concatenations onto a uniquely-owned string now allocate the
same thirteen blocks a release build allocates, against a thousand and one
before.

The row is deliberately not in `runtime_table.rs`: the two length words go
**unmasked**, because VALUE-MODEL.md §3.1's ASCII flag is an input to a
concatenation rather than a tag to be stripped, and the flattening a table row
drives masks every `Str` length. `rtcall.rs`'s `str_concat` is the one caller
and is where that fact lives.

### 5.1 How the arguments get there

`cli/runtime/lib.rs` §2's rule is: the flattened Buri arguments, then the
element pair, then the out-pointer. What differs here is only *where* an
argument is. Cranelift builds a value list and lets its register allocator place
it; this backend has no register allocator at the call boundary, so where an
argument is has to be spelled in the stencil, and the shape is
`(integers, floats, result)` — 132 shapes per family, ten integers rather than eight
because the ninth and tenth go on the **machine** stack and that is entirely
clang's business (the stencil is the zero-register prototype, so nothing of this
backend's is live across the call). `buri_rt_str_replace` is the entry that
needs them, being three `Str`s flattened and an out-pointer. Integers and floats
are counted separately, because AAPCS64 assigns the two register banks
independently: a double in argument position three still goes in `d0` if it is
the first float.

There are **two families of that shape**, and `rtcall.rs::c_call_to` picks per
call site:

* **`crts`, the slots family.** One frame-offset hole per argument, which
  `extract.rs::fold_addressing` puts in the `imm12` field of the load that uses
  it. An argument that is already a whole frame word therefore costs **one
  instruction and no store at all**.
* **`crt`, the array family.** The arguments copied into a contiguous scratch
  area with the ordinary `mov` and `imm` stencils, and one stencil reading them
  off consecutively with `ldp`s. A folded `imm12` reaches 32 KiB into a frame,
  and a frame wider than that is what this is still for.

**The cross product this design rejects is over operand *kinds*** — the paper's
§5.1 axis, register / slot / immediate — and neither family is that one. Every
argument of both is a slot, so both are the same 132 shapes; what the slots
family has more of is *holes per stencil*, not stencils. An operand that is not
already a frame word — a literal, a narrow field, an address, a glue symbol — is
materialised into the scratch area first in either family and read from there,
so the two differ only in what they do with the operands that were already in
the frame.

**What it bought, measured.** On `dot`'s inner loop, one `a.get(i)` — six
integer arguments, three of them frame words:

| | array family | slots family |
|---|---:|---:|
| staging the six operands | 12 | 6 |
| reading them into `x0`–`x5` | 4 (`add` + three `ldp`) | 6 (`ldr` each) |
| prologue, `bl`, result, epilogue | 9 | 9 |
| clang's clears after the call (§5.1.1) | 5 → 0 | 5 → 0 |
| **total** | **30 → 25** | **26 → 21** |

Four instructions — and **the four instructions are not where the time went**.
The same kernel went **701.3 ms to 302.0 ms, a 57% cut**, which four
instructions in a twenty-six-instruction sequence cannot explain. What the array
family really cost was a *store-to-load round trip*: six `str`s into the scratch
area immediately followed by three `ldp`s reading the same addresses back, a
dependent chain through memory in the middle of the hottest loop in the program.
The slots family reads each argument from where it already was, so the chain is
gone. **An instruction count is the wrong unit for this boundary, and this table
is here to say so rather than to be believed.**

Against the incumbent the four kernels went **1.86× → 1.38×** of Cranelift
`opt_level=none` — past the 1.49× the pre-parity prototype reached — and the
geomean against LLVM `-O0`, the bar the paper claims, went **1.190 → 0.927**,
which is the first time any measurement in this repository has cleared it. The
cell a developer actually waits on moves with it: `buri test //suite/heavy`
incremental at a hundred thousand lines is **1.56× → 1.26×**, and the same
1.26× on three corpora nothing was tuned on.

**What is left is a C function's own frame.** Of the twenty-one, six stage the
operands that were not already frame words — a literal, a null, an out-pointer —
six are the argument loads that are the point, and nine are the `crt` stencil's
C-ABI prologue, `bl` and epilogue. An emitter can remove none of them: a stencil
is a C function, and a C function that calls has a frame.

#### 5.1.1 The clears clang leaves after a call

A zero-register stencil ends in `musttail return _JIT_CONT0(fp)`, and clang
clears the argument registers it used — `x1`–`x7` — before the `b`, exactly as
it clears `x8`–`x17`: they are not parameters of the callee and it has proved
them dead. That was five instructions after every runtime call and every Buri
call, and `extract.rs::strip_dead_clears` now drops them.

They are droppable for a reason this backend *states* rather than inherits:
`jit.rs::is_barrier` already treats every zero-register stencil as clobbering
the whole CPS register file, so nothing downstream may read `x1`–`x7` across
one. The rule is therefore keyed on the tail hole's name — `_JIT_CONT0` and not
`_JIT_CONT` — because at the register-passing prototype `x1`–`x3` *are* the
continuation's `r0`–`r2` and a `movz x1, #0` there can be a value the next
stencil reads.

**What it bought was code and not time.** `dot`'s body went 130 instructions to
120; the four kernels moved by 0.7%, which is inside this machine's spread. It
is kept because five dead instructions inside every call stencil are five the
artifact should not carry, and the artifact being 52% larger than Cranelift's is
a cell of its own — not because it made anything faster.

The callee is a hole that is **called** rather than materialised, so it becomes
one `bl` and one `ARM64_RELOC_BRANCH26` instead of a pooled pointer and an
indirect call. There is one `_JIT_RT_*` symbol declared per shape rather than
one for all of them, because C has one type per name and the declared type is
what decides which registers clang reads the arguments out of.

### 5.2 Two traps worth writing down

**Going out.** A frame slot holds every integer **zero-extended**, whatever its
type (`sources.rs::write`'s convention: "a frame slot is never partially
defined"), and the typed stencils reinterpret the low bytes. So an `I8` of `-3`
is `0xfd` in its slot — and handing that word to a C parameter declared
`int64_t` renders `253`. Every narrow *signed* value crossing to the runtime is
widened first (`rtcall::int_bits`). This is a class of bug, not an instance: it
is invisible in the emitted stencil, invisible in the IR, and shows up as an
unsigned number in a rendered string.

**Coming back**, and this one was found by the x86-64 port rather than by
reading. A `crt` stencil **declares** the entry it calls, and the declared
return type has to be the one the entry actually returns: both psABIs leave the
upper bits of an integer return narrower than a register **unspecified**.
`buri_rt_str_eq` answers a `u8`, `buri_rt_char_to_upper` a `u32`, and a fallible
entry's discriminant a C `int` — three widths, and a stencil declaring
`uint64_t` for the first two reads whatever was in the register above the byte
that mattered.

AAPCS64 hid it completely: Rust's arm64 codegen zeroes the register on the way
out, so every arm64 run agreed with every other backend. SysV does not, and the
symptom was `assert.isFalse` failing on five conformance files and `p == q`
answering `true` for two strings that differ — a `Bool` read out of the garbage
above `al`. `sources.rs::RETURN_SHAPES` now has a shape per C return width and
`rtcall::scalar_kind` picks it from the destination's own IR type, which is the
same fact Cranelift builds its call signature from. The cast is inside the
stencil, so it costs the `movzx` the psABI already required of the caller and
nothing more.

### 5.3 `I128`, which is where a backend can fall short of the type system

The model says 128-bit arithmetic is exact (VALUE-MODEL.md §1) and a backend
that cannot do it in registers calls the runtime. Where an operation on `I128`
is not natively lowered on the target, the middle end legalizes it into a pair
of `I64`s with explicit carry — a legalization pass in `middle`, so the LLVM
backend gets the same rewrite if it ever needs it and both backends produce the
same answers. Division and remainder always go to the runtime
(`buri_rt_i128_divmod`), on both, because that is a hundred instructions nobody
should inline. Its operands cross as **pairs of `I64`s, low half first**, rather
than as `I128`: the contract says a parameter is a scalar leaf, and passing a
pair means neither backend has to agree with the platform ABI about how a
128-bit integer is classified.

This subsection is not this backend's either — it is VALUE-MODEL.md §1's
fallback, stated in a code generator's document rather than in the model's
because it is a backend limitation and not a model decision, and it moved here
when the document it was written in was removed (§13).

### 5.4 Aborts

`buri_rt_abort(msg_ptr, msg_len)` never returns, and neither does any of the
fixed messages beside it — `buri_rt_abort_div_zero`, `buri_rt_abort_shift`,
`buri_rt_abort_bounds`, `buri_rt_abort_unreachable`. They exist so that a
message pinned by `cli/tests/crash/` lives in the runtime rather than in a
backend's string table, which is why they outlived the backend they were first
written for, and they are reached through §5's boundary like every other runtime
call.

## 6. Reference counting, and the functions a unit generates for itself

MEMORY.md §5.1's saturating increment and its decrement, open-coded as two
stencils, with the dying arm calling the type's drop glue and then
`buri_rt_free` — instruction for instruction what MEMORY.md §5.1 spells and
what `llvm/emit.rs::decref_pointer` emits, because two backends deciding
separately when a block dies is the one divergence MEMORY.md §5 cannot
tolerate. `buri_rt_free` is the sole owner of the free and of the live-block
counters on both.

`emit::Lower::walk_rc` covers all five site kinds: a
`Str`/`[T]` block, a nested aggregate, a tagged enum's per-variant payloads, a
**boxed** field, and a **niche** whose payload is walked behind its null test.

The last of those is not belt-and-braces. `.None` is written by storing null at
the one pointer the discriminant is and nothing else, so every other byte of the
payload area is whatever the frame last held; walking it unguarded decrements a
count at an address that was never a pointer. The guarded site is a shape both
native backends have, for the same reason.

### 6.1 `glue.rs`

Four things a unit generates for itself — the set the removed backend's
`helpers.rs` generated, under its argument, which outlived it because the set is
a property of the value model rather than of an emitter — and every one a
**local** symbol so that two units needing the same one do not collide:

| Helper | Why it is generated rather than called |
|---|---|
| `Thunk` | A closure's `code` takes its environment as a *pointer*; a lifted lambda takes it as an aggregate parameter, flat in its frame. Something has to convert. |
| `Walk` | The per-type counted-pointer walk as a C `fn(*mut u8)`: the drop glue `buri_rt_decref` calls, and the per-element retain `cli/runtime/list.rs` is handed. |
| `Elems` | The same over a whole `[T]` block, whose element count is `cap / stride`. |
| `EnvGlue` | The one indirection that lets a closure environment carry its own release function: `Ty::Fn` does not record what was captured. |

A thunk is entered by the `calli` stencil and is an ordinary frame-threaded
body. A glue function is entered by the **runtime**, so it is `extern "C"` and
each one is a hand-written eight-instruction stub in front of a frame-threaded
body. The stub's whole job is to make a frame, and it takes the *machine* stack
for it: drop glue recurses — a `[[Str]]` releases a `[Str]` releases a `Str` —
and a fixed scratch frame would be re-entered by its own callee.

The walk reads the value out of a **copy** in that frame rather than through the
pointer, which is what lets one implementation of `walk_rc` — addressing
everything as a frame offset — serve both an `Inst::DecRef` and a glue function.

### 6.2 The threshold, which has to apply at every level

A type graph is a DAG whose nodes are revisited along every path, so an inline
walk of a record of records of records expands once per *path* rather than once
per type. Past `RC_INLINE` levels a compound field's walk goes through that
type's own `Walk` glue instead, so an emitted body holds a bounded number of
levels plus one call per deeper field and the code is linear in the distinct
types a program holds. `RC_INLINE` is the threshold both native backends apply,
and `conformance/lib/semantics/test/generics.buri` is the file that needs it.

### 6.3 The closure environment is a block

`MakeClosure` allocates `[release fn][record]` and puts the pointer in the
closure's `env` word, which is the shape VALUE-MODEL.md §7.1 pins for both
native backends, and `walk_rc` counts that word. Carrying the environment by value in one word — what
wave 1 did — cannot hold a `Str` and cannot be released, and both show up as
refusals rather than as a smaller closure.

## 7. What a refusal is

**A diagnostic naming the shape, never an artifact that aborts when it reaches
it.** The prototype emitted an `unsupported` stencil and skipped the tests that
reached one, because it was measuring throughput on the part it could compile. A
backend cannot: `compile_unit` finishes the emission — so that one build reports
*every* refusal rather than the first — and then produces no object at all,
with one error per distinct shape.

`Backend::missing_intrinsics` is the cheaper, earlier form of the same answer,
and the two are different questions: the hook says "this backend has no body for
that key", and a refusal says "this backend has a body but not for that shape".

## 8. The Buri stack, and its guard

Generated code makes no use of the machine stack, so the Buri stack is a
zero-filled block the unit owning `main` emits, and `main` passes its address as
the first frame pointer. It costs nothing in the object or on disk — a zero-fill
section has a size and no bytes — and nothing in the artifact until it is
touched. The block is **65 MiB**: 64 MiB a program may use and a 1 MiB guard
above it.

### 8.1 The guard, and which stack it is on

`main` — both shims — turns the top megabyte into `PROT_NONE` with one
`mprotect` before it calls anything of the program's, which is nine instructions
once per process (`asm.rs::install_guard`). A runaway recursion then **faults**
where it used to keep writing.

Three decisions, each with a reason that is not "it seemed safer":

* **Above, not below.** A callee's frame is `fp + frame_size(caller)`, so this
  stack grows *upward*: the address a runaway reaches first is the top of the
  block. That is the opposite side from where the kernel puts a thread stack's
  guard, and getting it wrong would be a guard nothing ever touches.
* **A megabyte, not a page.** A guard narrower than the widest frame can be
  *stepped over* — a callee whose locals area exceeds it writes past it without
  touching it. This is the hazard native code answers with stack probes, and
  neither the removed backend nor LLVM at `-O0` enables them, so a machine frame
  past the OS guard has the same exposure everywhere. Nothing here is less sound
  than what it replaced; a megabyte is far past any frame `middle::layout`
  produces, and zero-fill pages that are never faulted in cost address space and
  nothing else.
* **One block and one symbol.** `MH_SUBSECTIONS_VIA_SYMBOLS` makes every symbol
  the start of an independently movable atom, so a second symbol at the guard's
  address would let `ld64` place the guard somewhere other than immediately
  above the stack — which is the one property the mechanism rests on. The guard
  is therefore addressed as the stack's symbol plus a constant, and the block is
  aligned to 16 KiB so that the constant lands on a page.

**Two stacks, and only one of them needed this.** A stencil artifact uses the
machine stack in exactly two places, and both are already guarded by the
kernel: a `crt` stencil's own prologue (§5.1), and `glue.rs`'s `extern "C"`
stubs, which take a machine frame precisely because drop glue recurses (§6.1).
There is no in-process JIT mode to guard separately — this backend writes
objects and the linker makes the artifact (§4) — so `main` is the only place a
stack is established at all, and `install_guard` is in both of `main`'s forms.

### 8.1.1 A second carrier, and its own block

Everything above is about *one* stack, because until slice B7 there was one:
`main` establishes it and `main` is the only way into Buri code. A **carrier**
— an OS thread from `cli/runtime/rt.rs`'s pool — is a second way in, and it
cannot share the block. Two carriers on one upward-growing stack write their
frames into each other, and the guard at the top belongs to whichever recursion
reached it first.

So a carrier asks for its own. `buri_rt_stack_acquire` (`cli/runtime/memory.rs`)
`mmap`s 65 MiB — **the same 64 + 1 as above**, in a constant each side names and
each side's tests assert — and `mprotect`s the top megabyte to `PROT_NONE`, for
all three of §8.1's reasons and by the same arithmetic. `buri_rt_stack_release`
puts the block on that carrier's free list rather than unmapping it, because a
carrier is reused and address space is what this whole mechanism spends.
Nesting gets a *different* block, so an entry inside an entry does not write
over the outer entry's live locals.

The door itself is `asm::carrier_entry`: `void(void *state, void *out)` at the
platform ABI, `backend/carrier.rs`'s signature, emitted in the unit that owns
the root next to `main`. It is `program_entry` with one line changed — the
frame pointer comes from the runtime instead of from `adrp buri$stencil$stack`
— and everything else about it, including the return area copied through
`out`, follows from that. `-dead_strip` deletes it from a program that never
references it, so a single-threaded artifact pays nothing: no door, no `mmap`,
no page.

`main` keeps the `__bss` block. That is deliberate and is what keeps every
number in §8.2 true of the program a user actually runs.

### 8.2 What a program does when it runs out

The same thing a Cranelift-compiled one did, which is what parity meant while
there was something to be at parity with: the process dies on the fault, with no
message, and the shell reports the signal. Measured on the same non-tail
recursion, both backends through `buri build` before the removal: Cranelift
**exited 139** (`SIGSEGV`, the OS guard under the machine stack) and stencil
**exits 138** (`SIGBUS`, the `PROT_NONE` guard above the Buri stack). SPEC §6.9
asks for "a message on stderr and a non-zero exit status" and neither printed the
message; that gap is the *runtime's* — it has no fault handler — and it is not
this backend's to close alone. What this section closed is the difference that
was stencil's own: **a deep recursion used to corrupt whatever the linker placed
after the stack and keep running.**

**Where the fault lands is the whole of the change, and it was measured rather
than assumed.** A `SIGBUS` handler injected into the artifact prints `si_addr`
beside the runtime address of `buri$stencil$stack`:

```text
before:  addr=0x1065e42c8  stack=0x1025e0d60  delta = 64.013 MiB
after:   addr=0x108f282c8  stack=0x104f28000  delta = 64.001 MiB
```

Before, the program wrote **13,656 bytes past the end of its own block** and
faulted only when it reached a page nothing had mapped — and `size -m` on that
binary shows `__bss` is 64 MiB *plus forty bytes*, so there was other zero-fill
data in the neighbourhood for a wider frame or a longer-lived program to land
on. After, the first byte past the usable stack is unmapped.

Two tests in `cli/tests/native/stencil.rs` hold it, and they are the two halves:
`a_runaway_recursion_faults_at_the_guard` links a non-tail recursion with the
product's own flags and asserts the process is **killed by a signal**, and
`a_deep_recursion_inside_the_stack_still_answers` asserts that a recursion that
fits is unaffected, which is what says the guard is *above* the usable stack
rather than carved out of it.

## 9. What is not here, and who else is not

Named rather than left to be discovered. Every one is a refusal, so a program
that needs one is told; none is a wrong answer.

**Refused by every backend**, and not a stencil gap. `native/conformance.rs`'s
`PACKAGES` records the reason per package; the removed backend refused the same
six conformance files for the same three reasons, which is one of the things
parity meant. They are:

* an **inexact** numeric conversion. `x.toT()` where not every value fits
  answers `Result<T, RangeError>` (SPEC 6.2.1), and `RangeError` is a struct of
  two `Str`s the backend would have to build. This subsumes float→integer
  entirely: no float-to-integer conversion is exact, so every one of them is
  this shape.
* **`json.*`, and `ToJson::toJson` called directly on a primitive** —
  `json.decode` is a descriptor-driven walker, and the five `bool.toJson` /
  `char.toJson` / `str.toJson` / `num.<T>.toJson` keys are the trait's own
  leaves. `derivePrimJson` was on this line and is not any more: both backends
  build `Json`'s primitive arm now (VALUE-MODEL.md §12 row 10), and the five
  keys above are the same answer reached through the trait rather than through
  the derive.
* **`core/math`'s thirteen transcendentals**, which are refused rather than
  unwritten; `cli/runtime/math.rs` argues it.

**stencil's own, and each is a sentence rather than a wrong answer:**

* **macOS on x86-64.** No stencil library is built for it, and none is
  intended: a stencil is the bytes clang emitted for a C function, so that
  combination needs x86-64 instructions in a Mach-O, and nothing this repository
  runs on or ships to is that. It is the one native triple with no debug backend
  at all. `mod.rs::supported` refuses it by name,
  `an_unsupported_cross_target_is_refused_with_a_reason` holds the sentence, and
  `RelKind::r_type` answers `None` for the two x86-64 kinds precisely so a
  Mach-O object can never carry one. The other three targets all emit, link and
  run (§3.2, §10.3).
* **Linux execution from this host.** Both Linux targets emit objects that a
  real linker accepts and fully resolves, and that is as far as *this* machine
  can go — §10.1 says why. CI runs the programs on both (§10.2, §10.3).
* **Debug information** — neither DWARF nor `.buri_symbols`. It was the gap the
  removed backend recorded for itself too, so nothing regressed with the flip;
  §11 is what closing it would start from.
* **An element wider than the staging room a frame keeps** (`lists.rs::STAGE`).
  A `zip`, a `flatten` and a `sortBy` move whole elements between two blocks
  through the frame, and the frame's scratch is a constant; past it the shape is
  refused with the two numbers in it.
* **A `Float` hashed or shown at `F32`** through the runtime boundary: a `crt`
  stencil declares every float parameter `double`, and an `F32` sits in its slot
  as its own thirty-two bits, so the shape `buri_rt_show_f32` wants is one this
  call boundary does not have.

**Not the backend's at all.** Whatever a conformance package leaves live at
exit is `middle::rc`'s plan rather than an emitter's, and that was the last
thing checked before the flip: **both backends left exactly the same blocks and
the same bytes** — measured through the runtime's own `buri_rt_heap_stats`, on
the objects `buri test` produced, re-linked with `build/link.rs`'s own flags.
That reading was 0 live on all nine measurable packages, where earlier rounds
recorded three files leaking (17, 5 and 20 blocks); the count moves with
`middle::rc`, and the *parity* is what the flip rested on (§13). What emits a release
is `middle::rc`'s plan, which both consume; a backend cannot release what it was
not asked to.

## 10. What is verified, what is not, and by whom

This section exists because the two Linux targets were built on a machine that
cannot run either of them, and the boundary between "checked" and "believed" has
to be written down rather than inferred from the absence of a failing test.

### 10.1 What this host could and could not do

The port was written on macOS/arm64. Three things make Linux **execution**
unreachable from here, and none of them is a missing effort:

* `link::can_link` is `host_platform() == target.platform && …`, and
  `actions::native_ready` gates on it. Cross **codegen** is supported and cross
  **linking** is deliberately refused.
* `runtime_native::ARCHIVE` is one archive for one triple — `cli/build.rs`
  builds `libburi_rt.a` with `--target <host>` — so there is no Linux
  `libburi_rt.a` to link against even if the linker were willing.
* `rustc` on this machine has no Linux standard library installed —
  `*-unknown-linux-gnu` when this was written, `*-unknown-linux-musl` since the
  static switch — so one cannot be produced here either; and there is no `qemu`, no
  `docker`, and no initialised `podman` machine.

That is exactly where `cli/benches/compiler.rs`'s `lower+linux-*` rows already
stopped when Cranelift took them: they lower and emit object bytes, and nothing
is linked and nothing is run in any native row.

**What was checked here**, and it is more than nothing:

| claim | how |
|---|---|
| the two Linux libraries build, cross, from this host's clang | `cli/build.rs`, three blobs in `OUT_DIR` |
| the arm64 libraries cover the same 13,904 operations | `the_two_arm64_libraries_cover_the_same_operations` |
| the three libraries cover the same operations, x86-64 included | `the_x86_64_library_covers_what_the_arm64_ones_do`, `the_spilled_constant_families_carry_their_bytes` |
| no baked library holds a hole naming a function the compiler carved out of a stencil, and the rule that refuses one is right about every name a hole is really bound to | `no_shipped_hole_names_a_function_the_compiler_invented`, `the_hot_cold_split_that_shipped_is_refused`, `every_outlining_family_is_named_rather_than_lumped_together`, `the_names_holes_are_really_bound_to_all_pass` (§3.3) |
| `elf.rs`'s output reads back through an independent ELF reader | `elf::tests::what_is_written_is_what_the_reader_reads` |
| `b`/`bl` and `add`/`ldr` get the right *split* relocation type | `a_branch_is_split_by_its_instruction`, `a_low_twelve_is_split_by_its_instruction` |
| `ld.lld` **accepts** real unit objects for **both** Linux targets, statically links them, and **every relocation resolves** with none left in the image | `linux_arm64_objects_link_and_every_relocation_resolves`, `linux_x86_64_objects_link_and_every_relocation_resolves` |
| each linked image still disassembles as its own machine and has a `main` | same two tests |
| neither machine's relocation kinds can be spelled for the other | `a_kind_the_machine_does_not_have_is_refused_by_name`, `an_x86_64_kind_has_no_mach_o_spelling` |
| two emissions are the same bytes, for both Linux targets | `a_cross_emission_is_reproducible` |
| the one target with no library is refused by name, and the two that have one are not | `an_unsupported_cross_target_is_refused_with_a_reason` |
| macOS is unregressed | the existing 997-file conformance corpus, unchanged |

The link in that suite uses a **generated stub** in place of `libburi_rt.a`,
derived from whatever the objects themselves leave undefined. That is honest
about what it proves: the *shape* of every reference, and nothing about what the
referent does.

### 10.2 What a Linux run had to confirm, and where it is confirmed

Both columns are discharged. `.github/workflows/ci.yml` is where it stopped
being prose: the suite runs on `macos-latest`, `ubuntu-24.04` and
`ubuntu-24.04-arm`, and the two Linux jobs run the artifacts rather than
only compiling them — the stack guard's `mprotect` and Linux's signal
disposition for a `PROT_NONE` page, the corpus at macOS parity, leak parity
through `buri_rt_heap_stats`, `--check-reproducible` on a linked Linux artifact,
and both linkers' idea of an ELF image. Three scripts hold the parts a green
exit would otherwise hide: `assert-stencils.sh` reads the blob sizes
`available_for` reads (and asserts the reverse degrade — on a Linux host the
`macos-arm64` library must be empty), `assert-suite-ran.sh` requires the corpus
census's own output, because every test in `stencil.rs` opens with
`if !supported() { return; }` and a runner with no stencils would otherwise pass
the suite having run nothing, and `assert-elf.sh` checks the linked image for a
defined `buri$stencil$stack`, a `.bss` still `NOBITS`, and a `PT_GNU_STACK`
without `E`. The workflow's own comments carry the reasoning; this document does
not keep a second copy of them.

Two things there remain **uncovered**, and neither is a step that could be
renamed into existence:

* **Leak parity as §9 states it.** What runs is heap-stats accounting on both
  suites on one box. "Both backends leave *exactly the same* blocks" is not
  something a leak checker's own accounting can state; the nine-package
  comparison in §9 was made by hand and has no harness here.
* **The run-side kernels.** `cli/benches/compiler.rs` measures compile phases,
  and the four kernels behind §1's 1.38× have no harness in this repository —
  nor is stencil a selectable emitter for the benchmark's `lower+*` rows. Both
  are repo-side hooks.

Two decisions inside those jobs are worth naming because they are load-bearing
rather than tidy. They use **apt and not the flake**, because the question is
whether *the CI image's* clang can produce objects and a `nix develop` would
answer for nixpkgs' clang instead — the flake is held green by the same file's
`nix` job. And they export **`CC=clang`**: `sources::compile_flags` passes
`--target=` and `-print-resource-dir` for both Linux targets including the
host's own, gcc understands neither, and `cli/build.rs` degrades to an empty
library rather than failing — so a run with gcc as `cc` is a green run that
checked nothing.

### 10.3 The x86-64 emitter, and the six pieces it was built from

**x86_64-unknown-linux-gnu emits, links and runs.** It needed everything the
aarch64 column needed and, before any of it could be asked, six pieces. Each is
listed here as it was written, because the shape of the answer is the argument
for it. It was written because the removal of the retargetable backend (§13)
would otherwise have left this target with no debug backend at all, which is the
one thing the flip could not be taken with outstanding.

**The landed state.** Five of the six are in; item 6 (`swap_arms`) is open and
is a measurement rather than a correctness gap. `asm::AVAILABLE_X86_64` reads
**`true`**, so `stencil::AVAILABLE`'s `linux-x86_64` disjunct holds and an
x86-64 Linux host runs the suite rather than skipping it. CI's `linux-x86_64`
job is the twin of the `linux-arm64` one and asserts, instead of a skip: the
corpus census at macOS parity (**26 of 36**, the same 26 by name, checked as a
set rather than a count), the whole `stencil::` suite running real programs, the
stack guard's `mprotect`, leak parity through `buri_rt_heap_stats`,
`--check-reproducible` on a linked `linux/x86_64` artifact, both linkers'
images, and a cross emission back to `linux-arm64`. `cargo test -p buri
--no-fail-fast` is green on `ubuntu-24.04`. §10.2's last paragraph and §3.2's
table carry the same fact.

The six:

1. **`asm.rs`: a SysV entry point.** `program_entry`, `test_entry` and
   `install_guard`, hand-encoded, with `rdi` as the frame pointer. It is the
   A64 shim instruction for instruction and decision for decision — the same
   `buri_rt_argv_init` first, the same guard, the same `cbz`/`cbnz` sense on the
   niche test — and only two things differ, both forced by the machine. `rsp`
   must be sixteen-aligned at every `call` and `main` is entered with the return
   address already pushed, so one `push rbp` opens the shim and nothing below it
   moves `rsp` again; and an address is one instruction, so `adrp`/`add` against
   the stack symbol becomes `lea rD, [rip+sym]` and one `R_X86_64_PC32`, and
   `bl` becomes `call rel32` and one `R_X86_64_PLT32`.

   One thing is *not* symmetric and is the only place the two shims read
   differently: an emitted function answers the frame pointer it was handed in
   **`rax`**, not in the register it arrived in, because every stencil is
   `uint64_t *st_…(uint64_t *fp, …) { … return fp; }` and that is where a C
   return value lives here. So the return area is read off `rax`.

   Every byte sequence in `asm.rs`'s x86-64 tests was disassembled with
   `llvm-mc -triple=x86_64-unknown-linux-gnu` and read back against what it is
   named for; the tests carry that listing as their comments.

2. **`jit.rs`: the patcher.** Four small functions, and the shapes were already
   decided by §3.2's table. `patch_rel32` serves a branch, a call and a `jcc`
   alike — the same arithmetic, unlike A64 where they are different fields — and
   is what `patch_branch` and `patch_cond` become on this target, with no veneer
   because a 32-bit displacement always reaches. `patch_pc32_imm` is the
   `lea` → `mov` rewrite for `Imm32`, in the seven bytes the `lea` occupied.
   `patch_imm64_x86_64` is the pool retarget for `Imm64`, which uses the
   **per-site instruction end** recorded in `Hole::pairs` and not a constant
   four, and which takes the same fits-in-32-bits relaxation A64 takes — guarded
   on the instruction actually being a plain `mov rD, [rip+disp32]`, since a
   float immediate arrives as `movsd` and a small comparison as `cmpl $0, …` and
   neither may become a `mov` of a literal.

   The fallthrough elision drops **five** bytes here and four there, because the
   trailing branch is a `jmp rel32` and not an instruction word.

3. **`region.rs`: an x86-64 relocation vocabulary.** `RelocKind` gained
   `Rel32` and `Pc32` beside the four A64 kinds; `elf::r_type` maps them to
   `R_X86_64_PLT32` and `R_X86_64_PC32`, and still **errors** on a kind the
   target has no counterpart for in either direction. The addend is explicit
   everywhere, because on this machine it has to be: a `rel32` and a
   rip-relative `disp32` are measured from the end of their instruction and the
   psABI computes from the start of the field.

4. **The narrower register file.** SysV gives five integer registers to the CPS
   file against AAPCS64's seven (`abi::SYSV_REGISTER_CAPACITY`). `NREGS` is
   three, fits both, and the static assertion in `abi.rs` says so. The `crt`
   family flattens up to ten integer arguments and past the sixth SysV puts them
   on the machine stack where AAPCS64 puts them past the eighth — that is
   clang's business inside the stencil, and the corpus is what confirms it:
   `str.replace` flattens three `Str`s and an out-pointer into ten integer
   arguments — four of them past SysV's six registers — and
   `data/strings.buri` exercises it.

5. **The thirty dropped keys.** Recovered, and not the way this section
   originally proposed. Rewriting the negation as an integer XOR would have
   changed the generated C, hence all three libraries, hence a fresh cache seed
   and a full 997-file re-run — and it would not have worked, because
   InstCombine folds `bitcast(xor(bitcast x, signbit))` straight back to `fneg`
   and the `xorps` constant would have returned. What was done instead is
   faithful and costs no arm64 byte: the spilled section's bytes travel with the
   stencil and the emitter copies them into the unit's own constant pool. §3.2
   has the detail. The three libraries now cover the same 13,904 operations.

6. **`swap_arms` has no x86-64 counterpart**, and still does not. It is the
   twin of `fold_cond`, `fold_cond` is unnecessary here, and whether picking the
   fall-through arm is worth an instruction-motion pass on this ISA is a
   measurement — one that now *can* be made, since CI runs these programs, but
   one this change did not make. It is the single thing on this list left open.

**What CI confirms.** `.github/workflows/ci.yml`'s `linux-x86_64` job is the
twin of the `linux-arm64` one and asserts the same things on the other
instruction set: the census printed (so the suite was live and the corpus is at
macOS parity — 26 of 36, the same 26), every program run, the stack guard's
`mprotect` and Linux's signal disposition for a `PROT_NONE` page, leak parity
through `buri_rt_heap_stats`, `--check-reproducible` on a linked
`linux/x86_64` artifact, and both linkers' idea of an ELF image. That job used
to assert the executing suite **skipped**; it now asserts it **ran**, by the
same census line, and that is the difference the entry point made.

What a maintainer's own machine can still say is bounded in the same way §10.1
bounds it, and one thing is worth naming: `linux_x86_64_objects_link_and_every_relocation_resolves`
links real unit objects with `ld.lld` against a generated stub and checks that
every relocation resolves and that the image still disassembles. That proves
more here than its arm64 twin does — a `rel32` or a rip-relative `disp32`
written at the wrong offset lands inside a *variable-length* instruction, so the
listing does not merely decode wrongly, it desynchronises.

**macOS on x86-64 stays unsupported**, and deliberately. Building it is within
reach of the existing tooling — `sources.rs::compile_flags` would take
`-target x86_64-apple-darwin` and `machobj.rs` already reads Mach-O — but the
*emitter* side is not free: `object.rs` writes A64 relocation types
(`ARM64_RELOC_*`) and would need an x86-64 Mach-O vocabulary of its own,
`RelKind::r_type` answers `None` for the two x86-64 kinds precisely so that a
Mach-O object can never carry one, and nothing this repository runs on or ships
to is that machine. It is refused by name, and that refusal is a test.

## 11. Debug info and backtraces

**No DWARF, and no `.buri_symbols` either.** §9 lists both as gaps rather than
as decisions; this section is what a wave that closes them starts from, and it
is the argument the removed backend's document carried before §13.

Producing DWARF means building `.debug_*` sections by hand —
`rustc_codegen_cranelift/src/debuginfo/` is what that costs, a seven-file
subsystem — and a copy-and-patch emitter has a second problem the first backend
did not: the instructions it copies are clang's, emitted for a C function that
is not the Buri function, so a line table has to be synthesised from the stencil
key and the IR rather than carried through the emission. That is a wave of its
own and it is not this one.

**No `.eh_frame` either, and nothing wants one.** The language has no exceptions
— an abort is a write to stderr and `_exit`, not an unwind (SPEC 6.9,
`generate.rs`) — so there is nothing to unwind, and §3.2's cross builds pass
`-fno-asynchronous-unwind-tables` for exactly that reason: the Linux drivers
default it on, and the result is a `.eh_frame` the size of the code that nothing
reads.

**What was designed to replace both, and does not transfer unchanged.** The plan
written for the removed backend was frame pointers plus a symbol table the
compiler emits itself — a sorted `(address, name)` array in a `.buri_symbols`
section — about eighty lines, giving an abort a stack trace with function names,
with line numbers waiting for DWARF. The symbol-table half is unchanged and is
still worth its eighty lines. The frame-pointer half is not: generated code
makes no use of the machine stack (§8), so what a walker follows is the Buri
stack, and §2's frame records a return area, parameters and locals and **not its
caller** — so the walk needs something the frame layout does not carry today.
Naming that is the point of writing this down rather than assuming the plan
survived the backend it was written for.

The escape hatch when someone needs a real debugger is the JavaScript backend,
which keeps names and structure (`generate.rs`), and `--release`, which will get
DWARF from LLVM for free when CODEGEN-LLVM.md §7 lands.

## 12. Linking, and what "incremental" honestly means

**This section is not this backend's.** The link step is one step and both
native backends reach it; it lives here because this is the backend inside the
loop a developer waits on, and because the document it was written in is gone
(§13). `build/link.rs` is the code, and it has never depended on which backend
produced the objects — they are opaque bytes to it.

### 12.1 Neither mold nor lld does incremental linking

**mold has no incremental mode and will not get one.** There is no
`--incremental` flag; `docs/design.md` has a "Rejected ideas" section that gives
three reasons, and the third is the one that settles it here:

> It's not reproducible, so your binary isn't going to be the same as other
> binaries even if you are compiling the same source tree.

That is a direct contradiction of this toolchain's central claim (`build.rs`).
An incremental linker and `--check-reproducible` cannot both be right. The
author's conclusion — "I wanted to make full link as fast as possible, so that we
don't have to think about how to work around the slowness of full link" — is the
plan.

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

### 12.2 So the granularity is re-compile, not re-link

"Swap only the object files that changed" is delivered, and it is delivered
above the linker rather than inside it:

- A codegen unit whose IR hash is unchanged is **not recompiled**. Its object
  comes out of the content-addressed cache (ARCHITECTURE.md §6.2). This is where
  the seconds are: LLVM codegen of a unit is measured in hundreds of
  milliseconds and a debug unit's in tens or fewer (`design/PERFORMANCE.md` §6).
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

### 12.3 Linker selection

The link is driven through the platform C compiler (`cc`, or `$CC`), never by
invoking the linker directly. The driver is what knows where `crt1.o`, `libc`,
and `libSystem.tbd` live, and reimplementing that is reimplementing the part of a
toolchain that changes with every OS release. It is also the same `cc` this
backend's stencil library is compiled with (§3), so the one external tool the
default toolchain needs is one tool rather than two.

**Linux**, in order: `mold`, `ld.lld`, the system default.

```
cc -fuse-ld=mold -o <artifact> <units...> libburi_rt.a \
   -Wl,--build-id=none -Wl,--gc-sections \
   --target=<arch>-unknown-linux-musl -B musl/lib -L musl/lib \
   -static-pie -unwindlib=none -lunwind
```

That is the line as it is issued, and the order is part of it: everything after
the objects and the archive, because `-lunwind` resolves against what precedes
it. `musl/lib` is **relative** — the link runs in `.buri/link/<link-key>/`, and
an absolute path in a command line is one more thing that differs between two
machines comparing artifacts byte for byte (ARCHITECTURE.md §7).

mold is a drop-in for GNU ld and accepts its options; it is chosen first because
it is 3-10x faster than lld on the benchmarks its README publishes, with the
honest caveat that the advantage is core-count-dependent — mold saturates every
core and lld often does not, so on two cores the gap is much smaller.
`--build-id=none` because a build id is a hash of content we are about to compare
byte for byte, and one fewer thing in the way. `--gc-sections` is `-dead_strip`'s
counterpart: relinking a CI job's own objects with and without it, debug
information removed from both, measured 373 936 bytes against 673 440.

Four of those flags are the libc's rather than the linker's, and each is the
narrowest thing that would work:

- **`--target=<arch>-unknown-linux-musl`** tells the driver which libc's
  conventions to link for. It is also the one flag whose absence is silent: a
  driver that ignores it links glibc and the artifact still runs *here*.
- **`-B musl/lib -L musl/lib`** put the staged sysroot in front of the driver's
  own search path — `-B` for the crt objects, `-L` for `-lc` — and nothing else
  points anywhere. **There is deliberately no `--sysroot`**, and that is a
  measurement rather than an omission: it was the obvious flag and it makes the
  link *fail*. clang locates its GCC installation relative to the sysroot, a
  staged directory has none, and the link ended at `mold: fatal: cannot open
  crtbeginS.o` — then, once those crt objects were baked too, at `mold: fatal:
  library not found: gcc`. The flag buys nothing the two prefixes do not already
  buy, because a link reads no headers. Which files a link needs, and in what
  order, stays the driver's business; `-nostdlib` with a hand-assembled crt
  sequence was the alternative and was rejected on exactly that.
- **`-static-pie`, not `-static`.** Both backends emit position-independent code
  and musl ships the `rcrt1.o` that self-relocates a static PIE before `main`.
  The same flag in debug and release: a *dynamic* musl executable needs a
  `ld-musl-*.so.1` that no glibc distribution has, so the profile-dependent
  variant would be a development build that runs in fewer places than the
  release one.
- **`-unwindlib=none -lunwind`** is the one library the driver must not choose.
  Its default is glibc's `libgcc_eh.a`, which is not libc-neutral: a link that
  keeps it ends at `undefined symbol: _dl_find_object`, glibc's unwinder
  reaching for a glibc-only loader entry point from inside a binary that has
  just left glibc behind. `-lgcc` and `-lc` stay the driver's — `-lgcc` is
  compiler builtins, arithmetic that knows nothing about a libc and which
  **musl's own `libc.a` needs** in order to print a long double.

`-lpthread -ldl -lm` used to be on this line and are gone. musl folds all three
into `libc.a` and ships no `libpthread.a` stub, so against the staged sysroot
`-lpthread` is `cannot find -lpthread` and the link stops there. They survive on
one path only, which is `BURI_MUSL=off`.

**Which libc**, beside "which linker", and on Linux it is the question that
decides whether the artifact is portable at all. Three tiers, in order, with
`BURI_MUSL` (`baked` | `system` | `off`) forcing the choice the way
`BURI_LINKER` forces the flavour:

1. **Baked.** `cli/build.rs` copied musl's `libc.a`, `libunwind.a` and the nine
   crt objects out of this rustc's own `self-contained/` directory — the one
   `rustup target add <arch>-unknown-linux-musl` installs — and `build/link.rs`
   stages them into the link directory. It needs a driver that understands
   `--target=<musl triple>`, which is one memoized `clang -c` probe. This is the
   state every correctly built toolchain is in.
2. **System.** The host is itself musl (Alpine: `cc -dumpmachine` says so) or
   carries a `musl-clang`/`musl-gcc` wrapper, in which case the wrapper *becomes
   the driver*, `-static-pie` still goes on the line, and nothing points it
   anywhere: the machine's musl is already the driver's own libc.
3. **Neither is a refusal**, naming both fixes. There is no silent glibc
   fallback, because the failure it would hide is invisible until the artifact
   is copied to another machine — which is after every test in this repository
   has passed.

`BURI_MUSL=off` links glibc dynamically and is the escape hatch for a machine
that has neither and wants a binary anyway; the executable it produces runs
where that glibc does and nowhere older. The archive's own libc is checked
against the link's: a glibc runtime archive on a musl command line is refused
rather than discovered as `undefined reference to __libc_start_main`.

**macOS**: the system `ld`. `ld64.lld` when `--linker=lld` names it or when no
system linker is found.

```
cc -fuse-ld=lld -o <artifact> <units...> libburi_rt.a \
   -Wl,-dead_strip -Wl,-oso_prefix,.
```

No `-no_uuid`: it was there until macOS 26's dyld began rejecting binaries
without an `LC_UUID`, and ld64's UUID is a content digest, so two identical
links carry one UUID and keeping it costs no reproducibility. No
`-platform_version` either — the driver computes it from the selected SDK and
passes its own, and a second one is an error rather than an override.
`-oso_prefix,.` is ARCHITECTURE.md §7's third source of nondeterminism on the
linker's side: ld64 records an absolute `N_OSO` path for every input carrying
debug information, the runtime archive is one, and the link runs *in* the link
directory, so `.` strips the checkout's path out of the artifact.

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

`-dead_strip` is not decoration: §4 sets `MH_SUBSECTIONS_VIA_SYMBOLS`, and the
two together are why every intra-unit call in an emitted object is a relocation
rather than a baked displacement. Linking the test programs without it once
measured a backend the build system does not produce — every program in
`native/stencil.rs` passed while `buri test` failed 977 of 997 conformance
tests on the same emitter — which is why the harnesses now ask
`build/link.rs::product_link_args` for this line rather than spelling it out.

**Fallback.** With neither mold nor lld present, `cc` uses whatever the system
provides and everything works, more slowly. There is **no** case in which the
build fails for want of a fast linker, and no flag that has to be set to get a
working build. That is the whole of the fallback story, and it is short because
the design does not depend on the linker being any particular one.

### 12.4 The manifest

`.buri/link/<link-key>/manifest` is one line per unit:

```
core_list      3f9a1c2b8d4e...  cached
core_str       71c0aa38f5b1...  cached
lib_money      c40e19b7ad22...  run
main           8b2e01f4c7a9...  run
```

It is written by the link step and read by `--explain`, which prints one
`codegen` line per unit in the existing format (`cache.rs`). It is the answer to
"which objects changed", and it is the thing that makes the claim in §12.2
observable from outside — which is the standard the rest of this build system
already holds itself to (`arguments.rs`: the build system's claims are about
which actions run, and a claim nothing can observe is not one anybody can hold
the toolchain to).

## 13. The backend this one replaced

`DECISIONS.md`'s rule is that a reversed decision is reversed in the document
that made it, with the reversal recorded there rather than deleted. Two of the
three reversed here were made in `CODEGEN-CRANELIFT.md`, and that document went
with its subject, so the record is this section and the index points at it.

**Reversed 2026-08-29.** Three rows moved at once:

* *"The stencil backend is compiled in and never selected"* (ARCHITECTURE.md
  §4). It is what `select` returns for `(Linux | Macos, Debug)` now.
* *"`backend-cranelift` on by default"* (BUILD-AND-WATCH.md §2). The feature and
  the five crates behind it are gone; `default = ["backend-stencil"]`.
* *"Cranelift pinned to the wasmtime 36 LTS"* (CODEGEN-CRANELIFT.md §8). The pin
  outlived its subject by nothing at all: it existed because a bump moved
  `Backend::identity()` and invalidated every cached object in every repository,
  and there is no longer a version to bump.

**Why, in the order the reasons weigh:**

1. **Parity was met and the gate was empty.** 997 of 997 native conformance
   tests through the real build system, the same six packages refused for the
   same three reasons, the same blocks live at exit on all nine measurable
   packages (§9). The gate was written as correctness parity plus a
   re-benchmark; both were answered, and what was left on the list was one
   target rather than one behaviour.
2. **38 transitive crates to 0.** `cargo tree -p buri -e normal` was 39 packages
   and is `buri` alone — `cranelift-{codegen,frontend,module,object,native}`,
   `regalloc2`, `gimli`, `object`, `target-lexicon` and their closure, over half
   of `Cargo.lock`. The root `Cargo.toml` says the policy "used to be 'none at
   all', and native code generation ends that"; for the default toolchain it does
   not end it any more. What this backend needs from outside is a host `cc`,
   which is a platform interface and not a lockfile entry
   (BUILD-AND-WATCH.md §1.1).
3. **A clean release build in half the time.** 142.68 s to 73.94 s, median of
   three interleaved runs on the same tree, and the bench binary from 2 m 02 s
   to 1 m 01 s. Measured 2026-08-29 on macOS/arm64, two builds into one target
   directory. The *shipped binary* did not shrink with it — that is the first
   bullet of what was accepted, below.
4. **x86-64 through CI rather than through a second code generator.** The
   incumbent covered four targets to this backend's two, and that was by a wide
   margin the largest item on the list — and it was a coverage difference rather
   than a performance one, closeable by writing the emitter §10.3 lists. It was
   closed that way, before the flip: `linux-x86_64` emits, links and runs, and
   CI runs the programs on it. macOS/x86-64 stays uncovered and §3.2 and §9 say
   so out loud.

**What did *not* have to be accepted.** The one behavioural difference the flip
would have carried — `str.concat` allocating unconditionally where the removed
backend appended into a block it owned alone — was closed before the flip rather
than accepted with it. The three paths moved into the runtime as
`buri_rt_str_concat`, both surviving backends allocate the same number of times,
and MEMORY.md §5.3's O(log k) promise holds on the debug backend too (§5.0.1).

**What was accepted with it, and is repaired by none of the above:**

* **1.38× on the four run-side kernels**, `opt_level=none` against
  `opt_level=none`, improving from 1.86× and concentrated in one kernel —
  `core/list`'s closure surface, at 2.9× (§5.1). The debug profile is slower than
  it was, deliberately, and `design/PERFORMANCE.md` §6 is where that is tracked.
  A fresh six-kernel series taken after the removal reads **1.26×** (§6.2).
* **A `buri` 5.06 MB larger, not smaller.** The machine-code section fell by
  5.35 MB — `__TEXT.__text` 8.37 MB to 3.03 MB — and the linked binary
  nonetheless grew, 17.57 MB to 22.63 MB, because the three baked stencil
  libraries are 11.93 MB the old binary never selected and the linker
  dead-stripped. `design/PERFORMANCE.md` §5 carries both rows and the rule that
  neither of them may be quoted without the other.
* **The `-O0` geomean and the four-kernel run side are the two figures that are
  not post-removal.** Neither has a harness in this repository, so
  `design/PERFORMANCE.md` §6.2 stands a fresh series beside them rather than
  re-taking them. Every other native figure in §6 was re-taken on 2026-08-29,
  and §6's own banner says which.
* **The benchmark's `lower+*` rows change emitter mid-series.** Goal 3's
  `lower+macos-arm64` figure was Cranelift's and the row is this backend's now;
  `design/PERFORMANCE.md` §6.1 marks the break rather than continuing the series
  through it.

**What moved here rather than being deleted**, because none of it was Cranelift's:
§0 (the IR both native backends consume), §4.1 (reproducibility of the object
bytes), §5.3 (`I128`), §5.4 (aborts), §11 (debug info) and §12 (linking). What
went with the document was true only of a code generator this repository no
longer contains: the CLIF construction rules, the settings table, and the
`return_call` analysis behind DECISIONS.md's tail-call row — which CODEGEN-LLVM.md
§5 argues on its own and never needed the other half.
