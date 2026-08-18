# The native value model

`runtime.js:7-30` documents the JavaScript one: every integer is a double, a
struct is an array, an enum is a number or `[tag, ...payload]`, `None` is
`undefined`. This document is the other one — sized integers, a struct layout, a
tagged union — and the language-visible consequences of the change, which
the roadmap correctly said need a SPEC amendment rather than a quiet
divergence.

The model is computed by `middle::layout` (ARCHITECTURE.md §2.2) and both native
backends read the same table. A layout Cranelift and LLVM disagreed about would
be a miscompile visible only when comparing profiles, so there is one
implementation and both consume it.

## 1. Scalars

| Type | Native | Notes |
|---|---|---|
| `()` | nothing | zero-sized; never passed, never stored, never loaded |
| `Bool` | `i1` in a register, `i8` in memory | 1 byte, values 0 and 1 only |
| `I8 … I128` | `i8 … i128` | two's complement |
| `U8 … U128` | `i8 … i128` | same bits, different operations |
| `F32`, `F64` | `f32`, `f64` | IEEE-754, as SPEC 6.2 already requires |
| `Char` | `i32` | a Unicode scalar value, not a code unit |
| `Int` | `i64` | `Int` is `I64` (`runtime.js:11`), and now it is |
| `Template` | `Str` | see §3.3 |

`Int = I64` for real. The consequence is the whole of §7.

`Char` as `u32` rather than as a small string is not a choice — `Char` is one
Unicode scalar (`str.buri:73` returns `[Char]` from `chars`, and `char.toU32()`
is exact per SPEC 6.2.1). The JS backend spells it as a one-scalar string because
JavaScript has no character type, and `Char` comparison there is string
comparison. Natively it is an integer comparison, which is the same answer by a
cheaper route: Unicode scalar order and UTF-8 byte order agree.

`I128`/`U128` are the one place a backend can fall short of the type system, and
the fallback is stated in CODEGEN-CRANELIFT.md §3.6 rather than here, because it
is a backend limitation and not a model decision. The model says 128-bit
arithmetic is exact; a backend that cannot do it in registers calls the runtime.

Every scalar is aligned to its own width, including the 128-bit pair: `i128` is
**16-aligned**, not 8-aligned. That is what LLVM, clang and the SysV ABI all
mean by `i128`, and §10's boundary is the reason it is not a free choice —
`cli/runtime` is Rust with `#[repr(C)]` types, so a layout pass that aligned
`i128` to 8 would disagree with the runtime at the one place disagreement is not
caught by a test of either side alone.

## 2. Heap values, and the one header

Every heap allocation has a **16-byte header immediately before the payload**:

```
  ptr - 16   u64  rc     reference count, or IMMORTAL
  ptr -  8   u64  cap    usable payload bytes
  ptr        ...  payload
```

Sixteen bytes and not eight, for three reasons that each independently decide it:
the payload stays 16-byte aligned, which every SIMD type and every `F64x2` in
`core/simd` wants for free; `cap` is what the free path needs to return a block
to the right size class; and `cap` is what MEMORY.md §5's in-place reuse tests
against. One header shape for every heap value, so `incref` and `decref` are two
instruction sequences in the whole program rather than one per kind.

`rc == u64::MAX` is `IMMORTAL`: a value that is never counted and never freed.
Every string literal, every constant aggregate the middle end interns, and every
zero-sized value has it. `incref` is a saturating add, so it is branchless;
`decref` tests for it, which is one well-predicted compare. MEMORY.md §5 has the
sequences.

## 3. `Str`

```
struct Str { base: *Header, ptr: *const u8, len: u64 }     // 24 bytes
```

`base` is the **payload pointer** of the allocation the bytes live in — the
pointer `buri_rt_alloc` returned, with the header at `base - 16` — and not a
pointer to the header itself, notwithstanding the `*Header` above. That is what
makes `incref(base)` and `decref(base)` the same two instruction sequences every
other heap value uses, which is §2's whole reason for having one header shape.

UTF-8, immutable, and **sliceable** — which is the requirement that decides the
shape. `core/str`'s own header says it: "`trim`, `slice`, and `splitOnce` are
pure because it is immutable and sliceable: they return views, not copies"
(`str.buri:3-4`), and `splitOnce` is documented as pure "because neither half is
a copy" (`str.buri:42-43`). A view's `ptr` is in the middle of somebody else's
allocation, so the reference count cannot be found by subtracting 16 from it.
`base` is what the count is on.

`base` is null for a literal or a static, which are `IMMORTAL` anyway, so a
literal string is three immediate constants and touches no allocator.

### 3.1 `len` is scalars, and the top bit of `len` says how much that costs

