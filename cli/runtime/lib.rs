//! # `libburi_rt` — the native runtime, and the `buri_rt_*` ABI contract
//!
//! This file is **the contract**. Both native backends (`backend/stencil`,
//! `backend/llvm`) emit calls into the symbols declared here, and neither of
//! them knows what is behind one. A disagreement between a backend and this
//! comment is a miscompile that only shows up as a wrong answer at run time, so
//! the rules are stated here once and cited from both.
//!
//! Design: `design/native/VALUE-MODEL.md` §10 (why a Rust staticlib),
//! `design/native/MEMORY.md` §5 (the counts and the allocator),
//! `design/native/BUILD-AND-WATCH.md` §2.2 (how it is built and embedded),
//! `design/native/CODEGEN-STENCIL.md` §5, §5.1 (who calls what).
//!
//! ## 0. What this crate is, and what it is not
//!
//! It is not the whole of the 203 `runtime.js` functions. It is what a
//! *generated program* cannot do for itself, which is now eight things rather
//! than three:
//!
//!   * **allocation** — the header, the block, the counts, the free path;
//!   * **aborts** — the messages, and the exit status;
//!   * **host capabilities** — the native counterpart of every `$host_*` in
//!     `backend/js/runtime.js`;
//!   * **rendering and hashing** ([`fmt`], [`hash`]) — the shortest-round-trip
//!     float formatter, 128-bit decimal, quoted `Show`, and FNV-1a over UTF-16
//!     code units. Every one of them has to produce *the same bytes as
//!     JavaScript* (VALUE-MODEL.md §12), which makes them a shared body rather
//!     than something each backend open-codes and gets subtly differently;
//!   * **`core/str` and the block-copying half of `core/list`** ([`text`],
//!     [`list`]) — UTF-8 slicing with the ASCII fast path, and the `[T]`
//!     producers that are a copy plus a retain;
//!   * **`core/char`'s eight** ([`char`]) — the classifiers, the two case
//!     mappings and `toDigit`. Seven of them are transcriptions of what
//!     `runtime.js` does; `isAlpha` is `\p{L}`, which is a General Category and
//!     not the **Alphabetic** property `char::is_alphabetic` answers, so that
//!     file carries the category as data and says where the data came from;
//!   * **the exactly-specified half of `core/math`** ([`math`]) — `sqrt`, the
//!     four rounding functions and the three predicates. The thirteen
//!     transcendentals are deliberately absent, and that file says why: IEEE
//!     754 does not fix their answers, so V8's fdlibm port and the platform's
//!     libm differ in the last bit, and a rendered `Float` shows seventeen
//!     digits of it;
//!   * **the stateful half of `core/host/testing`** ([`testing`]) — a captured
//!     stdout, a seeded generator, a test clock, a fixture environment, and a
//!     stdin that was handed its lines. Every one of them is mutable process state outliving the expression
//!     that made it, which is why each double carries an `I64` handle
//!     and puts the state on the runner's side; on JavaScript that side is
//!     `runtime.js`'s `$t.h`, and here it is one table. `alloc()`
//!     is the exception in both and both backends open-code it, because it
//!     reads no state;
//!   * **128-bit arithmetic** — [`buri_rt_i128_divmod`], [`buri_rt_i128_checked`]
//!     and [`buri_rt_i128_saturating`], at the bottom of this file. They are
//!     here for one reason: the overflow test both backends use at 64 bits is
//!     a widening multiply, which neither has at `i128`, so a hand-rolled
//!     128-bit one would be two code generators to get it wrong in.
//!
//! What deliberately stays *out* falls into two kinds, and the difference
//! matters:
//!
//!   * **It needs a type the runtime does not have.** Every `list.*` entry
//!     taking a closure, `zip`, `flatten`, and the whole of `json.*`. A closure
//!     is `{ code, env }` where `code`'s signature is the *flattened* one of its
//!     own element type, so calling one from C would mean synthesizing a call
//!     whose parameter list depends on `T` — a backend's job by construction.
//!   * **It has no single right answer, and guessing would be worse than a
//!     gap.** `core/math`'s thirteen transcendentals. Implementing `sin` with
//!     the platform libm would put a divergence into the toolchain that shows
//!     up on one input in a few thousand — which is the hardest kind of bug to
//!     find and the easiest to ship.
//!
//!     `core/char`'s classifiers used to be the second entry in this list, for
//!     the same reason spelled at `isAlpha`: Rust's `is_alphabetic` is the
//!     **Alphabetic** derived property and JavaScript's `\p{L}` is a General
//!     Category, and they differ on about fifteen hundred characters. They are
//!     out of this list rather than answered differently — [`char`] does not
//!     use `is_alphabetic`; it carries `\p{L}` as a table, generated from the
//!     engine the JavaScript backend runs on, and that file states the Unicode
//!     version. The transcendentals have no equivalent move available, because
//!     what differs there is the last bit of an arithmetic result and not a set
//!     that can be written down.
//!
//! Each is named where it would otherwise be looked for — `list.rs`'s and
//! `math.rs`'s headers, and `backend/{stencil,llvm}`'s missing-intrinsic
//! tables — rather than left to be discovered as a link error.
//!
//! ## 1. The symbol rule
//!
//! **Every exported symbol is `buri_rt_` followed by `snake_case`.** No
//! exceptions, including the host capabilities: `host.HostFs.readFile` is
//! `buri_rt_host_fs_read_file`. One prefix and one rule, so that "is this
//! symbol ours" is a string comparison and not a table.
//!
//! ## 2. The calling convention
//!
//! `extern "C"` — the *platform* ABI, not the flattened Buri-to-Buri one
//! (VALUE-MODEL.md §5.1). This is the single place in a Buri artifact where a
//! platform ABI appears, and it is why the runtime is Rust with `#[repr(C)]`
//! types rather than anything cleverer.
//!
//! Five rules make every signature below mechanical from the Buri one:
//!
//! 1. **Every parameter is a scalar leaf.** An aggregate parameter is
//!    flattened into its leaves, in declaration order, exactly as VALUE-MODEL
//!    §5.1 flattens a Buri call. So a `Str` argument is *three* parameters —
//!    `base`, `ptr`, `len` — and a `[T]` argument is two. Nothing is ever
//!    passed as a by-value aggregate, so no SysV classification rule and no
//!    aggregate-parameter support in a backend is ever needed.
//!
//! 2. **A scalar result is returned; an aggregate result is written through an
//!    out-pointer.** A function producing a `Str` takes `out: *mut BuriStr` and
//!    returns nothing, or returns a discriminant. Never `sret`, never a
//!    multi-word return, and therefore nothing for the two backends to disagree
//!    about.
//!
//! 3. **A sum-typed result returns its discriminant as an `i32`, and its
//!    payload through out-pointers.** The runtime does *not* know how
//!    `middle::layout` encoded `Option<Str>` or `Result<Str, IoError>` — that
//!    is the backend's business, and a niche (VALUE-MODEL.md §6) can change
//!    without touching this file. So the boundary is explicit:
//!    [`BURI_OK`] (`-1`) means the success arm, and `0 ..= n` is the error
//!    variant's index in *declaration order* in `core/effect`.
//!
//!    An `Option<T>` is the case with exactly one non-success arm, so it uses
//!    the same convention with nothing added: [`BURI_OK`] and the payload
//!    written, or `0` and the out-pointer untouched. Both backends translate
//!    that into whatever `middle::layout` chose — a tag, or a niche — and the
//!    runtime never learns which.
//!
//! 4. **A generic parameter arrives as a stride, and as glue.** One key is one
//!    body (`backend/runtime_table.rs`), so `list.map` at `[Int]` and at
//!    `[Point]` reach the same symbol and the element type does not cross.
//!    Everything the runtime needs to know about it arrives as a value: the
//!    element's **stride**, which `middle::layout` computed and which is an
//!    immediate at every call site, and a per-element **retain** function the
//!    backend generated, or null where the element holds no counted pointer
//!    ([`list`]'s header). A bare `T` — one with no leaf list a C signature
//!    could name — goes by address, at that same stride.
//!
//! 5. **A closure parameter arrives as an entry thunk and an opaque state.**
//!    Rule 4 answers "the runtime cannot name `T`" for a value; this answers it
//!    for a *call*. Four words: a `void(state, index, in, out)` the backend
//!    generated at the call site, where the element types are known; the
//!    backend's own record, which the runtime passes back untouched and never
//!    looks inside; and the two element strides, because an operation with a
//!    step reads one element type and writes another. The `index` is which item
//!    the call is for: the runtime drives the walk, so it is the only side that
//!    knows, and `effect Tasks` promises the step is told. It is on every step
//!    rather than on the keys that read it, so that this boundary has one C
//!    signature (`list.rs`'s `StepEntry`).
//!
//!    §0 says a closure "cannot be called from C", and it still cannot: what
//!    the runtime calls is the thunk, which is C. Two entries use this:
//!    `list.mapCtxStep`, the pilot, which is `list.mapCtx` reached a second way
//!    and exists to be compared against an answer that is already known; and
//!    `host.HostTasks.parallel`, which is the scheduler the pilot was landed
//!    for (`list.rs`'s `StepEntry`, `rt.rs`).
//!
//! ## 2.2 The one entry that goes the other way
//!
//! Every rule above describes a call **into** this crate. There is now one
//! symbol that describes a call **out of** it, and it is not in this file at
//! all: `backend/carrier.rs` fixes
//!
//! ```text
//!   void entry(void *state, void *out);
//! ```
//!
//! as the C door by which a carrier enters Buri code. Both native backends
//! emit one in front of a program's root. It is stated *there* rather than
//! here because the shape of the thing behind the door is the backend's: the
//! frame-threaded one takes a Buri data stack from
//! [`memory::buri_rt_stack_acquire`] and puts it in the frame-pointer
//! register, and the LLVM one is a `ccc` wrapper over a `fastcc` body and
//! needs none. What is *shared* is only the two words, which is why they are
//! written down in one file and diffed byte for byte by a test.
//!
//! [`memory::buri_rt_stack_acquire`] and [`memory::buri_rt_stack_release`] are
//! the two entries that pair with it, and they are the first `buri_rt_*`
//! symbols nothing in a Buri *program* names: no intrinsic key produces them,
//! neither runtime table has a row, and the only caller is a shim the stencil
//! backend hand-writes.
//!
//! ## 2.1 `Result<T, E>`, and the one thing rule 3 leaves open
//!
//! Rule 3 says "`0 ..= n` is the error variant's index", and for a long time
//! nothing used the range: every fallible entry was an `Option`, and `0` was
//! `.None`. `Result<T, E>` was recorded as deferred more than once, for the same
//! reason each time — *a typed error needs the error variant's payload, and
//! there is only one out-pointer*. That is the whole of the open question, and the
//! answer is that **the range is enough on its own**, because the discriminant
//! `0 ..= n` already names a variant and a variant with no fields is fully
//! determined by which one it is.
//!
//! So the convention is rule 3 with one sentence added, and the sentence is a
//! restriction rather than a mechanism:
//!
//!   * [`BURI_OK`], and `.Ok`'s payload written through the out-pointer — or,
//!     where `T` is zero-sized, **no out-pointer at all**, because a parameter
//!     for a value that occupies no bytes is a parameter the two sides can
//!     disagree about for free. `TestFs.writeFile` answers `Result<(), IoError>`
//!     and takes no `out`.
//!   * `0 ..= n`, naming a variant of `E` in declaration order, **which must
//!     carry no fields.** The out-pointer is untouched.
//!
//! A payload-carrying error *variant* is therefore not expressible, and that is
//! deliberate: it would need an out-pointer whose offset depends on which
//! variant `n` turned out to be, which is a switch the backend would generate
//! per entry rather than the two stores it generates now. Nothing in the archive
//! needs one — `IoError`'s seven variants are reached as `NotFound` and
//! `AlreadyExists`, both payload-less, exactly as `runtime.js`'s
//! `$host_testing_fsReadFile` returns `$err([0])`.
//!
//! **An error type that is not an enum at all is the other half, and it is
//! not the same problem.** `bytes.fromUtf8` answers `Result<Str, Utf8Error>`
//! where `Utf8Error(Int)` is a *struct* carrying the offset the decoding stopped
//! at — so there is no variant to name and no index that could name it, and the
//! value has exactly one place to go. It goes through **a second out-pointer**,
//! after `.Ok`'s and omitted on the same rule (present iff `E` occupies bytes),
//! and the discriminant is `0` meaning "the error is written there" rather than
//! "error variant zero".
//!
//! Which of the two an entry uses is not a column in either backend's table and
//! must not be: it is `E`'s own shape. An enum error is named by its index; any
//! other error is written through the pointer. A table column would be a second
//! statement of a fact the type already makes, and the two could disagree.
//!
//! The backends cannot *enforce* the payload-less restriction, and it is worth
//! saying which way that fails. `n` is a register, so "the variant it names has
//! no fields" is not a static question, and `IoError` has an `.Other(Str)` the
//! archive promises never to name. `stencil/rtcall.rs::store_result_tag` and
//! `llvm/emit.rs`'s `call_result` therefore **zero the error variant's payload
//! area** on the failure path: an entry that broke the promise produces
//! `.Other("")` — wrong, and safely wrong — rather than a reference count on
//! whatever the stack held.
//!
//! What the backend does with the two numbers is the mirror of what it already
//! does for an `Option`: store the `Result`'s own `.Ok`/`.Err` discriminant, and
//! on the error side store `n` as the tag of the `E` sitting at `.Err`'s payload
//! offset — or nothing at all, where the runtime wrote `E` itself.
//! `middle/layout.rs` gives a `Result` a `Bare` or a `Tagged`
//! representation and never a niche — the niche is `Option`'s alone
//! (`build_enum`'s first branch tests `is_option`) — so "store the tag" is one
//! store at a known offset in every case.
//!
//! 4. **A generic parameter is a pointer and a stride.** `list.get` and
//!    `list.push` cannot name `T`'s leaves, so the element crosses as
//!    `(*const u8, stride)` with the caller spilling to a stack slot, plus the
//!    per-element `retain` glue described in `list.rs`'s header. This is the
//!    one shape that is *not* mechanical from the Buri signature, and it exists
//!    for exactly the entries where the Buri signature has a type variable in
//!    it.
//!
//! ## 3. Ownership
//!
//! * A **parameter is borrowed.** No runtime function increments or decrements
//!   a count on anything it was passed. The caller's own reference keeps the
//!   value alive for the duration of the call, which is MEMORY.md §5.2's
//!   borrowed-parameter rule applied to the FFI edge.
//! * A **result is owned**, with `rc == 1`, transferred to the caller. The
//!   caller is what eventually decrefs it.
//! * A runtime function never stores a pointer it was passed.
//!
//! ## 4. The heap layout
//!
//! Per VALUE-MODEL.md §2 and MEMORY.md §5.1:
//!
//! ```text
//!   p - 16   u64  rc     reference count, or IMMORTAL (u64::MAX)
//!   p -  8   u64  cap    bit 63 shared, bit 62 arena, bits 0..61 usable
//!                        payload bytes — read through `memory::cap_of`
//!   p        ...  payload, 16-byte aligned
//! ```
//!
//! Every pointer that crosses this boundary — every `buri_rt_alloc` result,
//! every `BuriStr::base`, every `BuriList::ptr` — is a **payload** pointer, so
//! the header is always at `p - 16` and `incref`/`decref` are one sequence in
//! the whole program. VALUE-MODEL.md §3 writes `Str::base` as `*Header`; it is
//! the payload pointer of the allocation the header belongs to, which is the
//! same pointer shape as everything else, and the distinction is load-bearing
//! enough to say twice.
//!
//! `incref` and `decref` are **open-coded by both backends** and are not calls
//! (MEMORY.md §5.1). [`buri_rt_incref`] and [`buri_rt_decref`] exist for the
//! runtime's own use and for a defensive build, not for the hot path.
//!
//! ## 5. The allocator, in v1
//!
//! **`malloc`-backed, one block per allocation, no size classes.** MEMORY.md
//! §5.4 describes segregated free lists over `mmap` chunks and says both
//! backends open-code the fast path; that is the growth path and it is not v1.
//! v1 is a call to [`buri_rt_alloc`], which is `std::alloc::alloc` with a
//! 16-byte alignment and the header written. The reason to ship this first is
//! that the size-class allocator is an optimization with no observable
//! behaviour of its own — the header, the counts, `cap`, the reuse test and the
//! `Alloc` cost model (MEMORY.md §7, which is *defined* and not measured) are
//! all identical either way — so it can be replaced under a green test suite
//! rather than co-developed with the backends.
//!
//! ## 5.1 `Alloc`, which is accounting and not allocation
//!
//! Five symbols, and none of them reaches the heap. MEMORY.md §7 makes the
//! `Alloc` cost model a **definition** computed from the types rather than a
//! measurement, so the accounting is a set of counters beside the allocator
//! and not inside it:
//!
//!   * [`buri_rt_host_alloc_allocate`] — the platform's `Alloc`, which counts
//!     nothing and answers the bytes it was asked for.
//!   * [`buri_rt_alloc_new_counter`], [`buri_rt_alloc_charge`],
//!     [`buri_rt_alloc_count`], [`buri_rt_alloc_total`] — `core/alloc`'s
//!     `GeneralPurpose`, `Arena` and `FixedBuffer`, which carry a handle into
//!     this table because Buri has no mutation to hold a running total with.
//!
//! [`buri_rt_alloc_charge`] is the one entry in this file that can end the
//! process on an ordinary success path: a `FixedBuffer` overrun aborts through
//! [`buri_rt_abort_alloc_budget`], because `allocate` answers `Region` and not
//! `Result` and there is no value to report the failure with (SPEC 6.10).
//! [`buri_rt_alloc_budget_check`] is the same check exposed on its own, for a
//! backend that would rather open-code the charge.
//!
//! The counters are deliberately *not* [`buri_rt_heap_stats`]. That one is a
//! measurement of `malloc` and has no counterpart on the JavaScript backend;
//! these are the defined model, and the same program produces the same numbers
//! from `runtime.js`'s `$alloc_*`.
//!
//! ## 5.2 The scope, which *is* allocation
//!
//! Seven more, and they are the one exception to the paragraph above:
//! [`buri_rt_alloc_arena_create`], [`buri_rt_alloc_arena_allocate`],
//! [`buri_rt_alloc_arena_release`], [`buri_rt_alloc_arena_count`],
//! [`buri_rt_alloc_arena_total`], [`buri_rt_alloc_arena_enter`] and
//! [`buri_rt_alloc_arena_leave`] are `core/alloc`'s `scoped`, and an arena
//! really maps the bytes it is charged and really `munmap`s them when the scope
//! ends. Its charges are the same defined numbers `runtime.js` produces; its
//! *pages* have no counterpart there and need none, because no program can ask
//! about them. [`buri_rt_heap_stats`]'s `arena_bytes` and
//! `arena_released_bytes` are how a test does.
//!
//! **The last two are the ones that make Buri values live in an arena.**
//! `enter` makes the arena the one [`buri_rt_alloc`] serves out of, for *this
//! carrier* and for the dynamic extent of `body`; `leave` puts back whatever
//! was there before. That is how the context this ABI drops (§2 rule 1's
//! neighbour, `runtime_table::Entry::ctx`) is restored without putting it back
//! in every signature. `memory.rs`'s G5 section is the argument.
//!
//! ## 5.3 The copy out of a scope
//!
//! Two more, and they are the runtime's half of a walk the *backends* generate:
//! [`buri_rt_copy_block`] duplicates one block and hands the fresh one to the
//! type's own copy glue, and [`buri_rt_copy_str`] does the same for a `Str`,
//! whose `ptr` points into its block and has to be rebased onto the copy.
//!
//! The division is the same one §2's "a key carries no type arguments" draws:
//! everything that depends on a **type** is in the generated walk, where the
//! type is known, and everything that depends on a **block** is here, where the
//! header is. This archive is compiled once against no Buri type at all, so a
//! copy that had to know a layout could not live in it.
//!
//! **Neither of them increments a count.** *A copy is not a share* is the
//! property `core/alloc::scoped`'s bulk free rests on, and it is a property of
//! these two functions rather than of the callers.
//!
//! ## 6. Startup and shutdown, which the generated entry point owns
//!
//! Two calls the emitted `main` must make, two it may, and the whole of what it
//! must know:
//!
//! ```c
//! int main(int argc, char** argv) {
//!     buri_rt_argv_init(argc, argv);        /* first statement */
//!     buri_rt_values_may_cross_tasks();     /* if they may — before any block */
//!     buri_rt_frames_are_per_carrier();     /* if, and only if, they are */
//!     ...                                   /* the program */
//!     buri_rt_flush();                      /* before every return path */
//!     return 0;
//! }
//! ```
//!
//! [`buri_rt_argv_init`] is what makes `env.args(ctx)` exact — `std::env` in
//! a staticlib depends on a platform-specific startup hook that a linker
//! `--gc-sections` pass is entitled to have opinions about — and it installs
//! the panic hook that turns a runtime bug into a message rather than a bare
//! `SIGABRT`. If it is never called, `env.args(ctx)` falls back to `std::env`
//! and the fallback is correct on both supported platforms; the call is
//! preferred, not required.
//!
//! [`buri_rt_frames_are_per_carrier`] is the artifact's one statement about
//! *itself* rather than about its arguments: whether a second carrier entering
//! Buri code gets frames of its own. Saying nothing is the safe answer and the
//! old behaviour — `Tasks.parallel` then runs its steps one at a time — so an
//! entry point that forgets it is slow and not wrong. Its doc comment says
//! which backend says it and why the other cannot yet.
//!
//! [`memory::buri_rt_values_may_cross_tasks`] is the artifact's *other*
//! statement about itself, and it is a different fact rather than the same one
//! twice: this one says whether a block this program allocates can come to be
//! reachable from a second carrier, which `middle::rc::crosses_tasks` answers
//! for the whole program, and **both** native backends make it for exactly the
//! programs it is true of. Its effect is that every block carries
//! `middle::layout::CAP_SHARED_FLAG`, so every reference operation takes the
//! atomic arm and no in-place write fires on a borrowed value. Saying nothing
//! is again the safe answer — `Tasks.parallel` refuses to fan out unless it has
//! been said — so an entry point that forgets *this* one is also slow and not
//! wrong. The one thing it must get right is the **order**: it comes before
//! anything that allocates, which is why it sits next to `argv_init`, and
//! `argv_init` itself builds no Buri block.
//!
//! [`buri_rt_flush`] is required. Standard output and standard error are
//! **buffered**, exactly as `$host` buffers them on JavaScript
//! (`runtime.js:1224-1234`), so that the write ordering a program observes is
//! the same on both backends. [`buri_rt_host_proc_exit_with`] and every abort
//! path flush for themselves; a normal return does not.
//!
//! ## 7. Platforms
//!
//! macOS and Linux, by `cfg(unix)` plus `std`. There is no Windows support and
//! no cross-compilation: the archive is built for the host by `cli/build.rs`
//! and for nothing else (ARCHITECTURE.md §9).
//!
//! ## 8. Features, and the manifest that is not called `Cargo.toml`
//!
//! Two features. `net` is on by default: `tokio`, `hyper`, `rustls`, `ring`
//! and `tungstenite`. `net-h3` is **off** by default and adds `quinn`. Together
//! they are the runtime's whole admitted dependency set, closed by an exact
//! list rather than by a habit (`manifest.toml` argues each entry, the root
//! `Cargo.toml` states the bar, and `dependencies_stay_behind_the_bar` asserts
//! the equality).
//!
//! **Three of the six are linked and three are not.** [`rt`] is the carrier
//! runtime — the reactor, the run baton, the carrier pool and the task table —
//! and `Clock::sleepMillis` and `Net::fetch` wait on it, so the archive carries
//! the reactor's code on purpose; `rustls` over `ring` is what [`tls`] uses for
//! `https://`, and it is why the archive grew by about 1.72 MiB, most of it
//! `ring`'s native object code, which a `staticlib` carries whether the linker
//! wants it or not. `hyper`, `tungstenite` and `quinn` are still referenced
//! only by [`net`], which names one type from each and stops, so `lto = "fat"`
//! leaves them out of the archive entirely — measurably: an archive built with
//! `net-h3` is *forty bytes smaller* than one without it, because the QUIC
//! crate does not reach it and the refusal string does not have to.
//! `.github/scripts/assert-runtime-archive.sh` holds both halves in CI by
//! grepping the symbol table — three names that must be there, three that must
//! not — and each of the three moved across that line in the commit that
//! linked it. The slice that links one of the other three moves it again.
//!
//! Nothing about the **feature's** shape changed with any of it: `net` off is
//! still a runtime with no dependency at all, [`rt`] and [`tls`] do not
//! compile, `Clock::sleepMillis` is `thread::sleep` as it always was, and
//! `https://` goes back to a refusal that names this feature as the reason.
//!
//! **`net-h3` is the opposite default, and the asymmetry is the argument.**
//! `net`'s five crates are what a program that speaks the network at all
//! needs, so they are on unless something takes them away; `quinn` is what a
//! program that has *asked for HTTP/3* needs, and the concurrency note gated h3
//! behind configuration until the crate is trusted. `BURI_RUNTIME_NET_H3=1` is
//! that configuration, `net-h3` implies `net` because QUIC carries TLS 1.3
//! inside the transport, and the provider is pinned to `ring` by name so that
//! an h3 archive has one cryptography implementation in it and not two.
//!
//! What a toolchain without it owes a program that asked for `.Http3` is a
//! **value**: [`net::serves`] answers `Err`, `serve` returns
//! `.Err(Unsupported)`, and the process keeps running. It is not an abort,
//! because a toolchain built without a feature is a configuration the program
//! can report or fall back from rather than a broken invariant (§5), and it is
//! not a compile-time refusal, because the keys HTTP/3 is reached through are
//! `HostListen`'s and refusing them would refuse every server that was only
//! ever going to speak HTTP/1.1. `buri_rt_net_h3_available` is the door a
//! linked program asks the same question through.
//!
//! **What a toolchain built without it owes the user.** `net` off is a
//! *language capability* missing, not a code generator missing, so the
//! compiler is told rather than left to find out: `cli/build.rs` writes the
//! feature list to `libburi_rt.a.features` beside the archive,
//! `runtime_native::net()` reads it, and `Backend::missing_intrinsics` refuses a
//! program reaching `host.HostListen.*`, `host.HostSockets.*` or
//! `host.HostTasks.*` with a diagnostic naming the operations
//! (`networking-not-available`) before code generation starts. Two of those
//! three families are reachable from ordinary source now: `Tasks` is granted on
//! the three non-page platforms and `Listen` on the two native ones, so a
//! program that calls `core/tasks` or serves through `core/net/server` meets a
//! toolchain built without `net` as a diagnostic naming the operations rather
//! than as an unresolved `buri_rt_*` symbol from the system linker. That is
//! what writing the refusal before any of the keys existed was for.
//! `host.HostSockets.*` is the family still waiting on a caller — `Sockets` is
//! granted alongside `Listen`, but nothing performs a WebSocket upgrade, so
//! there is no socket for a program to name — and the same rule already covers
//! it for the day there is.
//!
//! `host.HostNet.fetch` is deliberately **not** one of those keys, and that is
//! a decision rather than an omission: with `net` off this runtime still speaks
//! cleartext HTTP, because `http.rs` writes that client itself. What it loses is
//! `https://`, which refuses at run time with a message naming this feature. A
//! compile-time refusal would have refused every program that mentions
//! `Net.fetch`, including the ones that were only ever going to ask for
//! `http://`.
//!
//! A host with no C compiler gets the same `net`-off runtime, and gets it
//! automatically: `ring` builds C and assembly, so `cli/build.rs` probes for
//! `cc` and falls back with a `cargo:warning` rather than failing the
//! toolchain's build. That is the bar's third clause — degrade, do not break —
//! reaching a tool rather than a crate.
//!
//! The package's manifest is `manifest.toml` and its lockfile is
//! `manifest.lock`, neither named the way Cargo would name it, because a
//! `Cargo.toml` in this directory would delete the directory from the
//! published `buri` crate: `cargo package` skips a nested package
//! unconditionally, ahead of `include`. `cli/build.rs` assembles the real
//! package in `OUT_DIR` and builds it there; that file's header is the
//! workflow, including how the lockfile is regenerated.

