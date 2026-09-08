# The native value model

`runtime.js` documents the JavaScript one: every integer is a double, a struct is
an array, an enum is a number or `[tag, ...payload]`, `None` is `undefined`. This
document is the other one — sized integers, a struct layout, a tagged union — and
the language-visible consequences of the change.

`middle::layout` computes the model (ARCHITECTURE.md §2.2) and both native
backends read the same table. A layout the debug and release backends disagreed
about would be a miscompile visible only when comparing profiles, so there is one
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
| `Int` | `i64` | `Int` is `I64` (`runtime.js`), and now it is — a `BigInt` on JavaScript, so it holds its whole range there too |
| `Template` | `Str` | see §3.3 |

`Int = I64` for real. The consequence is the whole of §7.

`Char` as `u32` rather than as a small string is not a choice — `Char` is one
Unicode scalar (`character.toU32()` is exact per SPEC 6.2.1). The JS backend spells it
as a one-scalar string because JavaScript has no character type, so `Char`
comparison there is string comparison and natively it is an integer comparison.
Those are the same answer only because the JavaScript side moved: `<` orders
UTF-16 code units, which puts every astral scalar *below* U+E000..U+FFFF, so
`$str_compare` spells the scalar order out and `$cmp` routes text through it.
§12 row 17.

`I128`/`U128` are the one place a backend can fall short of the type system, and
CODEGEN-STENCIL.md §5.3 states the fallback rather than this document, because it
is a backend limitation and not a model decision. The model says 128-bit
arithmetic is exact; a backend that cannot do it in registers calls the runtime.

Every scalar is aligned to its own width, including the 128-bit pair: `i128` is
**16-aligned**, not 8-aligned. That is what LLVM, clang and the SysV ABI all mean
by `i128`, and §10's boundary is why it is not a free choice — `cli/runtime` is
Rust with `#[repr(C)]` types, so a layout pass that aligned `i128` to 8 would
disagree with the runtime at the one place no test of either side alone catches.

## 2. Heap values, and the one header

Every heap allocation has a **16-byte header immediately before the payload**:

```
  ptr - 16   u64  rc     reference count, or IMMORTAL
  ptr -  8   u64  cap    bit 63: shared — every block of a program that can
                                 reach a task boundary, and no block of one
                                 that cannot (§2.1)
                         bit 62: arena — served out of a `core/alloc::scoped`
                                 arena, so not the platform allocator's to
                                 give back (§2.2)
                         bits 0..61: usable payload bytes
  ptr        ...  payload
```

Sixteen bytes and not eight, for three reasons that each independently decide it.
The payload stays 16-byte aligned, which every SIMD type and every `F64x2` in
`core/simd` wants for free; `cap` is what the free path needs to return a block to
the right size class; and `cap` is what MEMORY.md §5's in-place reuse tests
against. One header shape for every heap value, so `incref` and `decref` are two
instruction sequences in the whole program rather than one per kind.

`rc == u64::MAX` is `IMMORTAL`: a value that is never counted and never freed.
Every string literal, every constant aggregate the middle end interns, and every
zero-sized value has it. `incref` is a saturating add, so it is branchless;
`decref` tests for it, which is one well-predicted compare. MEMORY.md §5 has the
sequences.

### 2.1 Bit 63 of `cap` is the multi-threaded mark

`cap` holds the usable payload bytes in its **low 63 bits**. **Bit 63 is the
mark**: set means *this block may be reached from more than one thread*. It is
the bit `incref` and `decref` branch on to choose an atomic count, and the bit
`buri_rt_unique_cap` refuses an in-place write on.

Every reader masks. `middle::layout::CAP_SHARED_FLAG` and `CAP_MASK` are the
compiler's copy of the number; `cli/runtime/memory.rs`'s `BURI_RT_CAP_SHARED` and
`BURI_RT_CAP_MASK` are the runtime's, spelled twice for the reason `BURI_OK` is.

**Which blocks carry it: all of a program's, or none.**
`middle::rc::crosses_tasks` asks the whole post-monomorphization program whether
it can reach a task boundary. Both native backends turn a `true` into one call in
`main`, `buri_rt_values_may_cross_tasks()`, before anything allocates, and
`memory.rs::finish` then ORs the bit into every `cap` it writes. A program with no
`core/tasks` in it is bit for bit the program it was before track G. MEMORY.md
§5.1 argues for answering per program: a per-value mark would have to be
transitive to be sound, and a shallow one is the silent aliasing §5.5 forbids.

**A program that takes the unshared arm pays two instructions** per reference
operation — a load of the word beside the count, on a cache line the operation
was going to touch, and a bit test. The *compiler* pays a median **+21 %** of
native release lowering, which is an amended budget on that row rather than a met
one. Both numbers are in `design/PERFORMANCE.md` §6.6.

**Why `cap` and not `rc`.** A bit of the count would cost both properties §2 gave
it. `IMMORTAL` is `u64::MAX` and `incref` is a *saturating* add exactly so the
sentinel is a fixed point with no branch; a tag bit in the same word makes that
add wrong. And MEMORY.md §5.3's licence for in-place reuse is the literal test
`rc == 1`, which a tagged count fails for a block that is genuinely unique, so
every append would copy. `cap` has neither problem: it is a byte count nothing
does arithmetic on without knowing it is one, it is read on cold paths, and a
capacity runs out of address space long before bit 63. §3.1 plays the same trick
with bit 63 of `Str::len`.