`str.len()` is "the number of Unicode scalar values, not the number of UTF-8
bytes" (`str.buri:17-18`). So the byte length in the value and the number the
language reports are different numbers, and one of them has to be computed.

The field holds the **byte** length in its low 63 bits — a view has to know
where it ends, and that is a byte offset — and bit 63 answers what the *scalar*
count costs.

Bit 63 of `len` is the **ASCII flag**. Set means every byte in the view is below
0x80, so the scalar count equals the byte count and `len()` is a mask. Clear
means the scalar count is computed by counting bytes with `(b & 0xC0) != 0x80` —
a loop that vectorizes to one compare and one popcount per 16 or 32 bytes.

This mirrors the JavaScript backend exactly. `$str_len` is `$wide(s) ? $chars(s).length : s.length`
(`runtime.js:695-697`): O(1) when the string is in the basic plane, O(n)
otherwise. Native is O(1) for ASCII and O(n) otherwise. The boundary is drawn in
a different place — JavaScript's fast path is "no astral characters", ours is
"no non-ASCII" — but the shape is the same, and no program's asymptotics change
between backends on the input that matters, which is ASCII.

The flag is computed once, by whichever runtime function built the string; the
builders already scan the bytes. Slicing an ASCII string yields an ASCII string,
so the flag survives `trim` and `slice` and `splitOnce` for free. Slicing a
non-ASCII string leaves the flag clear even where the slice happens to be ASCII:
rescanning on every slice would cost the thing slicing exists to avoid.

Strings are capped at 2^63 - 1 bytes, which is not a cap.

### 3.2 No small-string optimization

Rejected for v1. It puts a branch in front of every `ptr` read in the program, it
doubles the number of `Str` shapes every backend and the runtime must handle, and
it buys least where this language spends most — `Str` is already a view, so the
allocation an SSO avoids has usually already been avoided by pointing into a
parent buffer. The measurement that would reopen it is a profile showing
`str.concat` and `str.fromInt` dominating an allocation-bound program, and the
growth path is a tagged `base` (low bit set means the other 23 bytes are inline
UTF-8), which nothing here forecloses.

### 3.3 `Template`

`Template` is `Str`. The backend renders every hole from its static type and
joins the parts (`runtime.js:22-23`), which is a middle-end rewrite of
`ExprKind::Template` into a `str.concat` chain and is already what happens; there
is no `Template` value at run time on either backend.

## 4. `[T]`

```
struct List { ptr: *const T, len: u64 }                    // 16 bytes
```

Elements are contiguous, at `layout(T).stride`. The header is at `ptr - 16`,
because unlike `Str` a list is **never a view**: every one of `slice`, `take`,
`drop`, `concat`, `push`, `reverse` and `filter` in `core/list` is bounded by
`Alloc` (`list.buri:94-120`), which is the language saying they allocate. So
`ptr` is always a payload start and 16 bytes suffice.

`len` is the element count, exactly. There is no ASCII-flag equivalent because
`list.len()` is the element count and always O(1) (`list.buri:18`).

### 4.1 A flat array, not a persistent vector

Rejected: a RRB tree, a chunked deque, or any structure with structural sharing
on append.

The stdlib's list surface is bulk producers — `map`, `filter`, `fold`, `range`,
`repeat`, `zip`, `flatten` — which build a whole array at once and read it
linearly. There is a `push` (`list.buri:100`) and it is `Alloc`-bounded, which
is the language stating that it copies. Making `push` cheap by making every other
operation indirect is the wrong trade for this library.

More decisively: `sum` (`list.buri:148`) and `core/simd` want a contiguous
`i64*`. A flat array is the only representation where a fold over `[Int]`
compiles to a vectorizable loop, and vectorizing folds is most of what a native
backend is for here.

What recovers append performance is not a different data structure, it is
uniqueness: `xs.push(ctx, x)` where `xs` has a reference count of 1 and spare
capacity writes in place and returns the same pointer. That is MEMORY.md §5, it
is invisible in the type, and it makes the loop `for … { xs = xs.push(ctx, x) }`
amortized O(1) without changing what `[T]` is.

That is not a plan: it is `cli/runtime/list.rs`'s `append_dest`, behind
`list.push` and `list.concat` on both backends, with the capacity coming from
doubling on the reallocating path. `cli/tests/native/cranelift.rs`'s
`a_unique_push_loop_allocates_logarithmically` is the amortization stated as an
allocation count. The one restriction is that an element type holding counted
references — `[Str]` — still copies; MEMORY.md §5.3 says why, and it is a
property of the drop glue rather than of `[T]`.

## 5. Tuples and structs

Fields in **declaration order**, at natural alignment, C layout. Size rounded up
to alignment. Nothing is reordered.