#![allow(clippy::missing_safety_doc)]

mod abort;
mod bytes;
mod char;
mod fmt;
mod hash;
mod host;
mod http;
mod list;
mod math;
mod memory;
mod net;
mod rng;
/// The carrier runtime — the tokio handle, the run baton, the carrier pool and
/// the task table. Behind `net` in full: without the feature there is no
/// reactor to hold and nothing here compiles.
#[cfg(feature = "net")]
pub mod rt;
/// The machine-stack switch `rt.rs` suspends a task with: three hand-written
/// `(arch, os)` blocks behind one `buri_rt_task_switch`. Behind `net` because
/// the scheduler that calls it is, and because a runtime with no tasks has
/// nothing to switch.
#[cfg(feature = "net")]
mod switch;
mod testing;
mod text;
/// TLS for `http`'s `https://` half. Behind the `net` feature because it *is*
/// the feature's only linked user; a runtime without it refuses `https://` by
/// name (see §8).
#[cfg(feature = "net")]
mod tls;
mod value;

pub use abort::*;
pub use bytes::*;
pub use char::*;
pub use fmt::*;
pub use hash::*;
pub use host::*;
pub use list::*;
pub use math::*;
pub use memory::*;
pub use net::*;
pub use testing::*;
pub use text::*;
pub use value::*;