**The readers.** `cli/runtime/memory.rs` masks in one place, `cap_of`, which
`buri_rt_free`, `buri_rt_realloc`, `buri_rt_cap`, `buri_rt_unique_cap` and the
`make_immortal` accounting read through; `buri_rt_grown_capacity` masks its
`old_cap` argument, because doubling the flag would ask for the address space.
Both backends open-code a read and both mask it: the `[T]` element count
`cap / stride` in a release glue (`llvm/emit.rs::glue`,
`stencil/glue.rs::elems_glue`) and the LLVM `str.concat` in-place probe.
`buri_rt_realloc` *preserves* the bit across a move rather than clearing it, so
growing a block cannot silently un-share it, and the `str.concat` probe reads it
a second time for a different question — a marked block is never unique, so the
in-place arm is not taken on one (MEMORY.md §5.1).

### 2.2 Bit 62 of `cap` is the arena bit

Set means *this block was served out of a `core/alloc::scoped` arena* (MEMORY.md
§7.2.1). It is the **runtime's alone**: nothing a backend emits tests it, and
exactly two functions read it. `buri_rt_free` does the accounting and then
returns rather than calling `dealloc`, because the pages go back in one `munmap`
when the scope ends. `buri_rt_realloc` grows such a block by allocating a new one,
because a bump allocator cannot grow what it handed out.

`middle::layout` declares it anyway, as `CAP_ARENA_FLAG`, because `CAP_MASK` is
declared there and every reader of a `cap` word in emitted code masks with it.
Putting the bit where the mask is means an element count cannot pick it up and
the bit cannot be spent twice.

**Why a header bit and not a side table.** "Whose is this block" has to be
answered on the free path, and a word that is already loaded answers it for
nothing. A side table would put a lookup in front of every free in the process.
A capacity that reached 2^62 bytes would collide with it, which is four exabytes
in one value.

## 3. `Str`

```
struct Str { base: *Header, ptr: *const u8, len: u64 }     // 24 bytes
```

`base` is the **payload pointer** of the allocation the bytes live in — what
`buri_rt_alloc` returned, with the header at `base - 16` — and not a pointer to
the header itself, notwithstanding the `*Header` above. That is what makes
`incref(base)` and `decref(base)` the same sequences every other heap value uses.

UTF-8, immutable, and **sliceable** — the requirement that decides the shape.
`core/str`'s header says it: "`trim`, `slice`, and `splitOnce` are pure because it
is immutable and sliceable: they return views, not copies". A view's `ptr` is in
the middle of somebody else's allocation, so subtracting 16 from it does not find
the reference count.

`base` is null for a literal or a static, which are `IMMORTAL` anyway, so a
literal string is three immediate constants and touches no allocator.

### 3.1 `len` is scalars, and the top bit of `len` says how much that costs

`str.length()` is "the number of Unicode scalar values, not the number of UTF-8
bytes" (`str.buri`), so the byte length in the value and the number the language
reports are different numbers and one of them has to be computed.

The field holds the **byte** length in its low 63 bits — a view has to know where
it ends — and bit 63 is the **ASCII flag**. Set means every byte in the view is
below 0x80, so the scalar count equals the byte count and `len()` is a mask.
Clear means counting bytes with `(b & 0xC0) != 0x80` — a loop that vectorizes to
one compare and one popcount per 16 or 32 bytes.

This mirrors the JavaScript backend, whose `$str_length` is
`$wide(s) ? $chars(s).length : s.length` (`runtime.js`). The boundary is drawn in
a different place — JavaScript's fast path is "no astral characters", ours is "no
non-ASCII" — but no program's asymptotics change between backends on the input
that matters, which is ASCII.

Whichever runtime function built the string computes the flag once; the builders
already scan the bytes. Slicing an ASCII string yields an ASCII string, so the
flag survives `trim`, `slice` and `splitOnce` for free. Slicing a non-ASCII string
leaves the flag clear even where the slice happens to be ASCII: rescanning on
every slice would cost the thing slicing exists to avoid.

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
joins the parts (`runtime.js`), which is a middle-end rewrite of
`ExprKind::Template` into a `str.concat` chain. There is no `Template` value at
run time on either backend.

## 4. `[T]`

```
struct List { ptr: *const T, len: u64 }                    // 16 bytes
```

Elements are contiguous, at `layout(T).stride`. The header is at `ptr - 16`,
because unlike `Str` a list is **never a view**: every one of `slice`, `take`,
`drop`, `concat`, `push`, `reverse` and `filter` in `core/list` is bounded by
`Allocator` (`list.buri`), which is the language saying they allocate. So `ptr` is
always a payload start and 16 bytes suffice.

`len` is the element count, exactly. There is no ASCII-flag equivalent because
`list.length()` is the element count and always O(1) (`list.buri`).

### 4.1 A flat array, not a persistent vector

Rejected: a RRB tree, a chunked deque, or any structure with structural sharing
on append.

The stdlib's list surface is bulk producers — `map`, `filter`, `fold`, `range`,
`repeat`, `zip`, `flatten` — which build a whole array at once and read it
linearly, and `push` is `Allocator`-bounded, which is the language stating that it
copies. More decisively: `sum` (`list.buri`) and `core/simd` want a contiguous
`i64*`. A flat array is the only representation where a fold over `[Int]` compiles
to a vectorizable loop, and vectorizing folds is most of what a native backend is
for here.

