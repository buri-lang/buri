# The copy-and-patch backend

A third native backend, behind `backend-cpjit`, which is on by default and off
on a host with no C compiler. **It is not chosen by `backend::select` yet.**
This document is the design as wave 1 built it; the seat it is meant to take —
everything except a native `--release` build — is gated on correctness parity
with Cranelift and on a re-benchmark, and §9 says exactly what is between here
and there.

The technique is Haoran Xu and Fredrik Kjolstad's *Copy-and-Patch Compilation*
(OOPSLA 2021). Section references of the form "§4.3 of the paper" are to it;
everything else is to this repository.

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
free. That is the whole code generator: `cpjit/jit.rs::emit`, about sixty lines.

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
of each other with no trend. `cpjit/abi.rs::NREGS`.

### 2.1 The two places C has to be bridged

The convention is not the C one, so two things cross the boundary and both are
hand-written rather than emitted from stencils:

* **`main`** — `cpjit/asm.rs`, which is `cranelift/mod.rs`'s two entry-point
  shims with the same behaviour: `buri_rt_argv_init`, the root or each `test`
  block behind `buri_rt_test_enter`, `buri_rt_flush`, and the exit convention
  of `cli/runtime/lib.rs` §6. It sets `x0` to the Buri stack and calls a
  frame-threaded body.
* **a runtime call** — `cpjit/rtcall.rs`. §5.

## 3. Where the stencils come from

`cli/build.rs` generates about twenty-three thousand C functions
(`cpjit/sources.rs`), compiles them with the host `cc` in twelve parallel
shards, reads the objects back (`cpjit/machobj.rs`), extracts one stencil per
exported `st_*` symbol with its relocation records as holes
(`cpjit/extract.rs`), applies four instruction-rewriting folds, and writes the
serialized library into `OUT_DIR` for the backend to `include_bytes!`.

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
* **Degrades rather than breaks.** A host with no `cc`, or one that is not
  arm64, gets an **empty** library; `cpjit::AVAILABLE` reads the emptiness and
  the backend reports itself unavailable, exactly as
  `runtime_native::AVAILABLE` does for the archive.
* **`Backend::identity` is the library's hash.** Not a version string, because
  there is no version to name: the bytes depend on the generators *and* on
  whichever `cc` was on the host. Hashing the library covers both, so two
  toolchains built against different C compilers share no cached object.

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
rewrites in `cpjit/extract.rs` recover them: `fold_addressing` puts a frame
offset in a load or store's own `imm12`; `fold_imm` puts a literal in an
`add`/`sub`/`cmp`'s; `fold_cond` makes a conditional branch's `imm19` a hole, so
a two-way branch is two instructions instead of three; and `swap_arms` builds
the twin with the arms exchanged, so the emitter can pick whichever one falls
through. Each stencil goes into the library with up to four twins, and
`Jit::emit` picks the most specific one whose fields the operands fit.

## 4. From a JIT to a backend

The prototype this backend grew out of wrote into an `mmap`ed, `PROT_EXEC`able
region and patched absolute addresses into it, because it executed what it
emitted in its own process. A backend does not. The same emitter now writes into
a plain `Vec<u8>` whose "addresses" are section offsets, and records what it
cannot resolve as a relocation (`cpjit/region.rs`).

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

* `__TEXT,__text` — the code. A call to a function this unit owns is a `bl` with
  a displacement, resolved at emit time; one to another unit's is an
  `ARM64_RELOC_BRANCH26` against `ir::Func::symbol`, which is the name both
  sides already agree on.
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

`cpjit/object.rs` is the Mach-O writer. There is no ELF one, which is why
`Platform::Linux` is refused with a sentence rather than half-attempted.

## 5. The runtime boundary

Every operation `middle::lower` leaves as a `Body::Runtime` or an
`Inst::CallIntrinsic` with a `buri_rt_*` symbol is **a call into
`libburi_rt.a`** — the same archive, the same contract
(`cli/runtime/lib.rs`) and the same table shape as the other two backends.
`cpjit/runtime.rs` is the third transcription of that contract, key for key and
shape for shape with `cranelift/runtime.rs`, and
`cli/tests/native/conformance.rs`'s companion test is what keeps the three from
disagreeing about which keys exist.

This is the wave's largest single deletion. The prototype had its own
`intrin.rs`: a descriptor-driven helper per operation, written in Rust, living
in the compiler's process. That could not survive object emission — a symbol in
the compiler is not a symbol in the artifact — and it was, in the honest naming,
`libburi_rt.a` written a second time, with every `num.U64.checkedMul` the
language ever adds having to be written twice.

