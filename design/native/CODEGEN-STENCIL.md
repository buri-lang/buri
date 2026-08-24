# The copy-and-patch backend

A third native backend, behind `backend-stencil`, which is on by default and off
on a host with no C compiler. **It is not chosen by `backend::select`.** The
seat it was meant to take — everything except a native `--release` build — was
gated on correctness parity with Cranelift and on a re-benchmark.

**Parity is met.** Driven through `buri build` and `buri test` — the real build
system, the real per-unit cache, the real batcher and above all the real link —
stencil passes **997 of 997** native conformance tests, refuses the same six
packages for the same three reasons Cranelift refuses them, and leaves the same
blocks live at exit on every package. §9 is the list of what neither backend
does.

**The benchmark's answer was a trade, and the run side has since closed most of
the way.** stencil wins every cell whose time is compiling — emission of a
121k-line program in 367 units is about 0.43× Cranelift's, and it is the first
backend in this repository to reach `design/PERFORMANCE.md`'s goal 3. On the run
side the four kernels are **1.38×** Cranelift `opt_level=none`, from 1.86×
before §5.1's slots-only `crt` family, and the geomean against LLVM `-O0` is
**0.927** — the paper's own bar, met here for the first time, though Cranelift
clears it by more. What is left of the gap is one kernel (`core/list`'s closure
surface, 2.9×) rather than the boundary. And it is one target where Cranelift is
four, which is now by a wide margin the largest difference on the list and is
not a performance one at all.

**A runaway recursion traps.** The Buri stack has a `PROT_NONE` guard above it
(§8), so a program that recurses past it faults where a Cranelift-compiled one
faults, instead of writing into whatever the linker placed next.

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

* **`main`** — `stencil/asm.rs`, which is `cranelift/mod.rs`'s two entry-point
  shims with the same behaviour: `buri_rt_argv_init`, the root or each `test`
  block behind `buri_rt_test_enter`, `buri_rt_flush`, and the exit convention
  of `cli/runtime/lib.rs` §6. It sets `x0` to the Buri stack and calls a
  frame-threaded body.
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
to is x86-64 Mach-O, and `mod.rs::supported` refuses it by name.

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
* **a relocation against a symbol the object itself defines is not a hole.** It
  is a constant clang spilled into `.rodata`, and there is nowhere to put one.
  Thirty keys of 13,904 do it, all in three families where AArch64 has an
  instruction and x86-64 has a constant: `un/neg/f32` and `un/neg/f64` (`fneg`
  against an `xorps` sign mask), `cvt/u2f` (`ucvtf` against the two-bias
  `unsigned long long` → `double` sequence), and `chk/div/i128`. Those keys are
  **dropped** and the emitter refuses those IR shapes with a sentence; the other
  13,874 are unaffected. `the_x86_64_library_drops_only_the_spilled_constant_families`
  is the bound, and it is a bound rather than a list because a drop that spread
  is exactly the failure a per-key refusal makes easy to miss.

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
`build::actions::codegen_units` caches at, and the same granularity
`cranelift/mod.rs` emits at. Two sections:

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

## 5. The runtime boundary

Every operation `middle::lower` leaves as a `Body::Runtime` or an
`Inst::CallIntrinsic` with a `buri_rt_*` symbol is **a call into
`libburi_rt.a`** — the same archive, the same contract
(`cli/runtime/lib.rs`) and the same table shape as the other two backends.
`stencil/runtime.rs` is the third transcription of that contract, key for key and
shape for shape with `cranelift/runtime.rs`, and
`cli/tests/native/conformance.rs`'s companion test is what keeps the three from
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
`Body::Runtime` function — and **both are emitted at the call site**, which is
`cranelift/emit.rs::call`'s first act for the same reason.

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

### 5.2 The one trap worth writing down