What recovers append performance is uniqueness, not a different data structure:
`xs.push(ctx, x)` where `xs` has a reference count of 1 and spare capacity writes
in place and returns the same pointer. That is MEMORY.md §5, it is invisible in
the type, and it makes the loop `for … { xs = xs.push(ctx, x) }` amortized O(1)
without changing what `[T]` is. It is `cli/runtime/list.rs`'s `append_dest`,
behind `list.push` and `list.concat` on both backends, with the capacity coming
from doubling on the reallocating path. The native suite's
`a_unique_push_loop_allocates_logarithmically` states the amortization as an
allocation count. The one restriction is that an element type holding counted
references — `[Str]` — still copies; MEMORY.md §5.3 says why, and it is a
property of the drop glue rather than of `[T]`.

### 4.2 `..rest` allocates, and the arm owns what it binds

**Ruling.** `[head, ..rest]` binds a **fresh block**, not an interior view of the
scrutinee. It is §4's "never a view" read at the one place pattern matching
rather than a `core/list` function produces a slice: the native slice calls
`buri_rt_list_new`, `memcpy`s the tail and retains every counted element. The
header at `ptr - 16` is what forces it — an interior pointer would make the next
release read a header that is not one.

The consequence is a rule for `middle/rc.rs`, which had the other answer by
default. Every *other* binding a pattern makes is a projection: it points into
the scrutinee, owns nothing, and takes a count only where a consuming match is
about to drop what it points into. A rest binding is the exception in both
directions — **the arm owns it however the scrutinee is held, and it takes no
count out of the scrutinee** — so `Pattern::fresh_binds` names exactly these
locals and `Scan::match_` puts the drop on them unconditionally.

Getting this backwards would be worse than the leak it replaces: marking the
binding owned where the slice *aliased* would free the scrutinee's block at the
arm's last use, which is a double free rather than a missed one. Measured on
`scratchpad/frcheck/cmd/r1` — `..rest` over `[Str]` and `[Int]` — 44 leaks on the
debug backend of the day and 46 on LLVM before, 0 on both after, answers
unchanged.

## 5. Tuples and structs

Fields in **declaration order**, at natural alignment, C layout. Size rounded up
to alignment. Nothing is reordered.

Because a size is already rounded up to its own alignment, `stride` — what a `[T]`
indexes by — equals `size` for every type in this model. Both are in the layout
table anyway: they are different questions, and a model that spelled them with one
number would have to be re-read the day a packed representation made them
differ.

Reordering to close padding is not taken in v1, because `Desc::Struct`
(`monomorphize.rs`) carries `fields` in declaration order and every derived
operation is a fold over that order — `derive Show` prints them in it, `derive
ToJson` writes a positional struct as an array in it, and that array's element
order is *wire format*. A layout pass that reorders and a descriptor that does
not is two orderings somebody has to keep in step by hand, and the failure is a
JSON document with its fields transposed. The growth path is one field on
`DescField` — the byte offset — after which reordering is safe and mechanical.
The padding cost is small: this language has no `u24`s and no bitfields, and the
common shapes (all-pointer, all-i64) have none.

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
SysV would classify a 24-byte struct as MEMORY and spill it. Since this compiler
generates both sides of every Buri call, there is no ABI to be compatible with.
The one place there is — `cli/runtime`'s C entry points — takes the platform ABI
and is written in Rust with `#[repr(C)]` types to match (§9).

Both native backends are told this as a signature with N scalar parameters.
Neither needs `byval`, `sret`, or a struct type in a signature anywhere.

### 5.2 Where a recursive type's indirection goes

§4 covers most of it: `enum Rose { Node([Rose]) }` needs no indirection, because
`[T]` already is one and laying out a list never asks for its element's layout,
and a closure's layout is two pointers whatever it closes over.

`enum Tree<T> { Leaf, Node(Tree<T>, T, Tree<T>) }` is the rest. It is legal, it is
in SPEC 5.4's own example list, and it is annotated there as "boxed by the
runtime" — a promise the layout pass has to keep, because a `Tree` stored inline
in a `Tree` has no finite size. So:

> A field is stored **behind a pointer** exactly when its owner's type
> constructor is in a recursion group that is a genuine cycle, and the field's
> type mentions a constructor of that group in a position that would be stored
> inline. Recursion groups are the strongly connected components of "mentions
> inline", where a generic argument counts and `[T]` and `fn(..) => T` do not.

Three things follow, and each is why the rule is stated over groups rather than
over back edges found while walking:

- **The answer does not depend on the order layouts were asked for.** In a cycle
  `A -> B -> A`, *both* edges are boxed. A rule that boxed the edge it happened
  to close on would give `A` one layout when `A` was asked for first and another
  when `B` was — a miscompile that reproduces only under one build order.
- **A constructor inside itself at smaller arguments is not a cycle.**
  `Option<Option<T>>` and `Pair<Pair<Int>>` share a constructor with their own
  payload, but `Option`'s declared payload is its parameter, so `Option` mentions
  nothing and its group is a group of one with no self-edge. Nothing is boxed and
  §6's tagged `Option<Option<T>>` holds its payload inline.