### 5.1 How the arguments get there

`cli/runtime/lib.rs` §2's rule is: the flattened Buri arguments, then the
element pair, then the out-pointer. What differs here is only *where* an
argument is. Cranelift builds a value list and lets its register allocator place
it; this backend has no register allocator at the call boundary, and a stencil
that read eight independent frame offsets would make the shape the cross product
of eight holes — thousands of stencils.

So the arguments are copied into a **contiguous scratch area** with the ordinary
`mov` and `imm` stencils the emitter already has — which is what the
frame-threaded convention does for a Buri call anyway, so it is the same store
to a different address — and one `crt` stencil reads them off consecutively into
`x0`–`x7` and `d0`–`d1`. The shape is then just `(integers, floats, result)`,
and the library holds 108 of them.

Two areas rather than one, because AAPCS64 assigns the two register banks
independently: a double in argument position three still goes in `d0` if it is
the first float.

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

## 6. Reference counting

MEMORY.md §5.1's saturating increment and its decrement, open-coded as two
stencils, with the cold path a call to `buri_rt_decref` — instruction for
instruction what `cranelift/emit.rs::incref`/`decref` emit, because two backends
deciding separately when a block dies is the one divergence MEMORY.md §5 cannot
tolerate.

`emit::Lower::walk_rc` is `cranelift/emit.rs::walk_rc` with two of its five site
kinds **refused rather than emitted**:

* a `[T]` whose element is itself counted needs a per-element **drop glue**,
  which is a C-ABI `fn(*mut u8)` this backend does not emit yet;
* a **boxed** field is a heap indirection whose layout question is the same one
  `MakeEnum` of a boxed field already refuses.

Both are refusals and not omissions on purpose: a missing release is a leak, and
a leak that compiles is a wrong program that passes its own tests.

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

## 8. The Buri stack, and the deviation it is

Generated code makes no use of the machine stack, so the Buri stack is a
zero-filled 64 MiB block the unit owning `main` emits, and `main` passes its
address as the first frame pointer. It costs nothing in the object or on disk —
a zero-fill section has a size and no bytes — and nothing in the artifact until
it is touched.

It has **no guard page**. A Cranelift-compiled program uses the real machine
stack and gets the operating system's; a cpjit-compiled one that recurses past
64 MiB writes into whatever follows. That is a real deviation from the other two
backends and not a detail: it is written here, in `cpjit/asm.rs`'s header, and
in `cpjit/mod.rs`'s, and closing it means either an `mmap` with `PROT_NONE`
either side at startup or a stack-limit check in the `call` stencil, and the
second costs an instruction per call.

## 9. What is not here yet

Named rather than left to be discovered. Every one of them is a refusal today,
so a program that needs one is told; none of them is a wrong answer.

* **Drop glue** (§6), and with it every `[T]` whose element carries a count at a
  runtime boundary or an `ArraySlice`.
* **The closure thunk.** `cranelift/emit.rs::make_closure` builds one for every
  closure so that `{code, env}` has one shape; this backend calls the target
  directly, which is only sound when the target already has an environment
  parameter. A synthetic function per target, an argument shuffle and a call.
* **An environment wider than one word**, which needs the heap block
  `cranelift/emit.rs::build_env` allocates and counts.
* **`deriveArrayEq` / `deriveArrayShow` / `deriveArrayJson` / `deriveArrayHash`**,
  which are the same loop over a code pointer the closure surface needs.
* **`GetPayload`/`MakeEnum` of a boxed field**: one heap indirection per
  recursive enum.
* **128-bit arithmetic**, and `Ret::Res` in the runtime table.
* **`core/bits`**, `num.minValue`/`maxValue`, and the `checked`/`saturating`/
  `wrapping` families.
* **Linux**, and **x86-64**: the stencils are arm64 and `object.rs` writes
  Mach-O.
* **Debug information** — neither DWARF nor `.buri_symbols`, which is the gap
  `cranelift/mod.rs` records for itself too.
* **In-place `str.concat`.** `cranelift/helpers.rs`'s appends into the left
  operand's block when it owns it alone (MEMORY.md §5.3); this always allocates.
  The string is the same either way, so nothing observable through `Show`
  differs — but the *allocation count* does, and `core/alloc`'s `count` and
  `total` are observable, so it is a divergence and not a missing optimisation.