A frame slot holds every integer **zero-extended**, whatever its type
(`sources.rs::write`'s convention: "a frame slot is never partially defined"),
and the typed stencils reinterpret the low bytes. So an `I8` of `-3` is `0xfd`
in its slot — and handing that word to a C parameter declared `int64_t` renders
`253`. Every narrow *signed* value crossing to the runtime is widened first
(`rtcall::int_bits`). This is a class of bug, not an instance: it is invisible
in the emitted stencil, invisible in the IR, and shows up as an unsigned number
in a rendered string.

## 6. Reference counting, and the functions a unit generates for itself

MEMORY.md §5.1's saturating increment and its decrement, open-coded as two
stencils, with the cold path a call to `buri_rt_decref` — instruction for
instruction what `cranelift/emit.rs::incref`/`decref` emit, because two backends
deciding separately when a block dies is the one divergence MEMORY.md §5 cannot
tolerate.

`emit::Lower::walk_rc` is `cranelift/emit.rs::walk_rc`, all five site kinds: a
`Str`/`[T]` block, a nested aggregate, a tagged enum's per-variant payloads, a
**boxed** field, and a **niche** whose payload is walked behind its null test.

The last of those is not belt-and-braces. `.None` is written by storing null at
the one pointer the discriminant is and nothing else, so every other byte of the
payload area is whatever the frame last held; walking it unguarded decrements a
count at an address that was never a pointer. `cranelift/emit.rs`'s
`Site::Guarded` is the same test for the same reason.

### 6.1 `glue.rs`

Four things a unit generates for itself, which is `cranelift/helpers.rs`'s set
under `cranelift/helpers.rs`'s argument, and every one a **local** symbol so
that two units needing the same one do not collide:

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
types a program holds. `cranelift/emit.rs::walk_or_call` is the same threshold,
and `conformance/lib/semantics/test/generics.buri` is the file that needs it.

### 6.3 The closure environment is a block

`MakeClosure` allocates `[release fn][record]` and puts the pointer in the
closure's `env` word, exactly as `cranelift/emit.rs::build_env` does, and
`walk_rc` counts that word. Carrying the environment by value in one word — what
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
  **Cranelift does not enable them either**, so a machine frame past the OS
  guard has the same exposure on the incumbent. The two backends are level
  rather than one being sound; a megabyte is far past any frame `middle::layout`
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

### 8.2 What a program does when it runs out

The same thing a Cranelift-compiled one does, which is what parity here means:
the process dies on the fault, with no message, and the shell reports the
signal. Measured on the same non-tail recursion, both backends through
`buri build`: Cranelift **exits 139** (`SIGSEGV`, the OS guard under the machine
stack) and stencil **exits 138** (`SIGBUS`, the `PROT_NONE` guard above the Buri
stack). SPEC §6.10 asks for "a message on stderr and a non-zero exit status" and
neither backend prints the message; that gap is the *runtime's* — it has no
fault handler — and it is shared, not this backend's, so closing it here would
make the two disagree. What this section closes is the difference that was
stencil's alone: **a deep recursion used to corrupt whatever the linker placed
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
`PACKAGES` records the same reasons for Cranelift, and stencil refuses the same
six conformance files:

* an **inexact** numeric conversion. `x.toT()` where not every value fits
  answers `Result<T, RangeError>` (SPEC 6.2.1), and `RangeError` is a struct of
  two `Str`s the backend would have to build. This subsumes float→integer
  entirely: no float-to-integer conversion is exact, so every one of them is
  this shape.
* **`json.*` and `derivePrimJson`** — a descriptor-driven walker.
* **`core/math`'s thirteen transcendentals**, which are refused rather than
  unwritten; `cli/runtime/math.rs` argues it.

**stencil's own, and each is a sentence rather than a wrong answer:**

* **x86-64 execution.** The stencils are built and extracted (§3.2) and
  `elf.rs` writes the container, but `asm.rs`'s two hand-written shims have no
  SysV counterpart, so `mod.rs::supported` refuses the target with that sentence
  rather than emitting an object with arm64 bytes in `main`. §10.3 is the list.
* **Linux execution from this host.** `linux-arm64` emits objects that a real
  linker accepts and fully resolves, and that is as far as *this* machine can
  go — §10.1 says why. CI runs the programs (§10.2).
* **Debug information** — neither DWARF nor `.buri_symbols`, which is the gap
  `cranelift/mod.rs` records for itself too.
* **In-place `str.concat`.** `cranelift/helpers.rs`'s appends into the left
  operand's block when it owns it alone (MEMORY.md §5.3); this always allocates.
  The string is the same either way, so nothing observable through `Show`
  differs — but the *allocation count* does, and `core/alloc`'s `count` and
  `total` are observable, so it is a divergence and not a missing optimisation.
  It is `malloc` calls that differ and not the model: `memory/allocators.buri`
  passes, because MEMORY.md §7's cost model is *defined* from the types.
* **An element wider than the staging room a frame keeps** (`lists.rs::STAGE`).
  A `zip`, a `flatten` and a `sortBy` move whole elements between two blocks
  through the frame, and the frame's scratch is a constant; past it the shape is
  refused with the two numbers in it.
* **A `Float` hashed or shown at `F32`** through the runtime boundary: a `crt`
  stencil declares every float parameter `double`, and an `F32` sits in its slot
  as its own thirty-two bits, so the shape `buri_rt_show_f32` wants is one this
  call boundary does not have.

**Shared with Cranelift and not the backend's at all.** Whatever a conformance
package leaves live at exit, **both backends leave exactly the same blocks and
the same bytes** — measured through the runtime's own `buri_rt_heap_stats`, on
the objects `buri test` produced, re-linked with `build/link.rs`'s own flags.
Most recently that is 0 live on all nine measurable packages, where earlier
rounds recorded three files leaking (17, 5 and 20 blocks); the count moves with
`middle::rc` and the *parity* is what this document claims. What emits a release
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
* `rustc` on this machine has no `*-unknown-linux-gnu` standard library
  installed, so one cannot be produced here either; and there is no `qemu`, no
  `docker`, and no initialised `podman` machine.

That is exactly where `cli/benches/compiler.rs`'s `lower+linux-*` rows already
stop for Cranelift: they lower and emit object bytes, and nothing is linked and
nothing is run in any native row.

**What was checked here**, and it is more than nothing:

| claim | how |
|---|---|
| the two Linux libraries build, cross, from this host's clang | `cli/build.rs`, three blobs in `OUT_DIR` |
| the arm64 libraries cover the same 13,904 operations | `the_two_arm64_libraries_cover_the_same_operations` |
| x86-64 drops only the three spilled-constant families | `the_x86_64_library_drops_only_the_spilled_constant_families` |
| `elf.rs`'s output reads back through an independent ELF reader | `elf::tests::what_is_written_is_what_the_reader_reads` |
| `b`/`bl` and `add`/`ldr` get the right *split* relocation type | `a_branch_is_split_by_its_instruction`, `a_low_twelve_is_split_by_its_instruction` |
| `ld.lld` **accepts** real `linux-arm64` unit objects, statically links them, and **every relocation resolves** with none left in the image | `linux_arm64_objects_link_and_every_relocation_resolves` |
| the linked image still disassembles as arm64 and has a `main` | same test |
| two emissions are the same bytes, for both Linux targets | `a_cross_emission_is_reproducible` |
| a target with no library, and one with no entry point, are refused with different sentences | `an_unsupported_cross_target_is_refused_with_a_reason` |
| macOS is unregressed | the existing 997-file conformance corpus, unchanged |

The link in that suite uses a **generated stub** in place of `libburi_rt.a`,
derived from whatever the objects themselves leave undefined. That is honest
about what it proves: the *shape* of every reference, and nothing about what the
referent does.

### 10.2 What a Linux run had to confirm, and where it is confirmed

The aarch64 column is discharged. `.github/workflows/ci.yml` is where it stopped
being prose: the suite runs on `macos-latest`, `ubuntu-24.04` and
`ubuntu-24.04-arm`, and the `linux-arm64` job runs the artifacts rather than
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

### 10.3 The x86-64 emitter, which is not written

**x86_64-unknown-linux-gnu** needs everything the aarch64 column needed and,
before any of it can be asked, six pieces:

1. **`asm.rs`: a SysV entry point.** `program_entry`, `test_entry` and
   `install_guard`, hand-encoded, with `rdi` as the frame pointer. This is the
   one piece that is deliberately not written here: it is 900 lines of
   convention in the arm64 case, and writing its twin on a machine that cannot
   execute a single instruction of it would be speculation with a code
   generator's name on it. `asm::AVAILABLE_X86_64` is the flag that says so.
2. **`jit.rs`: the patcher.** Four small functions, and the shapes are already
   decided by §3.2's table: `rel32` from the instruction's end for a branch and
   for a `jcc` (the same arithmetic, unlike arm64 where they are different
   fields); the `lea` → `mov` rewrite for `Imm32`; and the pool retarget for
   `Imm64`, which must use the **per-site instruction end** recorded in
   `Hole::pairs` and not a constant four. Note that `pairs` means
   `(instruction end, field)` on x86-64 and `(adrp, add)` on AArch64 — the two
   ISAs never share a patcher, and each meaning is stated where it is produced
   and where it is consumed.
3. **`region.rs`: an x86-64 relocation vocabulary.** `RelocKind` is
   `Branch26`/`Abs64`/`Page21`/`PageOff12`, which is an arm64 list.
   `elf::r_type` maps the first two and **errors** on the last two for an
   x86-64 target; that error is where this gap is meant to show, rather than a
   nearest-number guess.
4. **The narrower register file.** SysV gives five integer registers to the CPS
   file against AAPCS64's seven (`abi::CAP_REGS_SYSV`). `NREGS` is three and
   fits both, and the static assertion in `abi.rs` says so — but the *`crt`*
   family flattens up to ten integer arguments, and past the sixth SysV puts
   them on the machine stack where AAPCS64 puts them past the eighth. That is
   clang's business inside the stencil, and it is worth confirming rather than
   assuming.
5. **The thirty dropped keys.** `un/neg/f32` and `un/neg/f64` are the ones a
   real program hits. They could be recovered by writing the negation as an
   integer XOR of the sign bit instead of `-x`: `0x8000000000000000` is an
   encodable AArch64 logical immediate, so that form is *also* one instruction
   with no memory reference there, and it would remove a `.rodata` constant
   here. It was not done, and the reason is scope discipline rather than doubt:
   it changes the generated C, so it changes 4.19 MB of `macos-arm64` stencils,
   which means a fresh cache seed and a full 997-file re-run to land. It belongs
   in the same change as the x86-64 emitter, where it can be measured.
6. **`swap_arms` has no x86-64 counterpart**, because it is the twin of
   `fold_cond` and `fold_cond` is unnecessary there. Whether picking the
   fall-through arm is worth an instruction-motion pass on this ISA is a
   measurement, and it needs a machine that can run one.

None of that is started, and the workflow says so rather than staying quiet.
Its `linux-x86_64` job asserts that
`an_unsupported_cross_target_is_refused_with_a_reason` runs and passes on an
x86-64 Linux host, and that the executing suite **skips** there. A census
printed on x86-64 would mean `stencil::AVAILABLE` had become true without
`asm.rs` gaining the SysV entry point item 1 asks for, which is the one way this
port could go wrong quietly.