- **A pointer introduced this way is never null**, so it is a niche candidate
  (§6), which is what makes `Option<a box-shaped struct>` free.

Boxing both edges of a two-cycle costs an indirection per level in a shape nothing
in the standard library has — `core/json`'s `Json` recurses through `[Json]` and
is not boxed at all. The growth path is to pick a canonical edge per group by
declaration order, which is a change to one predicate.

## 6. Enums

`tag ++ payload`, where the tag is the smallest of `i8`/`i16`/`i32` that holds
the variant count, the payload area is the union of the variants' field layouts
at the maximum alignment among them, and the whole is a struct at that alignment.
A variant's fields are laid out inside the payload area in declaration order,
independently per variant.

The tag is at **offset 0** and its value is the variant's **index in declaration
order**, which is the number `derive Ordered` compares and the number a decision tree
switches on. An enum with no variants is uninhabited, has no value, and occupies
nothing.

Two niches, both on day one, both because the IR already assumes them:

- **An enum whose payload area is empty is a bare integer.** `Desc::payloadless`
  already means exactly this (`monomorphize.rs`), and the JS backend already
  compiles equality on one to `a === b` (`generate.rs`). Stated in bytes rather
  than in fields, so that `Option<()>` — one variant with a zero-sized field — is
  a byte too.
- **`Option<T>` where `T`'s layout has a pointer field with a known-nonnull
  invariant is the pointer, with null for `.None`.** `Option` already has no tag
  in the IR: `Desc::Option(inner)` says only what the payload is, because "`None`
  is `undefined` and `Some(x)` is `x`" (`monomorphize.rs`, `runtime.js`). The
  niche keeps that true natively for the case that matters — `Option<Str>`,
  `Option<Box-shaped struct>` — at zero cost.

"A pointer field with a known-nonnull invariant" is a short list, and half of the
pointers in this model *are* nullable, so picking one of those would be a silent
miscompile rather than a missed optimization:

| Shape | The niche | Why not the other one |
|---|---|---|
| `Str` | `ptr`, at offset 8 | `base` is null for a literal (§3) |
| a closure | `code`, at offset 0 | `env` is null when nothing was captured (§7) |
| a struct or tuple | the first such pointer inside it, by offset | — |
| a field boxed by §5.2 | that pointer | — |
| an enum | none | which pointers exist depends on the tag |

`[T]` used to be on that list, on the reasoning that a list is never a view so
its `ptr` is always a payload start — and the empty list refuted it: both backends
make an empty list's `ptr` null, so `.Some(list.empty())` *was* `.None`.
`Ty::Array` is not a niche candidate (`middle/layout.rs`), and `Option<[T]>`
carries a tag.

`.None` is that one word set to null; nothing writes or reads the rest of the
value. So `Option<Str>` is 24 bytes, exactly a `Str`, and testing it is one
compare against zero.

Everything else gets a tag, **`Option<Option<T>>` included** — a semantic
improvement over JavaScript rather than a cost, since `runtime.js` records that
`Some(None)` and `None` collide there and that the collision is why `Option<T>` in
JSON does not round-trip.

General niche discovery — scanning a type for any unused bit pattern, Rust-style
— is deferred: a large amount of machinery for a language whose enums are mostly
`Option` and `Result`, and `Result<T, E>` gets no niche from it anyway because
both arms carry payloads.

## 7. Closures

```
struct Closure { code: *const fn, env: *const Environment }        // 16 bytes
```

`middle::closures` (ARCHITECTURE.md §2.2) lifts every lambda to a top-level
function taking `env` as an extra first parameter, and builds `Environment` as an
ordinary struct of the captured locals — which `ExprKind::Lambda { captures }`
already lists (`typed.rs`).

A lambda that captures nothing has a null `env`, and the middle end rewrites a
call through a known-empty closure into a direct call, so `xs.map(ctx, double)`
costs no indirection at all. `ExprKind::FnRef` is exactly that case.

The environment is plain immutable data with no capability in it, because SPEC
10.6 forbids capturing an effect-carrying value and the checker enforces it. That
makes it an ordinary reference-counted record with an ordinary generated `drop`,
and makes MEMORY.md §2's acyclicity argument go through.

### 7.1 What `code` and `env` actually point at

Two additions §7 does not follow from, both forced by §5.1 and both identical in
every native backend that has emitted a closure.

**`code` is always a generated thunk, never the lifted lambda.** The lifted
lambda takes its environment as an *aggregate* first parameter, and §5.1 passes an
aggregate parameter as its scalar leaves — so calling one requires knowing the
capture layout, which is precisely what `Ty::Fn` does not record and what a call
site holding `{ code, env }` therefore cannot know. `code` instead points at a
two-line function

```
thunk(env: *const Environment, args...) -> R = f(load-leaves(env), args...)
```

whose first parameter is the environment *pointer*. A capture-free lambda gets one
too, ignoring the pointer: which of the two shapes a closure value holds is a
run-time fact, so both must be call-compatible. The rejected alternative — `code`
is the lambda and the caller spreads the environment — needs the caller to know
the capture layout, and there is no type through which to tell it.

**The environment block leads with its own drop glue.**

```
env - 16   the ordinary 16-byte heap header (§2)
env +  0   u64  drop_glue   the release function for this environment's record
env +  8   u64  copy_glue   the deep-copy function for the same record
env + 16   ...  the captured locals, at their record layout
```