Because a size is already rounded up to its own alignment, `stride` — what a
`[T]` indexes by — equals `size` for every type in this model. Both are in the
layout table anyway: they are different questions, one about the distance
between two elements and one about how many bytes a value is, and a model that
spelled them with one number would have to be re-read the day a packed
representation makes them differ.

Reordering to close padding is the obvious optimization and it is not taken in
v1, because `Desc::Struct` (`monomorphize.rs:120`) carries `fields` in
declaration order and every derived operation is a fold over that order —
`derive Show` prints them in it, `derive ToJson` writes a positional struct as an
array in it (and that array's element order is *wire
format*). A layout pass that reorders and a descriptor that does not is two
orderings that must be kept in step by hand, and the failure is a JSON document
with its fields transposed. The growth path is one field on `DescField` — the
byte offset — after which reordering is safe and mechanical.

The padding cost is small in practice: this language has no `u24`s and no
bitfields, and the common shapes (all-pointer, all-i64) have none.

A struct or tuple is stored inline wherever it appears — inside another struct,
inside an enum payload, as an array element. It is heap-allocated only when a
`[T]` of it is built, and then the whole array is one allocation. So `[(A, B)]`
from `list.zip` is one block, not `n` pairs.

### 5.1 The calling convention flattens

Buri-to-Buri calls do not use the platform C ABI. Every aggregate parameter is
**flattened into its scalar leaves** and passed as separate arguments, up to
eight; beyond eight leaves the aggregate is passed by pointer to caller-owned,
immutable memory.

This is what makes a 24-byte `Str` cost three registers rather than a stack slot:
SysV would classify a 24-byte struct as MEMORY and spill it. Since both sides of
every Buri call are generated by this compiler, there is no ABI to be compatible
with, and the one place there is — `cli/runtime`'s C entry points — takes the
platform ABI and is written in Rust with `#[repr(C)]` types to match (§9).

Cranelift is told this as a signature with N scalar parameters; LLVM the same.
Neither needs `byval`, `sret`, or a struct type in a signature anywhere.

### 5.2 Where a recursive type's indirection goes

§4 covers most of it by accident: `enum Rose { Node([Rose]) }` needs no
indirection introduced anywhere, because `[T]` already is one and laying out a
list never asks for its element's layout. The same is true of a closure, whose
layout is two pointers whatever it is a closure over. Those are the two places
recursion stops for free.

They are not all of it. `enum Tree<T> { Leaf, Node(Tree<T>, T, Tree<T>) }` is
legal, is in SPEC 5.4's own example list, and is annotated there as "boxed by the
runtime" — which is a promise the layout pass has to keep, because a `Tree`
stored inline in a `Tree` has no finite size. So:

> A field is stored **behind a pointer** exactly when its owner's type
> constructor is in a recursion group that is a genuine cycle, and the field's
> type mentions a constructor of that group in a position that would be stored
> inline. Recursion groups are the strongly connected components of "mentions
> inline", where a generic argument counts and `[T]` and `fn(..) => T` do not.

Three things follow, and each of them is the reason the rule is stated over
groups rather than over back edges found while walking:

- **The answer does not depend on the order layouts were asked for.** In a cycle
  `A -> B -> A`, *both* edges are boxed. A rule that boxed the edge it happened
  to close on would give `A` one layout when `A` was asked for first and another
  when `B` was, and a memo table would hand out whichever it computed. That is a
  miscompile that reproduces only under one build order.
- **A constructor inside itself at smaller arguments is not a cycle.**
  `Option<Option<T>>` and `Pair<Pair<Int>>` share a constructor with their own
  payload, but `Option`'s declared payload is its parameter, so `Option` mentions
  nothing and its group is a group of one with no self-edge. Nothing is boxed and
  §6's tagged `Option<Option<T>>` holds its payload inline.
- **A pointer introduced this way is never null**, so it is a niche candidate
  (§6), which is what makes `Option<a box-shaped struct>` free.

The cost of boxing both edges of a two-cycle rather than one is an indirection
per level in a shape nothing in the standard library has — `core/json`'s `Json`
recurses through `[Json]` and is not boxed at all. The growth path, if a profile
ever wants it, is to pick a canonical edge per group by declaration order, which
is a change to one predicate.

## 6. Enums

`tag ++ payload`, where the tag is the smallest of `i8`/`i16`/`i32` that holds
the variant count, the payload area is the union of the variants' field layouts
at the maximum alignment among them, and the whole is a struct at that alignment.
A variant's fields are laid out inside the payload area in declaration order,
independently per variant.

The tag is at **offset 0** and its value is the variant's **index in
declaration order**, which is the number `derive Ord` already compares and the
number a decision tree already switches on. An enum with no variants at all is
uninhabited, has no value, and occupies nothing.

Two niches, both on day one, both because the IR already assumes them:

- **An enum whose payload area is empty is a bare integer.** `Desc::payloadless`
  already exists and already means exactly this (`monomorphize.rs:141-147`), and
  the JS backend already compiles equality on one to `a === b`
  (`generate.rs:374-377`). Stated in bytes rather than in fields, so that
  `Option<()>` — one variant with a zero-sized field — is a byte too, rather than
  a byte of tag and a payload area of nothing after it.
- **`Option<T>` where `T`'s layout has a pointer field with a known-nonnull
  invariant is the pointer, with null for `.None`.** `Option` already has no tag
  in the IR: `Desc::Option(inner)` says only what the payload is, because "`None`
  is `undefined` and `Some(x)` is `x`" (`monomorphize.rs:969-979`,
  `runtime.js:22-27`). The niche keeps that true natively for the case that
  matters — `Option<Str>`, `Option<[T]>`, `Option<Box-shaped struct>` — at zero
  cost.

"A pointer field with a known-nonnull invariant" is a short list, and it is
worth writing out because half of the pointers in this model *are* nullable and
picking one of those would be a silent miscompile rather than a missed
optimization:

| Shape | The niche | Why not the other one |
|---|---|---|
| `Str` | `ptr`, at offset 8 | `base` is null for a literal (§3) |
| `[T]` | `ptr`, at offset 0 | a list is never a view, so `ptr` is always a payload start (§4) |
| a closure | `code`, at offset 0 | `env` is null when nothing was captured (§7) |
| a struct or tuple | the first such pointer inside it, by offset | — |
| a field boxed by §5.2 | that pointer | — |
| an enum | none | which pointers exist depends on the tag |

`.None` is that one word set to null; the rest of the value is not written and
not read. So `Option<Str>` is 24 bytes, exactly a `Str`, and testing it is one
compare against zero.

Everything else gets a tag. In particular **`Option<Option<T>>` gets a tag**, and
that is a semantic improvement over JavaScript rather than a cost: `runtime.js:24-27`
records that `Some(None)` and `None` collide there, and that the collision is why
`Option<T>` in JSON does not round-trip. Natively it does
not collide. §8 lists the test.

General niche discovery — scanning a type for any unused bit pattern, Rust-style
— is deferred. It is a large amount of machinery for a language whose enums are
mostly `Option` and `Result`, and `Result<T, E>` gets no niche from it anyway
because both arms carry payloads.

## 7. Closures

```
struct Closure { code: *const fn, env: *const Env }        // 16 bytes
```

`middle::closures` (ARCHITECTURE.md §2.2) lifts every lambda to a top-level
function taking `env` as an extra first parameter, and builds `Env` as an
ordinary struct of the captured locals — which `ExprKind::Lambda { captures }`
already lists (`typed.rs:194`).

A lambda that captures nothing has a null `env`, and the middle end rewrites a
call through a known-empty closure into a direct call, so `xs.map(ctx, double)`
costs no indirection at all. `ExprKind::FnRef` is already exactly that case.

The environment is plain immutable data with no capability in it, because SPEC
10.6 forbids capturing an effect-carrying value and the checker enforces it. That
is what makes the environment an ordinary reference-counted record with an
ordinary generated `drop`, and it is what makes MEMORY.md §2's acyclicity
argument go through.

### 7.1 What `code` and `env` actually point at

Two additions the section above does not follow from, both forced by §5.1 and
both landed identically in `backend/cranelift` and `backend/llvm`. A closure
built by one backend and a closure built by the other have the same shape, which
is what lets this section stay one description rather than two.

**`code` is always a generated thunk, never the lifted lambda.** The lifted
lambda takes its environment as an *aggregate* first parameter, and §5.1 passes
an aggregate parameter as its scalar leaves — so calling one requires knowing the
capture layout, which is precisely what `Ty::Fn` does not record and what a call
site holding `{ code, env }` therefore cannot know. `code` instead points at a
two-line function

```
thunk(env: *const Env, args...) -> R = f(load-leaves(env), args...)
```

whose first parameter is the environment *pointer*. A capture-free lambda gets
one too, ignoring the pointer: which of the two shapes a closure value holds is a
run-time fact, so both must be call-compatible. The rejected alternative —
`code` is the lambda and the caller spreads the environment — needs the caller to
know the capture layout, and there is no type through which to tell it.

**The environment block leads with its own drop glue.**

```
env - 16   the ordinary 16-byte heap header (§2)
env +  0   u64  drop_glue   the release function for this environment's record
env +  8   ...  the captured locals, at their record layout
```

`Ty::Fn` says what a function takes and answers, and nothing about what it
captured, so `decref` of a closure has no type from which to derive the function
that releases the environment's contents. One universal glue reads word 0 and
calls it on `env + 8`; the per-type release function is generated the same way
every other drop glue is, from `middle::layout`. Eight bytes per closure, against
a closure whose captures could not be freed. The rejected alternative — a glue
pointer beside `code` in the closure value — costs the same eight bytes in a
value that is copied far more often than the block is allocated.