/// The discriminant a fallible runtime entry returns for its success arm.
///
/// `-1` rather than `0` so that an error variant's index is its index — no
/// biasing at either end of the call, and a backend that gets the sign wrong
/// fails immediately rather than silently reporting `NotFound`.
pub const BURI_OK: i32 = -1;

/// Whether every carrier gets its own Buri frames, declared by the artifact.
///
/// `false` until an entry point says otherwise, and the conservative answer is
/// the one that costs nothing: a runtime that believes frames are shared runs
/// `rt::buri_rt_host_tasks_parallel`'s steps one at a time, which is what it
/// did before there was a fan-out at all.
static FRAMES_PER_CARRIER: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// The artifact declaring that a second carrier entering Buri code gets frames
/// of its own — §6's third call, and the one that is *optional*.
///
/// It is a property of the **artifact**, not of a call site, which is why it is
/// stated once at startup rather than carried on the step boundary: what it
/// answers is "where does a Buri frame live", and a program has one answer.
///
/// * The **LLVM** backend calls it. A Buri frame there is a machine frame —
///   `alloca`s in an ordinary function — so the stack a task runs on is its
///   own and two tasks may be in flight at once. Since B9 that stack is one
///   this runtime maps, `memory::BURI_RT_STACK_BYTES` wide with a guard, so
///   the depth a task may recurse to is the same number on both backends;
///   before it, it was a carrier's 512 KiB thread stack, which is the
///   asymmetry `reports/wave6-b7b8.md` §5.2 recorded.
/// * The **frame-threaded** backend does not, and must not until each carrier
///   owns a Buri stack (track B, B7). Today a program has exactly one, the
///   `buri$stencil$stack` block its `main` guards, and an entry thunk works in a
///   frame the *call site* set aside — so two steps of one `parallel` would
///   share it, and one that suspends would still be holding it. Sequential is
///   not a limitation of that backend's scheduler; it is the truth about where
///   its frames are.
/// * A toolchain built without `net` has no scheduler to tell, and this is then
///   a store nobody loads.
///
/// Idempotent, and never unset: an artifact makes one statement about itself.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_frames_are_per_carrier() {
    FRAMES_PER_CARRIER.store(true, std::sync::atomic::Ordering::Release);
}