`Ty::Fn` says what a function takes and answers, and nothing about what it
captured, so **neither** of the two operations a generic path performs on an
environment can be derived from the type at the site that performs it. `decref`
of a closure has no type from which to derive the release; `core/alloc::copyOut`
has none from which to derive the copy (MEMORY.md §7.2.1). One universal glue
reads word 0 and calls it on `env + 16`, a second reads word 1 and does the same,
and both per-type functions are generated from `middle::layout` the way every
other glue is. Sixteen bytes per closure, against a closure whose captures could
not be freed and could not leave a scope.

Rejected for the first word: a glue pointer beside `code` in the closure *value*,
which costs the same eight bytes in a value copied far more often than the block
is allocated. Rejected for the second: one word pointing at a static
`{ release, copy }` pair, which costs eight bytes per *type* but puts a second
load in front of every drop of every closure in the language.

## 8. Contexts cost nothing

A context is "an array of implementations, in binding order" on JavaScript
(`runtime.js`). Natively it is usually **nothing at all**.

Monomorphization resolves every effect call to a direct call and
`Program::ctx_layouts` records the exact `Vec<TraitId>` per context type
(`monomorphize.rs`), so by the time anything is laid out a `CtxGet` has a
statically known answer and the only question is whether the *implementation
value* carries data.

Every implementation `core/host` exports is a zero-sized struct — `struct HostFileSystem {}`,
`struct HostStdout {}`, fifteen of them (`host.buri`), of which any one platform
grants at most thirteen. A context of zero-sized values is zero-sized. So in a
program built on `core/host`, **`ctx` is not a parameter**: the layout pass drops
it from every signature, the way it drops zero-sized parameters everywhere. The
single largest ergonomic tax in the language — threading `ctx` through every
allocating function — has zero runtime cost on a native backend.
`list.map(ctx, f)` is `map(xs, f)`.

Where an implementation is not zero-sized — a test fake holding recorded output,
an attenuation wrapper (SPEC 10.8) that stores a prefix — the context becomes a
struct of exactly those non-empty implementations, laid out by §5 and passed by
§5.1, so the cost is proportional to the state a capability holds.

**At the archive boundary the rule is not about size at all.** A `buri_rt_*` call
drops its context argument *whatever it weighs*, because `cli/runtime` allocates
through `buri_rt_alloc` and reads no capability — so the C signature has no
parameter for one. Which argument that is is a fact about the **declaration**:
`list.push(self, ctx, item)` names its second, `list.repeat(ctx, item, times)`
its first. Both native backends read it off their runtime tables
(`backend/runtime_table.rs`'s `Entry::ctx`, `backend/llvm/runtime.rs`'s
`Arg::Dropped`).

Asking the *argument's type* instead — "is it a `Ty::Ctx`?" — is the same question
only while every `C: Allocator` is instantiated at a `context { … }` record, and it is
not: `<C: Allocator>` and `<T: Ordered>` are one feature (SPEC 10.1), and SPEC 10.8's
attenuation exists so that programs pass something that merely implements the
effect. Such a value spread to a leaf the C signature had no parameter for and
shifted every argument after it one register down — a fault in `memmove`.

A mixed context keeps **one offset per binding**, in binding order, including the
ones that occupy nothing: a zero-sized binding gets the offset of whatever follows
it, so a `CtxGet` indexes by the binding number it already has.
`Layout::is_zero_sized` on the whole context is the one predicate that drops it
from a signature, and it is the same one that drops a `()` parameter.

## 9. Descriptors and derives: generated, not walked

The JS backend has it both ways: `derive Equal` is compiled per type into its own
function (`generate.rs`), while `Show`, `Hash`, `ToJson` and `FromJson` go through
a runtime walker over a `Desc` value (`monomorphize.rs`). The walker is right
there — it keeps one `$show` in the artifact instead of one per type, and artifact
size is what a JavaScript build is judged on.

**Natively, all of them are generated and no descriptor reaches the artifact.**
The reasons are the ones CODEGEN-LLVM.md §0 lists. A descriptor walk is an
interpreter: an indirect dispatch on `Desc`'s tag per field per element, which is
the single megamorphic call site `generate.rs` already identifies as the problem
in the JavaScript version. It defeats `readnone`/`readonly` attribution
(CODEGEN-LLVM.md §3) because the walker reads a global table. It defeats DCE,
because everything reachable from any descriptor is reachable. And it costs code
size in the one place code size does not matter.

So `Desc` stays in the middle end as the **fold input** it already is — its
recursion-terminating `Desc::Reserved` slot (`monomorphize.rs`) is what a code
generator needs to emit a recursive type's `show` without looping — and
`middle::derives` emits one function per (trait, type) pair. Two consequences:

- Codegen consumes `Func::desc` (`monomorphize.rs`) and it never becomes data.
  The test runner's `report`, which is the one thing that needs a descriptor at
  run time, gets a generated `show` for the type instead.
- `json.decode`, which takes its type from an annotation and is handed a
  descriptor rather than a value, becomes a generated `decode_T` selected the
  same way.

The generated functions are `internal` in release, so a `derive Show` on a type
nothing prints is deleted.

## 10. The FFI boundary