## 8. Contexts cost nothing

A context is "an array of implementations, in binding order" on JavaScript
(`runtime.js:29`). Natively it is usually **nothing at all**.

Monomorphization resolves every effect call to a direct call: `resolve_trait_call`
reads the implementation type out of the context type's layout and dispatches on
it statically (`monomorphize.rs:709-741`), and `Program::ctx_layouts` records the
exact `Vec<TraitId>` per context type (`monomorphize.rs:186-187`). So by the time
anything is laid out, a `CtxGet` has a statically known answer and the only
question is whether the *implementation value* carries data.

Every implementation `core/host` exports is a zero-sized struct — `struct HostFs {}`,
`struct HostStdout {}`, ten of them (`host.buri:18-75`). A context of zero-sized
values is zero-sized. So in a program built on `core/host`, **`ctx` is not a
parameter**: it is dropped from every signature in the program by the layout pass,
which drops zero-sized parameters everywhere.

That is the effect system paying for itself at the machine level, and it is worth
stating plainly: the single largest ergonomic tax in the language — threading
`ctx` through every allocating function — has zero runtime cost on a native
backend. `list.map(ctx, f)` is `map(xs, f)`.

Where an implementation is not zero-sized — a test fake holding recorded output,
an attenuation wrapper (SPEC 10.8) that stores a prefix — the context becomes a
struct of exactly those non-empty implementations, laid out by §5, and passed by
§5.1. So the cost is proportional to the state a capability actually holds, which
is zero for the platform's own.

A mixed context keeps **one offset per binding**, in binding order, including the
ones that occupy nothing: a zero-sized binding gets the offset of whatever
follows it, which costs no bytes and means a `CtxGet` indexes by the binding
number it already has rather than by a number that has had the empty
implementations subtracted from it. `Layout::is_zero_sized` on the whole context
is then the one predicate that drops it from a signature, and it is the same
predicate that drops a `()` parameter — dropping zero-sized parameters is one
rule, not a special case for contexts.

## 9. Descriptors and derives: generated, not walked