/// What [`buri_rt_frames_are_per_carrier`] was told, for the scheduler.
#[must_use]
pub fn frames_are_per_carrier() -> bool {
    FRAMES_PER_CARRIER.load(std::sync::atomic::Ordering::Acquire)
}

/// Unsay it, which only a test may do.
///
/// An artifact makes one statement about itself and never takes it back, so
/// this is not part of the contract above; it exists because `cargo test` runs
/// every case of this crate in one process, and a case that wants to see the
/// scheduler fan out must not leave every later case fanning out too.
///
/// `net`-gated as well as test-gated, because without the reactor there is no
/// scheduler to tell and `rt.rs` — the only caller — is not compiled.
#[cfg(all(test, feature = "net"))]
pub(crate) fn forget_frames_are_per_carrier() {
    FRAMES_PER_CARRIER.store(false, std::sync::atomic::Ordering::Release);
}

/// 128-bit division and remainder, in one call.
///
/// CODEGEN-STENCIL.md §5: this is a hundred instructions on both backends and
/// neither should inline it. Division by zero aborts here rather than at
/// the call site, so the two backends share one message.
///
/// Every operand is a **pair of `u64`s, low half first**, rather than an
/// `i128`: §2's first rule says a parameter is a scalar leaf, and a 128-bit
/// value is not one. Passing it as a pair also means neither backend has to
/// agree with the platform ABI about how a 128-bit integer is classified,
/// which is a place the two have historically differed.
///
/// `signed` selects `sdiv`/`srem` semantics over `udiv`/`urem`. Truncating
/// toward zero, so `a == (a / b) * b + (a % b)` holds — the same identity
/// `$divi` documents on JavaScript (`runtime.js:48-50`).
///
/// # Safety
/// `quot` and `rem` must each be non-null and point at two writable,
/// `u64`-aligned words (low half first).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_i128_divmod(
    a_lo: u64,
    a_hi: u64,
    b_lo: u64,
    b_hi: u64,
    signed: u8,
    quot: *mut u64,
    rem: *mut u64,
) {
    let a = u128::from(a_lo) | (u128::from(a_hi) << 64);
    let b = u128::from(b_lo) | (u128::from(b_hi) << 64);
    if b == 0 {
        buri_rt_abort_div_zero();
    }
    let (q, r) = if signed == 0 {
        (a / b, a % b)
    } else {
        // `i128::MIN / -1` overflows. Overflow is undefined in this language
        // (SPEC 6.2) and the two's-complement wrap is the native answer
        // (VALUE-MODEL.md §11.1), so wrap rather than abort.
        let (sa, sb) = (a as i128, b as i128);
        (sa.wrapping_div(sb) as u128, sa.wrapping_rem(sb) as u128)
    };
    unsafe {
        quot.write(q as u64);
        quot.add(1).write((q >> 64) as u64);
        rem.write(r as u64);
        rem.add(1).write((r >> 64) as u64);
    }
}