203 functions in `runtime.js` (`grep -c 'function \$'`), 24 of them `$host_*`.
Reimplementing that surface once per native backend is not a plan.

**`cli/runtime` is a Rust static library with a C ABI, built for the host by
`cli/build.rs` and embedded with `include_bytes!`.** Every intrinsic key becomes
one symbol: `list.map` -> `buri_rt_list_map`, `host.HostFileSystem.readFile` ->
`buri_rt_host_fs_read_file`. Both backends emit an ordinary call; neither knows
what is behind it.

**One prefix, `buri_rt_`, with no exceptions**, including the host capabilities
and including `buri_rt_abort`. A split — `buri_rt_` for the memory and abort
surface and a bare `buri_host_` for the capabilities — makes "is this symbol the
runtime's" a table lookup instead of a string comparison, in a compiler that has
to answer that question at every call site it emits.

The ABI itself — how an aggregate parameter is flattened, how an aggregate result
leaves, how a `Result` reports which arm it took — is stated once in
`cli/runtime/lib.rs`'s module comment, which is the contract both backends cite.
A contract two files away from its implementation is a contract that drifts.

The alternatives and why not:

- **Open-code everything in both backends.** 203 operations times two backends.
  The two would drift, and users would find the drift.
- **Write the runtime in Buri.** Circular: `str.concat` needs an allocator, the
  allocator needs `mmap`, and the language has no way to say `mmap`.
- **Call libc directly from generated code.** Works for `write` and `mmap` and
  nothing else. UTF-8 scalar counting, `sortBy`, JSON parsing, and SHA-256 are not
  in libc.
- **Ship the runtime as C.** Then the toolchain needs a C compiler at *build*
  time, which is a heavier dependency than the Rust one it already has.

The boundary is narrow by construction: everything above the intrinsics is
generated Buri, and `check_intrinsics` (`generate.rs`) becomes
`Backend::missing_intrinsics` (ARCHITECTURE.md §3), so a runtime missing
`str.splitAny` is a build error naming it, per backend.

The hot three — `incref`, `decref`, `alloc` fast path — are **not** calls. Both
backends open-code them, because a call per reference-count operation is the thing
that makes reference counting slow. MEMORY.md §5 gives the sequences.

## 11. The SPEC amendment

SPEC §6.2 and §6.2.2 were written as though JavaScript were the only backend. The
amendment that fixed them shipped, so this document does not reproduce the text:
`buri docs language/expressions` serves it, and SPEC §6.2, §6.2.1 and §6.2.2 are
where it landed. It was written to the sources rather than to the assembled
`SPEC.md`, which `buri docs assemble` would have edited back out.

What is worth keeping is the reasoning, which is not in the specification and
should not be:

- The amendment **does not make the backends agree**, and it is not a plan to. It
  says what each does and declines to promise either, which is what makes §12's
  divergence list a list rather than a bug queue. The 2^53 ceiling it described is
  gone: `I64`, `U64`, `I128` and `U128` are `BigInt`s on JavaScript
  (buri-lang/buri#8, #4), at the cost §12's table measures.
- Two documents outside §6.2 were amended with it: `docs/build/proto.md`'s 64-bit
  caveat, now the JavaScript backend's rather than the language's, and `core/number`'s
  own module comment.
- **A native backend that also stopped at 2^53 shipped for a wave and was
  reversed.** It makes `Checked` useless on `I64` natively, which is exactly where
  a program reaches for it, and it buys portability of a result nobody should be
  branching on. The ruling is that a `Checked` method is bounded by the numbers
  the *backend* has, so `.None` natively means "outside the type's range" and
  nothing else.
- The float-rendering promise — that the rendering of a float is the shortest
  decimal that round-trips, so `1.0 / 3.0` prints the same characters on every
  backend — is in the SPEC and is a promise about digits rather than about values.
  §12 is what holds it.

## 12. JavaScript ↔ native deltas, and what pins them

One test file, `cli/tests/native/agreement.rs`, runs a corpus under both backends
and compares. Every row below is either "must agree" or is on that file's explicit
divergence list — a divergence with no entry is a bug. The last column is a test
name rather than an intention: `every_row_of_the_table_names_a_test_that_exists`
reads this table and fails if a row names a test that is not there.

| # | Behaviour | JavaScript | Native | Verdict | Pinned by |
|---|---|---|---|---|---|
| 1 | `Int` overflow | the exact sum, unbounded | two's-complement wrap | Undefined on both (SPEC §6.2). **Divergence, listed.** A `BigInt` has no width to overflow *at*, so `maxValue<I64>() + 1` is 9223372036854775808 here and −9223372036854775808 natively. A program that wants the defined answer says `wrappingAdd`, which agrees at every width (row 3). Wrapping every result back with `asIntN` would close the row and was not done: it is a call on every add in every program to make one undefined answer match another. | `row_01_int_overflow`, `row_01_integer_show_at_the_64_bit_extremes` |
| 2 | `checkedAdd` above 2^53, within `I64` | `.Some` | `.Some` | **Must agree, and does.** `Checked` is bounded by the numbers the *backend* has (SPEC §6.2.2), and a `BigInt` says which integer the answer is, so `exact_int_range` and `int_range` are the same range at every width. `Saturating` was never bounded this way. | `row_02_checked_above_the_exact_range`, `row_02_saturating_is_bounded_by_the_type_on_both_backends` |
| 3 | `wrappingMultiply` at 64 bits | exact | exact, native | Must agree, at every width. `$wrapOp` computes in `BigInt` wherever the operands are `number`s and the intermediate can leave 2^53, which is a product at 32 bits and nothing else; at 64 and 128 the operands are `BigInt`s and the wrap is one `asIntN`. Natively `wrapping*` **is** the machine's own add, subtract and multiply, because §3.4 emits no `nsw`/`nuw`. | `row_03_wrapping_arithmetic_agrees`, `row_03_wrapping_at_narrow_widths_agrees`, `row_03_wrapping_at_the_type_boundaries_agrees` |
| 4 | `I128`/`U128` arithmetic | exact | exact | **Must agree, and does.** Both are `BigInt`s (buri-lang/buri#4). `I128` is the escape hatch the language offers when 64 bits are not enough, and an escape hatch that rounds is not one. | `row_04_wide_integer_arithmetic`, `row_04_integer_show_at_the_128_bit_extremes` |
| 5 | `Option<Option<T>>` | distinct, via `$some`/`$val`'s `$n` counter | distinct (§6) | **Must agree, and does**, at any nesting depth, through `match`, `Equal` or `Show`. | `row_05_nested_option_is_distinct` |
| 6 | `str.length()` | scalar count | scalar count | Must agree, including on astral input. | `row_06_str_len_counts_scalars` |
| 7 | `str.slice` past the end | clamps (`runtime.js`) | clamps | Must agree. Pinned on the boundary cases. | `row_07_str_slice_clamps` |
| 8 | Float rendering | JS `Number#toString` | shortest round-trip (SPEC §6.2) | Must agree, character for character. The runtime implements Ryū rather than trusting a libc `printf`. The exhaustive corpus is `native/float_parity.rs`'s 3.8 million doubles; the row here is the end-to-end variant. | `row_08_float_rendering` |
| 9 | `derive Show` output | runtime walker | generated (§9) | Must agree, character for character, including field order and separators. A `[T]` field goes through `deriveArrayShow`, which calls the element's generated `show` once per element and joins the results in `buri_rt_show_list` — one body, because the brackets and the `, ` have to be the same bytes on both backends. `Equal`, `Ordered` and `Hash` ride along here because they are the same generator. | `row_09_derived_show`, `row_09_integer_show_at_every_width`, `row_09_bool_char_and_str_show`, `row_09_a_match_over_a_literal_and_an_interpolation`, `row_09_derived_eq_and_ord_verdicts`, `row_09_derived_hash_values`, `row_09_derived_show_of_a_list` |
| 10 | `derive ToJson` output | runtime walker | generated (§9) | Must agree, byte for byte. It is a wire format. The leaf (`stencil/emit.rs::json_prim`, `llvm/emit.rs::json_prim`) builds `Json`'s arm for a primitive — `Bool` to `.Bool`, `Str`/`Char` to `.Str`, every number to `.Num` — and the compound arms are `middle::derives`' own tree. The variant index is read off `core/json`'s declaration by name rather than hard-coded, and the `.Str` arm takes a count, because `middle::rc`'s contract is that an intrinsic borrows and this one's result keeps. `json.stringify` needs closures and is not reachable, so the row's program walks the tree by hand. | `row_10_derived_tojson` |
| 11 | Division by zero | aborts (`runtime.js`) | aborts | Must agree, including the message. The *whole* stream does not: JavaScript writes `e.stack` after the message, so what is compared is the first line and the status. | `row_11_division_by_zero` |
| 12 | `Allocator` accounting | `$host_HostAllocator_allocate` | `buri_rt_host_alloc_allocate` | Must agree, and does. The model is *defined* rather than measured (MEMORY.md §7.1), which is what makes agreement checkable: the charge is a function of the argument and the types, so `allocate(64)` is `Region(64)` on both. Nothing accumulates *in* `HostAllocator` on either side; the totals a program can read belong to `core/alloc`'s counters. | `row_12_alloc_accounting` |
| 13 | Tail calls in constant stack | rewritten to a loop | rewritten to a loop | Must agree. A merged group's forwarders were labelled `()` for a while, so a mutually recursive `Bool` came back as nothing. | `row_13_tail_calls_run_in_constant_stack` |
| 14 | Abort message and exit status | stderr, exit 1 (`generate.rs`) | stderr, exit 1 | Must agree. The `.Err` return is the one failure whose whole stream agrees, because nothing was thrown. | `row_14_shift_out_of_range`, `row_14_an_error_return` |
| 15 | `character.toUpper` / `toLower` where the full case mapping is not one scalar | `"SS"` — a `Char` of two scalars | `'S'` — the **first** scalar of the full mapping | **Divergence, listed**, and the JavaScript side is the one outside the type: `Char` is one Unicode scalar value (`character.buri`), and `"ß".toUpperCase()` is two characters. The *simple* case mapping (`'ß'` unchanged) was the tidier answer and disagrees with JavaScript at `toU32` as well, where the first scalar agrees. So the divergence is confined to **rendering the whole `Char`**, and every use that reads it as a scalar agrees. `cli/runtime/character.rs` §3. | `row_15_char_case_of_a_multi_scalar_mapping` |
| 16 | A NaN payload through `bytes.f64FromBytes` / `f64ToBytes` | canonicalized — moving a NaN through a `number` drops the payload | canonicalized | Must agree, and it did **not**: the payload survived natively, so one program computed different bytes on two backends across a round trip. SPEC §6.2 had already ruled every NaN `==` every other "regardless of sign or payload", so native was the accident and native moved. `cli/runtime/bytes.rs` canonicalizes on ingress, at four octets as well as eight, and both backends answer `[0, 0, 0, 0, 0, 0, 248, 127]`. Signed zero is untouched and pinned beside it. A program whose identity rule is raw IEEE-754 bits wants `[U8]`. | `row_16_nan_payloads_canonicalize_on_every_backend` |
| 17 | `Str` and `Char` ordering | UTF-16 code units, from `<` | Unicode scalar value, from a `memcmp` | Must agree, and it did **not**: the two orders part company exactly where one string has an astral scalar and the other a scalar in U+E000..U+FFFF, so `"\u{1F600}" < "\u{E000}"` was `true` on JavaScript and `false` natively. `buri_rt_str_compare` had hidden it by transcoding to UTF-16, buying parity at the price of an order agreeing with neither the unit `Str` indexes in nor Rust's `str::cmp`; `Char` never had even that. The order is now scalar value on both, and the native side is a plain byte comparison. buri-lang/buri#35. | `row_17_text_orders_by_scalar_value` |

**What the `BigInt` representation costs, measured.** Bun 1.3, macOS arm64,
release builds, the same sources through both toolchains, best of nine
alternating passes:

| | before | after |
|---|---|---|
| the conformance corpus, one process per package | 313 ms | 351 ms (+12%) |
| the same, one process for all of it | 110 ms | 144 ms (+31%) |
| counting to twenty million twice, at `Int` and at `I32` | 220 ms | 828 ms |
| the `I32` half of that program alone | 128 ms | 128 ms |
| the golden corpus, bytes emitted | 44 586 | 44 925 (+1%) |

Real code — the corpus is a thousand assertions over strings, lists, maps and
JSON — pays about a third of its own runtime, and about a tenth of what a person
waits for, because a JavaScript process spends more time starting than the corpus
spends running. A tight counted loop pays sevenfold: the `Int` half of the third
row went from about 90 ms to about 700 ms. The fourth row is the mitigation and
the reason the line is drawn at 32 bits rather than at 8: a `number` holds every
value of every width up to `I32`, so a loop counter that does not need 64 bits can
say `I32` and pay nothing at all.

Rows 8, 9 and 10 are the ones that cost work and the ones worth the cost: a `Show`
that differs between backends means every golden test in every repository is
backend-specific.

Two findings that are not rows. `middle/lower.rs` interned `Str` and `Template` as
two types, so a `match` whose arms are a string literal and an interpolation — the
shape of every function that returns a message — did not verify natively at all;
§3.3 says the two *are* one type and the interner now says so too. And
`cli/tests/crash/` cannot be run through this file as it stands, because every
case there makes its divisor opaque with `env.arguments(ctx).length()` and
`host.HostEnvironment.arguments` has no native body; the rows here use `"".length()` instead.

**A third, fixed by a ruling rather than by a fifteenth row.** A struct holding
`NaN` compared with **itself** used to answer `true` on JavaScript and `false` on
both native backends. The ruling of 2026-08-20 is that **`NaN == NaN` is true**,
everywhere and at every depth, so `==` is an equivalence relation. The ordering
operators stay on IEEE-754 — `NaN < NaN` is still false — so `<` and `compare` no
longer agree with `==` at `NaN`, which is the price paid for reflexivity. What it
changed:

- **The float leaf.** `==` and `!=` at `F32`/`F64` are `a == b || (isnan(a) &&
  isnan(b))` on all three backends: `fcmp Equal` / `bor` / two `fcmp Unordered`
  through the runtime boundary in the debug backend, `fcmp oeq` / `or` / two
  `fcmp uno` in `llvm/emit.rs`'s `float_equality`, and
  `a === b || (a !== a && b !== b)` in `js/generate.rs`'s `float_eq` (with `$feq`
  in `runtime.js` for operands that cannot be written twice). Not a bitwise
  compare, which would separate `-0.0` from `0.0` and two `NaN`s with different
  payloads.
- **Derived equality needed no change natively**: `middle/derives.rs` emits
  `PrimOp::Eq` at a float field, which lowers to the leaf above. On JavaScript
  `eq_kind` answered `Identity` — bare `===` — for every primitive, so the float
  field is now its own `EqKind::Float` and `runtime.js`'s `$eq` gained a line.
- **`Hash` was already right and is now load-bearing.** `buri_rt_hash_f64` and
  `$hashInto` both mix `ToUint32(Math.trunc(x) || 0)`, and `|| 0` catches every
  `NaN` regardless of payload, so equal values hash equally, which is what a `Map`
  key needs.

Row 9's reason for grouping `Equal` with `Show` — "they are the same generator" — is
false and is worth knowing: `derives.rs` runs from `middle::native` and nowhere
else, so derived equality has **two** implementations, and the only thing
comparing them is `agreement.rs`.

The referential fast path in `eq_decl` and in `$eq` — `if (a === b) return true;`
— stays, and is sound: an equivalence relation is reflexive, so two references to
one value are equal. SPEC 7.2 rejected referential equality as the *definition*,
which is untouched.