The JS backend has it both ways. `derive Eq` is compiled per type into its own
function (`generate.rs:221-232`: "compiled at the type, a two-field struct is
`a[0]===b[0]&&a[1]===b[1]` — no dispatch left at all"), while `Show`, `Hash`,
`ToJson` and `FromJson` go through a runtime walker over a `Desc` value
(`monomorphize.rs:820-841`). The walker is the right call
there: it keeps one `$show` in the artifact instead of one per type, and artifact
size is what a JavaScript build is judged on.

**Natively, all of them are generated and no descriptor reaches the artifact.**

The reasons are the ones `design/LLVM-tips.md` lists. A descriptor walk is an
interpreter: an indirect dispatch on `Desc`'s tag per field per element, which is
the single megamorphic call site `generate.rs:222-227` already identifies as the
problem in the JavaScript version. It defeats `readnone`/`readonly` attribution
(CODEGEN-LLVM.md §3) because the walker reads a global table. It defeats DCE,
because everything reachable from any descriptor is reachable. And it costs code
size in the one place code size does not matter — a native binary that is 40 KB
larger and does not interpret its own type table is the better artifact.

So `Desc` stays in the middle end as the **fold input** it already is — the
recursion-terminating `Desc::Reserved` slot (`monomorphize.rs:128-131`) is
exactly what a code generator needs to emit a recursive type's `show` without
looping — and `middle::derives` emits one function per (trait, type) pair. Two
consequences:

- `Func::desc` (`monomorphize.rs:52`) is consumed at codegen time and never
  becomes data. The test runner's `report`, which is the one thing that needs a
  descriptor at run time (`monomorphize.rs:49-52`), gets a generated `show` for
  the type instead.
- `json.decode` (`monomorphize.rs:408-415`), which takes its type from an
  annotation and is handed a descriptor rather than a value, becomes a generated
  `decode_T` selected the same way.

The generated functions are `internal` in release, so a `derive Show` on a type
nothing prints is deleted.

## 10. The FFI boundary

203 functions in `runtime.js` (`grep -c 'function \$'`), 24 of them `$host_*`.
Reimplementing that surface twice — once in Cranelift lowering, once in LLVM
lowering — is not a plan.

**`cli/runtime` is a Rust static library with a C ABI, built for the host by
`cli/build.rs` and embedded with `include_bytes!`.** Every intrinsic key becomes
one symbol: `list.map` -> `buri_rt_list_map`, `host.HostFs.readFile` ->
`buri_rt_host_fs_read_file`. Both backends emit an ordinary call; neither knows
what is behind it.

**One prefix, `buri_rt_`, with no exceptions**, including the host capabilities
and including `buri_rt_abort`. The alternative that was nearly taken — `buri_rt_`
for the memory and abort surface and a bare `buri_host_` for the capabilities —
makes "is this symbol the runtime's" a table lookup instead of a string
comparison, in a compiler that has to answer that question at every call site it
emits. `libburi_rt.a` was already named for the prefix.

The ABI itself — how an aggregate parameter is flattened, how an aggregate
result leaves, how a `Result` reports which arm it took — is stated once in
`cli/runtime/lib.rs`'s module comment, which is the contract both backends cite.
It is written there rather than here because it is the file that has to be right,
and a contract two files away from its implementation is a contract that drifts.

The alternatives and why not:

- **Open-code everything in both backends.** 203 operations times two backends.
  The two would drift, and the drift would be found by users.
- **Write the runtime in Buri.** Circular: `str.concat` needs an allocator, the
  allocator needs `mmap`, and the language has no way to say `mmap`.
- **Call libc directly from generated code.** Works for `write` and `mmap` and
  nothing else. UTF-8 scalar counting, `sortBy`, JSON parsing, and SHA-256 are
  not in libc.
- **Ship the runtime as C.** Then the toolchain needs a C compiler at *build*
  time, which is a heavier dependency than the Rust one it already has.

The boundary is narrow by construction: everything above the intrinsics is
generated Buri, and `check_intrinsics` (`generate.rs:2669`) becomes
`Backend::missing_intrinsics` (ARCHITECTURE.md §3), so a runtime that is missing
`str.splitAny` is a build error naming it, per backend, exactly as the roadmap
asks.

The hot three — `incref`, `decref`, `alloc` fast path — are **not** calls. They
are open-coded by both backends, because a call per reference-count operation is
the thing that makes reference counting slow. MEMORY.md §5 gives the sequences.

## 11. The SPEC amendment

`cli/src/docs/SPEC.md:910-916` and `cli/src/docs/SPEC.md:1001-1006` are written as though JavaScript were
the only backend. They are the amendment.

**Applied in wave 3c**, to the sources rather than to `cli/src/docs/SPEC.md` — the document is
assembled from `cli/src/docs/lang/expressions.md` by `buri docs assemble`, and
editing the output would be edited back out by the next run. Three consequential
notes on the landing:

- The amendment **does not make the backends agree**, and it is not a plan to.
  It says what each does and declines to promise either, which is what makes
  §12's divergence list a list rather than a bug queue. In particular the
  JavaScript backend still has the 2^53 ceiling: `BigInt` everywhere is the only
  fix and it taxes every loop counter in every program (SPEC §13, open question
  8, has the trade). `design/TODO.md`'s native-backend section carries that as the
  follow-up.
- Two documents outside §6.2 said the same thing unconditionally and were
  amended with it: `docs/build/proto.md`'s 64-bit caveat, which is now the
  JavaScript backend's rather than the language's, and `core/num`'s own module
  comment, which the roadmap named as the file that would have to change.
- §11.4's float-rendering promise is now in the SPEC and is **not yet checked**.
  It is a promise about digits — `1.0 / 3.0` prints the same characters on every
  backend — and the test that holds it is §12's, which does not exist. A promise
  in a specification with no test behind it is a claim; this one is written down
  as such rather than assumed.

### 11.1 §6.2, replacing the paragraph at cli/src/docs/SPEC.md:910-916

> Undefined does not mean unbounded in practice, and what it means in practice
> depends on the backend.
>
> On a **native** backend every integer type is its own width and integer
> arithmetic is two's complement, so the observable consequence of overflow is a
> wrapped value. On the **JavaScript** backend every integer type compiles to a
> `number`, which represents every integer up to 2^53 - 1 exactly and no integer
> above it, so the observable consequence is lost precision. Neither is promised
> and neither is a definition — a program that overflows is wrong, and these are
> descriptions of two implementations rather than a specification of one.
>
> That the two differ is the reason overflow is undefined rather than
> implementation-defined: a language that pinned one of them would be pinning a
> backend. Code that needs an exact answer above 2^53 has two ways to ask for one
> that are defined on every backend: the `Checked` methods, which answer `.None`
> rather than a value they cannot hold, and `core/bits`, which computes on the
> bit pattern.

### 11.2 §6.2.1, adding after cli/src/docs/SPEC.md:946-950

> `I64 → F64` is lossy above 2^53 on every backend, and `toF64` rounds. On the
> JavaScript backend the *source* is a double already, so the conversion is the
> identity and the loss happened earlier; the answer is the same either way.

### 11.3 §6.2.2, replacing the paragraph at cli/src/docs/SPEC.md:1001-1006

**Shipped.** This is the text in `cli/src/docs/lang/expressions.md` and in
`cli/src/docs/SPEC.md` §6.2.2, after the ruling recorded in `OPEN-QUESTIONS.md`.

> A `Checked` method answers `.None` whenever it cannot hand back the true result
> — outside the type's range, or above what the backend represents exactly. The
> second bound is the backend's, and it is the numbers that backend has. On a
> native backend there is no second bound: `.None` means "outside the type's
> range" and nothing else, so `checkedAdd` on `I64` reports two's-complement
> overflow and nothing more. On the JavaScript backend the second bound is
> 2^53 - 1, well below `maxValue<I64>()`, because past it a `number` can no
> longer say which integer it is.
>
> So `(1 << 60).checkedAdd(1)` is `.Some` natively and `.None` on JavaScript, and
> both are correct, because both are the same promise kept over different
> numbers: `.Some(v)` means `v` is the exact true result as that backend
> represents numbers, and `.None` means that backend will not name a value it
> cannot hold.
>
> A program whose behaviour depends on which of those it gets is a program
> relying on a `Checked` method to *fail*, which is not what the trait is for.

The alternative — a native backend that also stops at 2^53 — was implemented,
shipped for a wave, and reversed: it makes `Checked` useless on `I64` natively,
which is exactly where a program reaches for it, and it buys portability of a
result nobody should be branching on. The cost of the ruling is one row on the
divergence list, and that row is pinned in both directions.

### 11.4 §6.2, adding after cli/src/docs/SPEC.md:918-919

> Floating point follows IEEE-754 on every backend, and the rendering of a float
> is the shortest decimal that round-trips. That is a promise about digits, not
> only about values: `1.0 / 3.0` prints the same characters on every backend.

## 12. JavaScript ↔ native deltas, and what pins them

One test file, `cli/tests/native/agreement.rs`, runs a corpus under both
backends and compares. Every row below is either "must agree" or is on that
file's explicit divergence list — a divergence with no entry is a bug.

**Written in wave 4b, and it did not agree.** The table had been a claim: nothing
compiled one program through both pipelines and compared the bytes. Four of the
fourteen rows were wrong, and the last column is now a test name rather than an
intention — `every_row_of_the_table_names_a_test_that_exists` reads this table
and fails if a row names a test that is not there, so the column cannot rot.

| # | Behaviour | JavaScript | Native | Verdict | Pinned by |
|---|---|---|---|---|---|
| 1 | `Int` overflow | precision loss above 2^53 | two's-complement wrap | Undefined on both (§11.1). **Divergence, listed.** | `row_01_int_overflow`, `row_01_integer_show_at_the_64_bit_extremes` |
| 2 | `checkedAdd` above 2^53, within `I64` | `.None` | `.Some` | **Divergence, listed** — and settled by a ruling, after a wave in which it was not. `Checked` is bounded by the numbers the *backend* has: `exact_int_range` on JavaScript (`js/intrinsics.rs`), `int_range` natively (`cranelift/emit.rs`'s `checked`, `llvm/emit.rs`'s `checked`, `buri_rt_i128_checked` at 128). Both are the same promise — `.Some(v)` is the exact true result as that backend represents numbers — kept over different numbers, and §11.3 is the SPEC text. The band between the two bounds is the divergence and it lives *only* here: it came out of `conformance/lib/numbers/test/integers.buri`, which `native/conformance.rs` runs natively and which therefore may only assert what both backends answer. `Saturating` was never bounded this way and is unaffected. | `row_02_checked_above_the_exact_range`, `row_02_saturating_is_bounded_by_the_type_on_both_backends` |
| 3 | `wrappingMul` at 64 bits | exact while the *intermediate* stays inside 2^53 | exact, native | Must agree — and **the row as written was false**. `$wrapTo` wrapped a product that had already been rounded, so `U32.wrappingMul(0xffffffff, 0xffffffff)` answered 0 rather than 1: a wrong answer at 32 bits, where both operands and the answer are exact doubles, not a 2^53 ceiling. `$wrapOp` computes in `BigInt` wherever the type's *whole range* is exact, which is every width up to 32 bits. At 64 and 128 it deliberately does not: the operands there may already be rounded — `maxValue<U64>()` is 2^64 — so exactness downstream is not a repair, and the compensating rounding is what makes `maxValue<U64>().wrappingAdd(1) == 0` on that backend, which the conformance corpus pins. So the true statement of this row is: **agrees at every width up to 32 bits, and above that agrees wherever the intermediate stays inside 2^53**. `(2^62 + 1024).wrappingMul(4)` is 4096 natively and 0 on JavaScript, and that case is row 1. Row 2's ruling does not touch this row: natively `wrapping*` **is** `iadd`/`isub`/`imul` (`cranelift/emit.rs`, and `llvm/emit.rs` where `wrappingAdd` and `add` are the same instruction because §3.4 emits no `nsw`/`nuw`), so it was exact at the type's width before the ruling and after it. | `row_03_wrapping_arithmetic_agrees`, `row_03_wrapping_at_narrow_widths_agrees`, `row_03_wrapping_at_the_type_boundaries_agrees` |
| 4 | `I128`/`U128` arithmetic | inexact above 2^53 | exact | **Divergence, listed.** The native answer is the correct one. | `row_04_wide_integer_arithmetic`, `row_04_integer_show_at_the_128_bit_extremes` |
| 5 | `Option<Option<T>>` | distinct, via `$some`/`$val`'s `$n` counter | distinct (§6) | ~~Divergence~~ — **must agree, and does.** `.Some(.None)` collided with `.None` before the depth counter landed; it does not now, at any nesting depth, through `match`, `Eq` or `Show`. | `row_05_nested_option_is_distinct` |
| 6 | `str.len()` | scalar count | scalar count | Must agree, including on astral input. | `row_06_str_len_counts_scalars` |
| 7 | `str.slice` past the end | clamps (`runtime.js:709-712`) | clamps | Must agree. Pinned on the boundary cases. | `row_07_str_slice_clamps` |
| 8 | Float rendering | JS `Number#toString` | shortest round-trip (§11.4) | Must agree, character for character. The runtime implements Ryū rather than trusting a libc `printf`. The corpus is `native/float_parity.rs`'s 3.8 million doubles; the row here is the end-to-end variant. | `row_08_float_rendering` |
| 9 | `derive Show` output | runtime walker | generated (§9) | Must agree, character for character, including field order and separators. A `[T]` field is a **named gap** — `deriveArrayShow` has no native body — and `Eq`, `Ord` and `Hash` ride along here because they are the same generator. | `row_09_derived_show`, `row_09_integer_show_at_every_width`, `row_09_bool_char_and_str_show`, `row_09_a_match_over_a_literal_and_an_interpolation`, `row_09_derived_eq_and_ord_verdicts`, `row_09_derived_hash_values`, `row_09_derived_show_of_a_list_is_a_gap`, `row_09_derived_show_of_a_list` |
| 10 | `derive ToJson` output | runtime walker | **absent** | Must agree, byte for byte. It is a wire format. Not reachable yet: `derivePrimJson` has no native body at any primitive, so the row is a named gap with the agreement test beside it, `#[ignore]`d. | `row_10_derived_tojson_is_a_gap`, `row_10_derived_tojson` |
| 11 | Division by zero | aborts (`runtime.js:44-47`) | aborts | Must agree, including the message. The *whole* stream does not: JavaScript writes `e.stack` after the message because there is a stack to write, so what is compared is the first line and the status. | `row_11_division_by_zero` |
| 12 | `Alloc` accounting | not implemented | not implemented | Must agree once both exist, which is why the model is *defined* rather than measured. A named gap on both sides today: `host.HostAlloc.allocate` has no native body, and the JavaScript one hands the byte count back without accumulating anything. | `row_12_alloc_accounting_is_a_gap`, `row_12_alloc_accounting` |
| 13 | Tail calls in constant stack | rewritten to a loop | rewritten to a loop | Must agree. **A merged group's forwarders were labelled `()` until wave 4b**, so a mutually recursive `Bool` came back as nothing: natively `even(3)` printed the empty string, and used as a condition it panicked inside Cranelift. | `row_13_tail_calls_run_in_constant_stack` |
| 14 | Abort message and exit status | stderr, exit 1 (`generate.rs:302-308`) | stderr, exit 1 | Must agree. The `.Err` return is the one failure whose whole stream agrees, because nothing was thrown. | `row_14_shift_out_of_range`, `row_14_an_error_return` |

Rows 8, 9 and 10 are the ones that actually cost work, and they are the ones
worth the cost: a `Show` that differs between backends means every golden test in
every repository is backend-specific, and the toolchain would have two sets of
expected output forever.

Two things wave 4b found that are not rows, recorded because the next reader will
otherwise find them again. `middle/lower.rs` interned `Str` and `Template` as two
types, so a `match` whose arms are a string literal and an interpolation — the
shape of every function that returns a message — did not verify natively at all,
on a program the JavaScript backend compiles and runs; §3.3 says the two *are*
one type and the interner now says so too. And `cli/tests/crash/` cannot be run
through this file as it stands, because every case there makes its divisor opaque
with `env.args(ctx).len()` and `host.HostEnv.arguments` has no native body; the
rows here use `"".len()` instead, which is opaque to the folder and reaches no
capability.