/// 128-bit checked arithmetic, in one call.
///
/// `op` selects the operation — `0` add, `1` sub, `2` mul, `3` div — and the
/// answer is [`BURI_OK`] with the result written through `out`, or `0` for the
/// `.None` an overflow (or a division by zero) produces.
///
/// It is a call rather than open-coded for the same reason
/// [`buri_rt_i128_divmod`] is: the overflow test both backends use at 64 bits is
/// a widening multiply, which neither backend has at `i128`, and a hand-rolled
/// 128-bit one in two code generators is two places to get it wrong. Here it is
/// `i128::checked_mul`.
///
/// **The bound is the type's own range**, which is `i128`/`u128` here and is
/// where this parts company with the JavaScript backend: `$checkedIn` tests
/// `exact_int_range` because past `2^53` a double cannot say *which* integer it
/// is, so `.None` is the only honest answer it has. `Checked` is bounded by the
/// numbers the *platform* has, and `.Some(v)` promises that `v` is the true
/// result as this backend represents numbers (SPEC 6.2.2,
/// `design/native/VALUE-MODEL.md` §12 row 2). Rust's `checked_*` is exactly that
/// promise, including `i128::MIN.checked_div(-1)`, which is `None` because
/// `2^127` has no two's-complement representation.
///
/// `buri_rt_i128_saturating` clamps at the same bounds, as `$sat` does on the
/// other backend: `Saturating` never had a second bound to lose.
///
/// Every operand is a **pair of `u64`s, low half first**, per §2's first rule.
///
/// # Safety
/// `out` must be non-null and point at two writable, `u64`-aligned words.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_i128_checked(
    op: u8,
    a_lo: u64,
    a_hi: u64,
    b_lo: u64,
    b_hi: u64,
    signed: u8,
    out: *mut u64,
) -> i32 {
    let a = u128::from(a_lo) | (u128::from(a_hi) << 64);
    let b = u128::from(b_lo) | (u128::from(b_hi) << 64);
    let answer = if signed == 0 {
        match op {
            0 => a.checked_add(b),
            1 => a.checked_sub(b),
            2 => a.checked_mul(b),
            _ => a.checked_div(b),
        }
    } else {
        let (sa, sb) = (a as i128, b as i128);
        let r = match op {
            0 => sa.checked_add(sb),
            1 => sa.checked_sub(sb),
            2 => sa.checked_mul(sb),
            _ => sa.checked_div(sb),
        };
        r.map(|v| v as u128)
    };
    // `None` is the whole answer: `checked_*` already reports every way the
    // type's own range can be left, and a division by zero along with it.
    let Some(v) = answer else { return 0 };
    // SAFETY: the caller promises two writable, aligned words.
    unsafe {
        out.write(v as u64);
        out.add(1).write((v >> 64) as u64);
    }
    BURI_OK
}

/// 128-bit saturating arithmetic, in one call. `op` is as
/// [`buri_rt_i128_checked`]'s, minus division, which cannot saturate.
///
/// `$sat(v, lo, hi)` clamps an exact double; this clamps at the type's own
/// bounds, which is the same statement without a wider type to compute in.
///
/// # Safety
/// As [`buri_rt_i128_checked`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_i128_saturating(
    op: u8,
    a_lo: u64,
    a_hi: u64,
    b_lo: u64,
    b_hi: u64,
    signed: u8,
    out: *mut u64,
) {
    let a = u128::from(a_lo) | (u128::from(a_hi) << 64);
    let b = u128::from(b_lo) | (u128::from(b_hi) << 64);
    let v = if signed == 0 {
        match op {
            0 => a.saturating_add(b),
            1 => a.saturating_sub(b),
            _ => a.saturating_mul(b),
        }
    } else {
        let (sa, sb) = (a as i128, b as i128);
        let r = match op {
            0 => sa.saturating_add(sb),
            1 => sa.saturating_sub(sb),
            _ => sa.saturating_mul(sb),
        };
        r as u128
    };
    // SAFETY: the caller promises two writable, aligned words.
    unsafe {
        out.write(v as u64);
        out.add(1).write((v >> 64) as u64);
    }
}
