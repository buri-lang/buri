//! The `buri_rt_*` table the frame-threaded backend reads: which intrinsic
//! keys have a symbol, and what shape the call has.
//!
//! `cli/runtime/lib.rs`'s module comment is the contract; this is the
//! transcription of it that the copy-and-patch backend's generated code is
//! emitted against. It is one table because a table describes `cli/runtime`,
//! which is one library — two transcriptions of one library disagreeing about
//! a row is one of them being wrong. It sits here rather than under
//! `stencil/` because that is what the file has always been: a description of
//! the runtime, not of a code generator.
//!
//! The LLVM backend keeps its own (`llvm/runtime.rs`), and that one is a
//! different table rather than a copy: it reconstructs the C argument list
//! from the row, so every entry there carries an `args` column this one has no
//! use for. The two tables also do not name quite the same keys, which is a
//! fact about what each backend has implemented and not a naming convention.
//!
//! # The four shapes, and the one emission rule
//!
//! Every call this table describes is emitted by the same sequence, in this
//! order and with nothing conditional in it but the table row:
//!
//! ```text
//!   1. the Buri arguments, flattened into scalar leaves  (lib.rs §2 rule 1)
//!   2. the element pair — stride, then retain glue       (lib.rs §2 rule 4)
//!      or a step's four words                            (lib.rs §2 rule 5)
//!   3. the out-pointer                                   (lib.rs §2 rules 2, 3)
//! ```
//!
//! Step 1 is the IR's own argument list: a `Str` argument spreads to three
//! values because a `Str` *is* three leaves. There is no per-argument column
//! for the shapes, because a second description of them would be a thing to
//! disagree with.
//!
//! Two arguments are the exception, and both are here because the IR genuinely
//! cannot answer them: [`Entry::by_ref`], whose type is a bare `T`, and
//! [`Entry::ctx`], which the C signature has no parameter for. The second one
//! read "a `ctx` spreads to no leaves because it occupies no bytes" for a long
//! time, and that is a fact about `core/host`'s empty marker structs rather
//! than about contexts — see [`Entry::ctx`] for what it costs when a program
//! writes something else.
//!
//! Steps 2 and 3 are what [`Extra`] and [`Ret`] select. What the backend
//! supplies for itself is where an argument *is*: its convention is
//! frame-threaded, so a leaf is a byte offset copied into a scratch area
//! (`stencil/rtcall.rs`).
//!
//! # A key carries no type arguments
//!
//! `middle::monomorphize` builds an intrinsic key out of the module, the type
//! and the name, and out of nothing else. One key is one row here and one
//! `buri_rt_*` symbol, so **every instantiation of a generic intrinsic reaches
//! the same body**: `list.map` at `[Int]` and at `[Point]` are one call, and
//! the element type does not cross. It cannot — this archive is compiled once,
//! against no Buri type at all.
//!
//! That is what steps 2 and 3 above are for. Everything the runtime has to
//! know about an erased type arrives as a value:
//!
//! * the element **stride and retain glue** of [`Extra::Element`] — the shape
//!   of a `[T]`, which is why `core/list`'s rows carry it and `core/bytes`'s
//!   do not (their element type is fixed at `U8`);
//! * an **address**, through [`Entry::by_ref`], for an argument whose type is
//!   a bare `T` and so has no leaf list a C signature could name;
//! * a **runtime descriptor**, for an operation whose subject is the *whole*
//!   shape of a type rather than its size — `json.decode` and the test
//!   runner's two, which are `middle::monomorphize`'s `Func::desc` and reach
//!   no row here;
//! * the **entry thunk** of [`Extra::Step`] — a function this backend
//!   generated, which is the same idea as the retain glue of the first bullet
//!   and answers a harder question with it: not "what does one element hold"
//!   but "how is one Buri closure called", which is the one thing
//!   `cli/runtime/list.rs`'s header says C cannot do.
//!
//! An intrinsic that is generic and has none of these is a miscompile with no
//! diagnostic, so the set of keys allowed to be generic is a written list —
//! `GENERIC_INTRINSICS` in `middle/monomorphize.rs` — and a generic intrinsic
//! outside it is an internal error at monomorphization. **A new row here for a
//! generic key needs a row there too**, and the question that list is asking is
//! which of the carriers above carries the type.
//!
//! # Why a table and not a mangling
//!
//! The rule in `lib.rs` §1 would happily produce `buri_rt_list_map` for
//! `list.map`, which does not exist, and a program that used it would get a
//! link error naming a symbol instead of `Backend::missing_intrinsics` naming
//! the operation. The mangler lives in `runtime_native.rs` as `symbol_for`,
//! and it is used to *check* this table rather than to drive emission.

/// The discriminant a fallible runtime entry returns for its success arm.
///
/// `cli/runtime/lib.rs`'s `BURI_OK`, restated here because the compiler and the
/// runtime are two crates that never link against each other — the archive is
/// `include_bytes!`d, not depended on — so a shared constant is impossible and
/// the two spellings are held together by `cli/tests/native/runtime.rs`'s C
/// driver instead.
pub const BURI_OK: i32 = -1;

/// What the backend appends after the flattened Buri arguments.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Extra {
    /// Nothing.
    None,
    /// The element **stride** and the per-element **retain glue** of the
    /// `[T]` this call operates on — `cli/runtime/list.rs`'s header, and
    /// `lib.rs` §2 rule 4. The element type is the argument's where there is a
    /// `[T]` argument and the result's otherwise, which covers `list.repeat`
    /// and `list.empty`, whose only mention of `T` is in the return type.
    Element,
    // -- the closure trampoline --------------------------------------------
    /// The four words a **runtime-driven step** crosses on
    /// (`backend/intrinsic_keys.rs`'s `step_call`):
    ///
    /// ```text
    ///   entry       the generated C-ABI thunk, `void(state, index, in, out)`
    ///   state       the backend's own record, opaque to the runtime
    ///   in_stride   the source element's stride
    ///   out_stride  the result element's stride
    /// ```
    ///
    /// This is [`Extra::Element`]'s idea with the pair widened, and it carries
    /// the erased type the same way: a **stride** for what the runtime walks,
    /// and a **function this backend generated** for what the runtime cannot
    /// name. There is no retain glue, because there is nothing here for the
    /// runtime to retain — the entry thunk is handed one element at a time and
    /// takes its own count on it, which is `middle/rc.rs`'s "a call through a
    /// function value owns its arguments" answered on the side of the boundary
    /// that knows the type.
    ///
    /// Two strides rather than one because a `map` reads a `[A]` and writes a
    /// `[B]`, and neither is the other's: [`Extra::Element`]'s single pair is
    /// wrong for this shape rather than merely narrow.
    ///
    /// The closure argument itself is **not** flattened. `{ code, env }` is
    /// this backend's business and reaches the runtime inside `state`; the
    /// closure is the last argument at every key `step_call` names, so "skip it
    /// and append the four" and "write the four where it stood" are the same C
    /// signature — which is what lets this table, which names no *shape* per
    /// argument, describe the same call `llvm/runtime.rs`'s `Arg::Step` does.
    Step,
}

/// What comes back.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ret {
    /// Nothing, and the Buri result is `()`.
    Void,
    /// One scalar, already at the destination's register shape.
    Scalar,
    /// One `i32`, narrowed to the destination's own tag width.
    ///
    /// `Order` is three variants and `middle::layout` gives it an `i8` tag; the
    /// runtime returns a C `int`. Declaring the import as returning `i8` would
    /// work on both supported platforms by accident — the low byte of `eax` is
    /// the low byte of the value — and would be wrong the first time a target
    /// returned a narrow integer unextended. So the import says `i32` and the
    /// narrowing is an instruction.
    Tag,
    /// An aggregate, written through a trailing out-pointer (`lib.rs` §2
    /// rule 2). The pointer is the destination's own stack slot, so the call
    /// writes the value where it already belongs and nothing is copied after.
    Out,
    /// An `Option<T>`: an `i32` discriminant, and the payload through a
    /// trailing out-pointer (`lib.rs` §2 rule 3).
    ///
    /// The out-pointer is the destination slot **offset to `.Some`'s payload**,
    /// so again nothing is copied after the call — and the runtime never learns
    /// whether `middle::layout` chose a tag or a niche, which is exactly what
    /// rule 3 is protecting.
    Opt,
    /// A `Result<T, E>`: an `i32` discriminant, `.Ok`'s payload through a
    /// trailing out-pointer, and an error variant **named by its index**
    /// (`lib.rs` §2.1).
    ///
    /// The difference from [`Ret::Opt`] is entirely on the failure side. An
    /// `Option`'s one failure is `.None` and carries nothing; a `Result`'s is a
    /// value of `E`, and the discriminant `0 ..= n` is what says which one — so
    /// the backend stores `.Err`'s tag into the `Result` and then `n` into the
    /// `E` sitting at `.Err`'s payload offset. `lib.rs` §2.1 states the
    /// restriction that makes two stores enough: the variant `n` names carries
    /// no fields.
    ///
    /// The out-pointer is **omitted where `T` is zero-sized**, which is
    /// `TestFs.writeFile`'s `Result<(), IoError>`. A parameter for a value that
    /// occupies no bytes is one the two sides can disagree about for free, and
    /// `Ret::Out` already drops it for the same reason.
    Res,
    /// The call does not come back (SPEC 6.10).
    NoReturn,
}

/// One runtime entry this backend can emit a call to.
pub struct Entry {
    /// The intrinsic key `monomorphize` built: `str.slice`, `host.HostStdout.println`.
    pub key: &'static str,
    /// The exported symbol, per `cli/runtime/lib.rs` §1.
    pub symbol: &'static str,
    pub extra: Extra,
    pub ret: Ret,
    /// The index, in the Buri argument list, of an argument passed **by
    /// address** rather than flattened into leaves.
    ///
    /// Exactly the arguments whose type is a bare type variable: `list.push`'s
    /// item and `list.repeat`'s. `lib.rs` §2 rule 1 flattens an aggregate into
    /// its leaves, and a `T` has no leaf list a C signature could name, so the
    /// caller spills it to a stack slot and passes the address — which is
    /// `lib.rs` §2 rule 4, and the same reason `stride` is a parameter.
    pub by_ref: Option<usize>,
    /// The index, in the Buri argument list, of the operation's **context**
    /// parameter — the one the C signature has no parameter for at all.
    ///
    /// `cli/runtime` allocates through `buri_rt_alloc` and reads no capability
    /// (`sources/alloc.buri`'s header says so), so a `ctx: C` crosses nothing.
    /// The question is *which argument that is*, and it is a fact about the
    /// **declaration** rather than about the value: `list.push(self, ctx,
    /// item)` names its second, `list.repeat(ctx, item, times)` its first.
    ///
    /// It is a column here for the reason [`Entry::by_ref`] is one — the IR
    /// cannot answer it. Asking the argument's *type* instead ("is it a
    /// `Ty::Ctx`?") is the same question only while every `C` is instantiated
    /// at a `context { … }` record, and `C` is an ordinary type parameter with
    /// an ordinary bound (SPEC 10.1): a value that *implements* `Alloc`
    /// satisfies `C: Alloc` without being a context, which is what SPEC 10.8's
    /// attenuating `ReadOnly<C>` and `core/host/testing`'s `alloc()` both are.
    /// One of those in this position spread to a leaf the C signature has no
    /// parameter for and shifted every argument after it — which links, runs,
    /// and dies in `memmove`.
    ///
    /// `llvm/runtime.rs` says the same thing with an `Arg::Dropped` at this
    /// index, and `an_entry_names_the_context_the_other_table_drops` holds the
    /// two together.
    pub ctx: Option<usize>,
}

const fn e(key: &'static str, symbol: &'static str, ret: Ret) -> Entry {
    Entry { key, symbol, extra: Extra::None, ret, by_ref: None, ctx: None }
}

const fn el(key: &'static str, symbol: &'static str, ret: Ret) -> Entry {
    Entry { key, symbol, extra: Extra::Element, ret, by_ref: None, ctx: None }
}

const fn er(key: &'static str, symbol: &'static str, ret: Ret, by_ref: usize) -> Entry {
    Entry { key, symbol, extra: Extra::Element, ret, by_ref: Some(by_ref), ctx: None }
}

/// `entry`, with the index of its declaration's `ctx` parameter
/// ([`Entry::ctx`]).
const fn cx(entry: Entry, at: usize) -> Entry {
    Entry { ctx: Some(at), ..entry }
}

/// A runtime-driven step ([`Extra::Step`]).
const fn es(key: &'static str, symbol: &'static str, ret: Ret) -> Entry {
    Entry { key, symbol, extra: Extra::Step, ret, by_ref: None, ctx: None }
}

/// Every key this backend has a runtime body for.
///
/// Grouped by the module the key names, and in each group by the order
/// `core/<module>` declares them, so that a reader comparing this against
/// `str.buri` or `list.buri` can see at a glance what is absent.
///
/// `str.concat` is absent and is the one absence that is not a missing body:
/// the archive exports `buri_rt_str_concat` and the copy-and-patch backend
/// calls it. Its two `Str` lengths go **unmasked**, because VALUE-MODEL.md
/// §3.1's ASCII flag is an input to a concatenation rather than a tag, and the
/// flattening this table drives masks every length. So the call is emitted at
/// the one site that knows that (`stencil/rtcall.rs`'s `str_concat`), and a row
/// here would be a second and wrong way to reach the same symbol.
pub const ENTRIES: &[Entry] = &[
    // -- core/str, pure -----------------------------------------------------
    //
    // Every one of these answers a *view* into the receiver's block and increfs
    // its base before doing so (`cli/runtime/text.rs`'s header). That is what
    // makes `slice`, `trim` and `splitOnce` allocation-free, which is what
    // `str.buri:26-45` says by declaring them without an `Alloc` bound.
    e("str.charAt", "buri_rt_str_char_at", Ret::Opt),
    e("str.slice", "buri_rt_str_slice", Ret::Out),
    e("str.trim", "buri_rt_str_trim", Ret::Out),
    e("str.trimStart", "buri_rt_str_trim_start", Ret::Out),
    e("str.trimEnd", "buri_rt_str_trim_end", Ret::Out),
    e("str.startsWith", "buri_rt_str_starts_with", Ret::Scalar),
    e("str.endsWith", "buri_rt_str_ends_with", Ret::Scalar),
    e("str.contains", "buri_rt_str_contains", Ret::Scalar),
    e("str.indexOf", "buri_rt_str_index_of", Ret::Opt),
    e("str.splitOnce", "buri_rt_str_split_once", Ret::Opt),
    e("str.compare", "buri_rt_str_compare", Ret::Tag),
    e("str.eq", "buri_rt_str_eq", Ret::Scalar),
    e("str.hash", "buri_rt_str_hash", Ret::Scalar),
    e("str.toInt", "buri_rt_str_to_int", Ret::Opt),
    e("str.toFloat", "buri_rt_str_to_float", Ret::Opt),
    // -- core/str, `Alloc`-bounded ------------------------------------------
    cx(e("str.split", "buri_rt_str_split", Ret::Out), 1),
    cx(e("str.splitAny", "buri_rt_str_split_any", Ret::Out), 1),
    cx(e("str.lines", "buri_rt_str_lines", Ret::Out), 1),
    cx(e("str.replace", "buri_rt_str_replace", Ret::Out), 1),
    cx(e("str.repeat", "buri_rt_str_repeat", Ret::Out), 1),
    cx(e("str.toUpper", "buri_rt_str_to_upper", Ret::Out), 1),
    cx(e("str.toLower", "buri_rt_str_to_lower", Ret::Out), 1),
    cx(e("str.chars", "buri_rt_str_chars", Ret::Out), 1),
    cx(e("str.fromChars", "buri_rt_str_from_chars", Ret::Out), 0),
    cx(e("str.fromInt", "buri_rt_str_from_int", Ret::Out), 0),
    cx(e("str.fromFloat", "buri_rt_str_from_float", Ret::Out), 0),
    cx(e("str.padStart", "buri_rt_str_pad_start", Ret::Out), 1),
    cx(e("str.padEnd", "buri_rt_str_pad_end", Ret::Out), 1),
    // -- core/list ----------------------------------------------------------
    //
    // `len` is open-coded (it is a load) and every entry taking a closure is
    // absent; `cli/runtime/list.rs`'s header says which and why.
    el("list.get", "buri_rt_list_get", Ret::Opt),
    cx(el("list.concat", "buri_rt_list_concat", Ret::Out), 1),
    // `push(self, ctx, item)` — the item is a `T`, so it goes by address.
    cx(er("list.push", "buri_rt_list_push", Ret::Out, 2), 1),
    cx(el("list.reverse", "buri_rt_list_reverse", Ret::Out), 1),
    cx(el("list.slice", "buri_rt_list_slice", Ret::Out), 1),
    cx(el("list.take", "buri_rt_list_take", Ret::Out), 1),
    cx(el("list.drop", "buri_rt_list_drop", Ret::Out), 1),
    // `repeat(ctx, item, times)` — likewise, one place earlier.
    cx(er("list.repeat", "buri_rt_list_repeat", Ret::Out, 1), 0),
    cx(e("list.range", "buri_rt_list_range", Ret::Out), 0),
    cx(e("list.join", "buri_rt_list_join", Ret::Out), 1),
    // -- the closure trampoline, and its one pilot key ----------------------
    //
    // `list.mapCtxStep` is `list.mapCtx` with its step reached through the
    // C-ABI entry thunk of [`Extra::Step`] instead of through the loop
    // `stencil/lists.rs` open-codes. It is the *pilot* for that mechanism and
    // nothing in `core/list` uses it: those combinators keep their loops, which
    // are faster than a call per element can be.
    //
    // So this row landed to be *called*, by a conformance fixture and by an
    // agreement row, before there was anything else to call it with. The
    // alternative was landing the boundary underneath `Tasks.parallel` and
    // debugging two new things at once. `host.HostTasks.parallel` is that
    // second key and it is in the `core/host` block below, beside the rest of
    // the host surface rather than up here — the trampoline is a mechanism, not
    // a section of this table.
    cx(es("list.mapCtxStep", "buri_rt_list_map_ctx_step", Ret::Out), 1),
    // -- core/bytes ---------------------------------------------------------
    //
    // Six of `bytes.buri`'s surface, and the rest of that module is Buri:
    // hexadecimal, base64, varints and zigzag are arithmetic over a `[U8]`.
    // These six are the two conversions whose answer is the platform's
    // representation — the UTF-8 encoding of a string, and the IEEE 754 byte
    // pattern of a `Float`.
    //
    // `Extra::None` at every one of them, including the three answering a
    // `[U8]`: the element type is fixed at `U8`, so there is no `T` for the
    // stride-and-glue pair of `lib.rs` §2 rule 4 to describe, and
    // `cli/runtime/value.rs`'s `list_of_bytes` knows the stride is one.
    cx(e("bytes.toUtf8", "buri_rt_bytes_to_utf8", Ret::Out), 0),
    // `Result<Str, Utf8Error>` — §2.1's *second* error shape. `Utf8Error(Int)`
    // is a struct, so there is no variant index to name it with and the value
    // crosses through its own out-pointer.
    cx(e("bytes.fromUtf8", "buri_rt_bytes_from_utf8", Ret::Res), 0),
    cx(e("bytes.f64ToBytes", "buri_rt_bytes_f64_to_bytes", Ret::Out), 0),
    e("bytes.f64FromBytes", "buri_rt_bytes_f64_from_bytes", Ret::Opt),
    cx(e("bytes.f32ToBytes", "buri_rt_bytes_f32_to_bytes", Ret::Out), 0),
    e("bytes.f32FromBytes", "buri_rt_bytes_f32_from_bytes", Ret::Opt),
    // -- core/char ----------------------------------------------------------
    //
    // Eight of `char.buri`'s nine. `toU32` is the ninth and is not here: a
    // `Char` **is** a `U32`, so it is a representation change the backend
    // open-codes, and `isAlphanumeric` is written in Buri over two of these.
    //
    // Every one of them is one comparison or one table lookup, and a call is
    // more instructions than the answer — which is the argument for open-coding
    // that `str.concat` wins and this loses. `isAlpha` is a binary search over
    // six hundred ranges of Unicode data, `isUpper` is two full case mappings,
    // and open-coding *those* in two backends is two places for the data to
    // drift. So all eight go through the archive together rather than four of
    // them here and four there, and `cli/runtime/char.rs` is the one place the
    // answers live.
    e("char.isDigit", "buri_rt_char_is_digit", Ret::Scalar),
    e("char.isAlpha", "buri_rt_char_is_alpha", Ret::Scalar),
    e("char.isSpace", "buri_rt_char_is_space", Ret::Scalar),
    e("char.isUpper", "buri_rt_char_is_upper", Ret::Scalar),
    e("char.isLower", "buri_rt_char_is_lower", Ret::Scalar),
    e("char.toUpper", "buri_rt_char_to_upper", Ret::Scalar),
    e("char.toLower", "buri_rt_char_to_lower", Ret::Scalar),
    e("char.toDigit", "buri_rt_char_to_digit", Ret::Opt),
    // -- core/math, the exactly-specified half --------------------------------
    //
    // Nine of twenty-two. `cli/runtime/math.rs` says why the other thirteen are
    // not here, and the short version is that IEEE 754 does not fix a
    // transcendental's answer, so V8 and the platform libm differ in the last
    // bit — which a rendered `Float` shows.
    e("math.sqrt", "buri_rt_math_sqrt", Ret::Scalar),
    e("math.absFloat", "buri_rt_math_abs_float", Ret::Scalar),
    e("math.floor", "buri_rt_math_floor", Ret::Scalar),
    e("math.ceil", "buri_rt_math_ceil", Ret::Scalar),
    e("math.trunc", "buri_rt_math_trunc", Ret::Scalar),
    e("math.round", "buri_rt_math_round", Ret::Scalar),
    e("math.isNan", "buri_rt_math_is_nan", Ret::Scalar),
    e("math.isInfinite", "buri_rt_math_is_infinite", Ret::Scalar),
    e("math.isFinite", "buri_rt_math_is_finite", Ret::Scalar),
    // -- the text streams ---------------------------------------------------
    //
    // `Result<(), IoError>` on all five, which is [`Ret::Res`] with the
    // out-pointer omitted — `()` occupies no bytes — so the C signature is the
    // arguments, a trailing `out_err`, and an `i32` discriminant. The same
    // shape `Fs`'s writers have, and for the same reason: a stream a program
    // cannot write to is a failure the program can act on, and a signature
    // saying `()` was claiming otherwise.
    e("host.HostStdout.print", "buri_rt_host_stdout_print", Ret::Res),
    e("host.HostStdout.println", "buri_rt_host_stdout_println", Ret::Res),
    e("host.HostStdout.writeBytes", "buri_rt_host_stdout_write_bytes", Ret::Res),
    e("host.HostStderr.eprint", "buri_rt_host_stderr_eprint", Ret::Res),
    e("host.HostStderr.eprintln", "buri_rt_host_stderr_eprintln", Ret::Res),
    // -- the scalar capabilities --------------------------------------------
    e("host.HostFs.fileExists", "buri_rt_host_fs_file_exists", Ret::Scalar),
    e("host.HostClock.nowMillis", "buri_rt_host_clock_now_millis", Ret::Scalar),
    e("host.HostClock.sleepMillis", "buri_rt_host_clock_sleep_millis", Ret::Void),
    e("host.HostRand.nextInt", "buri_rt_host_rand_next_int", Ret::Scalar),
    e("host.HostRand.nextFloat", "buri_rt_host_rand_next_float", Ret::Scalar),
    e("host.HostProc.exitWith", "buri_rt_host_proc_exit_with", Ret::NoReturn),
    // `allocate(self, bytes) -> Region`. `self` is `HostAlloc`, an empty
    // struct, so it flattens to nothing and the C call is the one `i64`; the
    // result is `struct Region(I64)`, whose single leaf is what makes
    // [`Ret::Scalar`] right where `host_testing.stdout`'s
    // `struct TestStdout(I64)` needs [`Ret::Out`] — the difference is the *C*
    // signature, and `buri_rt_host_alloc_allocate` returns an `i64` rather
    // than a struct.
    //
    // MEMORY.md §7 is the body: `HostAlloc` is zero-sized and unbounded, so
    // the charge is the request and the accounting is the caller's. The row is
    // here rather than open-coded next to `TestAlloc.allocate` because the
    // archive already has the body and `llvm/runtime.rs` already calls it — two
    // backends reaching one definition of a *defined* cost model, which is what
    // §7.1 means by "the same number on both backends".
    e("host.HostAlloc.allocate", "buri_rt_host_alloc_allocate", Ret::Scalar),
    // -- Tasks --------------------------------------------------------------
    //
    // `parallel(self, ctx, items, f)`. `self` is `HostTasks`, an empty struct,
    // so it flattens to nothing; `ctx` is the caller's whole context and is the
    // one this row's fourth column names, because it is dropped from the C call
    // and read into the step's state record instead; `items` is the `[A]` the
    // runtime walks, which is what the strides of [`Extra::Step`] describe; `f`
    // crosses as the entry thunk and the state record rather than as
    // `{ code, env }`.
    //
    // Two of the four arguments carry no bytes across and they are dropped for
    // different reasons: `self` because it is empty, `ctx` because the runtime
    // reads no capability. Only the second is a rule — a `TestTasks` receiver is
    // a live handle and crosses — which is why the column names an index rather
    // than a width.
    //
    // The body is in `cli/runtime/rt.rs` behind feature `net`, which is why
    // `runtime_native::net_intrinsic` names the `host.HostTasks.*` family: a
    // toolchain built without the reactor refuses this key with a sentence
    // before code generation rather than with a missing symbol from `cc`.
    cx(es("host.HostTasks.parallel", "buri_rt_host_tasks_parallel", Ret::Out), 1),
    // -- Listen, and Sockets beside it --------------------------------------
    //
    // Four operations and no closure among them: the accept loop is
    // `core/net/server`'s, in Buri, so nothing here is runtime-driven and none
    // of these rows is an [`Extra::Step`]. That is the whole reason `Listen`
    // costs four ordinary rows where `Tasks` costs one with a trampoline
    // behind it.
    //
    // Every one is `Result<_, ServeError>`, and `ServeError` is a **struct** —
    // so all four take §2.1's *second* shape: the error crosses whole through
    // an out-pointer of its own and the discriminant says only that it failed.
    // `bytes.fromUtf8` is the other row with that shape, and `NetError`'s
    // payload-carrying variants are why `ServeError` was declared a struct
    // rather than a ninth and tenth `NetError` variant.
    //
    // `self` is `HostListen`, an empty struct, so it flattens to nothing and
    // no row here needs a `ctx` column: none of the four takes a context.
    // `listenBind`'s `ListenOptions` and `listenRespond`'s `Response` flatten
    // into their leaves by §2 rule 1, and `Listener` into its two `Int`s.
    //
    // The bodies are in `cli/runtime/net.rs`, which is why
    // `runtime_native::net_intrinsic` names the `host.HostListen.*` family: a
    // toolchain built without the network refuses these keys with a sentence
    // before code generation rather than with a missing symbol from `cc`.
    e("host.HostListen.listenBind", "buri_rt_host_listen_bind", Ret::Res),
    e("host.HostListen.listenAccept", "buri_rt_host_listen_accept", Ret::Res),
    e("host.HostListen.listenRespond", "buri_rt_host_listen_respond", Ret::Res),
    e("host.HostListen.listenClose", "buri_rt_host_listen_close", Ret::Void),
    // The socket half. `()` on all three, because a frame is enqueued rather
    // than delivered and "did this arrive" was never answerable — which is
    // also what makes the runtime's current bodies, which drop what they are
    // handed because nothing hands out a socket yet, the *declared* behaviour
    // for a socket that has gone rather than a stub.
    e(
        "host.HostSockets.socketSendText",
        "buri_rt_host_sockets_socket_send_text",
        Ret::Void,
    ),
    e(
        "host.HostSockets.socketSendBytes",
        "buri_rt_host_sockets_socket_send_bytes",
        Ret::Void,
    ),
    e("host.HostSockets.socketClose", "buri_rt_host_sockets_socket_close", Ret::Void),
    // -- core/alloc's counters ----------------------------------------------
    //
    // Four scalars in, one scalar out, and no context anywhere in them: the
    // handle *is* the allocator here, so these are the one part of the `Alloc`
    // story that needs no argument this ABI drops (`runtime_call`, above).
    // `charge` is the one that can end the process, and it is `Ret::Scalar`
    // rather than `Ret::NoReturn` because it returns on every request that
    // fits.
    e("alloc.newCounter", "buri_rt_alloc_new_counter", Ret::Scalar),
    e("alloc.charge", "buri_rt_alloc_charge", Ret::Scalar),
    e("alloc.count", "buri_rt_alloc_count", Ret::Scalar),
    e("alloc.total", "buri_rt_alloc_total", Ret::Scalar),
    // -- `core/alloc`'s scope (G4) ------------------------------------------
    //
    // The same shape as the four counters above and for the same reason: the
    // handle *is* the arena, so no context reaches these and there is nothing
    // for this ABI to drop. `arenaRelease` answers the bytes it gave back
    // rather than `()`, because a scalar out is the row this table has and
    // `scoped` discards it.
    e("alloc.arenaCreate", "buri_rt_alloc_arena_create", Ret::Scalar),
    e("alloc.arenaAllocate", "buri_rt_alloc_arena_allocate", Ret::Scalar),
    e("alloc.arenaRelease", "buri_rt_alloc_arena_release", Ret::Scalar),
    e("alloc.arenaCount", "buri_rt_alloc_arena_count", Ret::Scalar),
    e("alloc.arenaTotal", "buri_rt_alloc_arena_total", Ret::Scalar),
    // G5's pair: the arena the platform allocator serves out of, for this
    // carrier and for the dynamic extent of `scoped`'s body. `arenaEnter`
    // answers the arena that was active before, so nesting is the caller's
    // local and not a stack in the runtime.
    e("alloc.arenaEnter", "buri_rt_alloc_arena_enter", Ret::Scalar),
    e("alloc.arenaLeave", "buri_rt_alloc_arena_leave", Ret::Scalar),
    // -- core/actor's mailbox, state and reply slots (F6) --------------------
    //
    // Nine rows, and not one of them carries a stride, a glue or a descriptor
    // — which is the whole reason `core/actor` is shaped the way it is. Every
    // value that crosses is a **one-element `[T]`**, and a `[T]` is `ptr` and
    // `len` whatever `T` is (VALUE-MODEL.md §4), so the message, the state and
    // the answer are two words each and the runtime holds them without ever
    // learning what is inside. That is a *fifth* way an erased type can be
    // carried, beside `Extra::Element`'s stride, `Entry::by_ref`'s address,
    // `Func::desc`'s descriptor and `Extra::Step`'s thunk, and
    // `middle/monomorphize.rs`'s `GENERIC_INTRINSICS` is where it is argued.
    //
    // The fourth column is `0` on every row: `core/actor` declares these as
    // `fn <name><C: Tasks, …>(ctx: C, …)` — a module function with the
    // authority in its bound, `core/list`'s shape — so argument 0 is the
    // context and the C signature has no parameter for it.
    //
    // `Ret::Opt` on six of them, and the `.None`s are all one sentence: there
    // is no actor, or there is nothing there yet. The payload is an `Int` for
    // the two that answer a depth and a `[T]` for the four that answer a
    // block; both are written through the trailing out-pointer at `.Some`'s
    // own offset, so a niche `Option<[T]>` is settled by the non-null block
    // pointer the runtime wrote — and `core/actor`'s `Carried<T>` is what
    // guarantees that pointer is non-null, since a zero-stride element would
    // have made the block empty and the niche `.None`.
    //
    // The bodies are in `cli/runtime/rt.rs` behind feature `net`, so
    // `runtime_native::net_intrinsic` names the `actor.*` family too: a
    // toolchain built without the reactor refuses these keys with a sentence
    // before code generation rather than with a missing symbol from `cc`.
    cx(e("actor.mailboxOpen", "buri_rt_actor_mailbox_open", Ret::Scalar), 0),
    cx(e("actor.mailboxPush", "buri_rt_actor_mailbox_push", Ret::Opt), 0),
    cx(e("actor.mailboxPop", "buri_rt_actor_mailbox_pop", Ret::Opt), 0),
    cx(e("actor.mailboxClose", "buri_rt_actor_mailbox_close", Ret::Opt), 0),
    cx(e("actor.stateTake", "buri_rt_actor_state_take", Ret::Opt), 0),
    cx(e("actor.statePut", "buri_rt_actor_state_put", Ret::Opt), 0),
    cx(e("actor.replyOpen", "buri_rt_actor_reply_open", Ret::Scalar), 0),
    cx(e("actor.replyPut", "buri_rt_actor_reply_put", Ret::Opt), 0),
    cx(e("actor.replyTake", "buri_rt_actor_reply_take", Ret::Opt), 0),
    // -- core/host/testing's stateful half -----------------------------------
    //
    // `core/host`'s names for a test source, over one handle table.
    // `cli/runtime/testing.rs`'s header is the argument for these being in the
    // archive rather than open-coded: each names a slot in one mutable table,
    // which is `runtime.js`'s `$t.h` written for a language that has statics.
    //
    // Every **constructor** is `Ret::Out` and not `Ret::Scalar`, and that is
    // the one non-obvious row here. `struct TestStdout(I64)` is a struct, and
    // `middle/layout.rs` gives every struct `Repr::Aggregate` however few
    // fields it has — so the result is an aggregate and §2 rule 2 puts it
    // through an out-pointer. Declaring it as returning one word would agree
    // with the archive by accident on both supported targets and be an ABI
    // disagreement nothing diagnoses.
    //
    // `TestFs`'s eleven methods answer a `Result<T, IoError>`, which was the
    // shape this table had no `Ret` for; §2.1 is that shape and [`Ret::Res`] is
    // the row for it. `host.HostFs.readFile` is still absent, and for a
    // different reason: the archive has a body for it and this table has no
    // row, which is a gap rather than a shape.
    //
    // `alloc` and `TestAlloc.allocate` are open-coded and are named in
    // [`the_unimplemented_surface_is_not_claimed`].
    //
    // `proc` and `TestProc.exitWith` are absent and are not named there either,
    // for `TestNet.fetch`'s reason rather than the allocator's: both are Buri
    // bodies, so no key reaches this table to be missing from it. `TestProc`
    // records nothing because nothing can read it back.
    e("host_testing.stdout", "buri_rt_host_testing_stdout", Ret::Out),
    e("host_testing.stderr", "buri_rt_host_testing_stderr", Ret::Out),
    // The five writers answer `Result<(), IoError>` here too, and always
    // `.Ok(())`: a captured stream is a buffer the runner owns, so there is
    // nothing to fail. The shape is the effect's, not the implementation's.
    e("host_testing.TestStdout.print", "buri_rt_host_testing_test_stdout_print", Ret::Res),
    e("host_testing.TestStdout.println", "buri_rt_host_testing_test_stdout_println", Ret::Res),
    e(
        "host_testing.TestStdout.writeBytes",
        "buri_rt_host_testing_test_stdout_write_bytes",
        Ret::Res,
    ),
    e("host_testing.TestStdout.captured", "buri_rt_host_testing_test_stdout_captured", Ret::Out),
    e("host_testing.TestStderr.eprint", "buri_rt_host_testing_test_stderr_eprint", Ret::Res),
    e("host_testing.TestStderr.eprintln", "buri_rt_host_testing_test_stderr_eprintln", Ret::Res),
    e("host_testing.TestStderr.captured", "buri_rt_host_testing_test_stderr_captured", Ret::Out),
    e("host_testing.stdin", "buri_rt_host_testing_stdin", Ret::Out),
    e("host_testing.TestStdin.lines", "buri_rt_host_testing_test_stdin_lines", Ret::Out),
    e("host_testing.TestStdin.bytes", "buri_rt_host_testing_test_stdin_bytes", Ret::Out),
    e(
        "host_testing.TestStdin.readLine",
        "buri_rt_host_testing_test_stdin_read_line",
        Ret::Opt,
    ),
    e(
        "host_testing.TestStdin.readBytes",
        "buri_rt_host_testing_test_stdin_read_bytes",
        Ret::Opt,
    ),
    // The stream's log, read back. A log is state the runner keeps, so it is
    // here for the reason the handle table itself is.
    e("host_testing.TestStdin.calls", "buri_rt_host_testing_test_stdin_calls", Ret::Out),
    // `TestFs`'s twenty-two, and every one of them takes a **handle** rather
    // than a `TestFs`. That value is a handle and a fault plan since the plan
    // landed, and an argument crosses as its leaves — so a row taking `self`
    // would be handed three values where it expects one, which is the crash
    // `TestNet.calls` found first. The eleven methods of `Fs` are Buri bodies
    // over these rows; `host_testing.buri` says why the plan is in the program.
    //
    // `snapshot` is `Ret::Out` over a `[(Str, Str)]` — one block of two-`Str`
    // elements, which is the layout `str.splitOnce` already writes through an
    // out-pointer, one element wide. The three builders and `newFs` answer an
    // `I64` and so are `Ret::Scalar`, `newNet`'s shape rather than `clock`'s:
    // what they answer is the handle, and the value around it is built in Buri.
    e("host_testing.newFs", "buri_rt_host_testing_new_fs", Ret::Scalar),
    e("host_testing.fsFiles", "buri_rt_host_testing_fs_files", Ret::Scalar),
    e("host_testing.fsFilesBytes", "buri_rt_host_testing_fs_files_bytes", Ret::Scalar),
    e("host_testing.fsReadOnly", "buri_rt_host_testing_fs_read_only", Ret::Scalar),
    e("host_testing.fsRead", "buri_rt_host_testing_fs_read", Ret::Res),
    e("host_testing.fsSnapshot", "buri_rt_host_testing_fs_snapshot", Ret::Out),
    e("host_testing.fsCalls", "buri_rt_host_testing_fs_calls", Ret::Out),
    e("host_testing.fsReadFile", "buri_rt_host_testing_fs_read_file", Ret::Res),
    e("host_testing.fsWriteFile", "buri_rt_host_testing_fs_write_file", Ret::Res),
    e(
        "host_testing.fsFileExists",
        "buri_rt_host_testing_fs_file_exists",
        Ret::Scalar,
    ),
    e("host_testing.fsReadDir", "buri_rt_host_testing_fs_read_dir", Ret::Res),
    e(
        "host_testing.fsReadFileBytes",
        "buri_rt_host_testing_fs_read_file_bytes",
        Ret::Res,
    ),
    e(
        "host_testing.fsWriteFileBytes",
        "buri_rt_host_testing_fs_write_file_bytes",
        Ret::Res,
    ),
    e("host_testing.fsAppendFile", "buri_rt_host_testing_fs_append_file", Ret::Res),
    e("host_testing.fsRenameFile", "buri_rt_host_testing_fs_rename_file", Ret::Res),
    e("host_testing.fsRemoveFile", "buri_rt_host_testing_fs_remove_file", Ret::Res),
    e("host_testing.fsMakeDir", "buri_rt_host_testing_fs_make_dir", Ret::Res),
    e("host_testing.fsSyncFile", "buri_rt_host_testing_fs_sync_file", Ret::Res),
    // -- the fault plan's promise -------------------------------------------
    //
    // The plan itself never crosses. It is a list of Buri values holding an
    // `IoError`, and §2.1 cannot name an error variant that carries a field, so
    // matching is the `Eq` the `Call` records derive and happens in
    // `host_testing.buri`. What crosses is the half a program cannot keep:
    // `fsWithPlan`/`netWithPlan` mint the plan, `addFsFault`/`addNetFault` say
    // what each entry would read like in a failure message, `noteFault` records
    // that one fired, and `test.leave` below reports the rest. `noteFsCall` is
    // the twelfth way into a log: a call the plan failed never reaches the row
    // that would have recorded it, and it is still a call.
    e("host_testing.fsWithPlan", "buri_rt_host_testing_fs_with_plan", Ret::Scalar),
    e("host_testing.addFsFault", "buri_rt_host_testing_add_fs_fault", Ret::Void),
    e("host_testing.addNetFault", "buri_rt_host_testing_add_net_fault", Ret::Void),
    e("host_testing.faultFails", "buri_rt_host_testing_fault_fails", Ret::Void),
    e("host_testing.noteFault", "buri_rt_host_testing_note_fault", Ret::Void),
    e("host_testing.noteFsCall", "buri_rt_host_testing_note_fs_call", Ret::Void),
    e("host_testing.netRebind", "buri_rt_host_testing_net_rebind", Ret::Scalar),
    e("host_testing.netWithPlan", "buri_rt_host_testing_net_with_plan", Ret::Scalar),
    // -- the call log's remaining four --------------------------------------
    //
    // `spelled` is an `FsCall` constructor's decode and not a filesystem
    // operation at all: a test writing a call down performs no effect, so it
    // has no context to reach `bytes.fromUtf8` with.
    //
    // The other three are `TestNet`'s. `net()` and `TestNet.fetch` are Buri
    // bodies and have no row — the absent-key list below says why — but the
    // *log* is state, so the handle naming it is minted here
    // (`alloc.newCounter`'s shape), written by `recordFetch` once the responder
    // has answered, and read back by `netCalls`. `recordFetch` takes `Request`
    // flattened by §2 rule 1, which is `buri_rt_host_net_fetch`'s argument list
    // without its answer; `netCalls` takes the handle rather than the `TestNet`,
    // because that value carries the responder too and an argument crosses as
    // its leaves.
    e("host_testing.spelled", "buri_rt_host_testing_spelled", Ret::Out),
    e("host_testing.newNet", "buri_rt_host_testing_new_net", Ret::Scalar),
    e("host_testing.recordFetch", "buri_rt_host_testing_record_fetch", Ret::Void),
    e("host_testing.netCalls", "buri_rt_host_testing_net_calls", Ret::Out),
    // -- tasks(): the order the work happens in ------------------------------
    //
    // `parallel` is the **second** key of the closure trampoline in this table
    // and the reason the double is worth having: it reaches its steps through
    // the same entry thunk `host.HostTasks.parallel` reaches them through, so a
    // program tested against this is tested against the boundary that ships.
    // `self` is a `TestTasks`, a handle, so it is a scalar where the real one is
    // `Arg::Dropped` — the runtime has to be able to ask which order this run
    // schedules in. The other rows are the ordering builders, the log, and the
    // plan's two halves.
    cx(
        es("host_testing.TestTasks.parallel", "buri_rt_host_testing_test_tasks_parallel", Ret::Out),
        1,
    ),
    e("host_testing.tasks", "buri_rt_host_testing_tasks", Ret::Out),
    e(
        "host_testing.TestTasks.anyOrder",
        "buri_rt_host_testing_test_tasks_any_order",
        Ret::Out,
    ),
    e(
        "host_testing.TestTasks.everyOrder",
        "buri_rt_host_testing_test_tasks_every_order",
        Ret::Out,
    ),
    e("host_testing.TestTasks.seed", "buri_rt_host_testing_test_tasks_seed", Ret::Out),
    e("host_testing.TestTasks.calls", "buri_rt_host_testing_test_tasks_calls", Ret::Out),
    e("host_testing.TestTasks.runs", "buri_rt_host_testing_test_tasks_runs", Ret::Scalar),
    e(
        "host_testing.TestTasks.orders",
        "buri_rt_host_testing_test_tasks_orders",
        Ret::Scalar,
    ),
    e("host_testing.TestTasks.replan", "buri_rt_host_testing_test_tasks_replan", Ret::Out),
    e(
        "host_testing.TestTasks.addFault",
        "buri_rt_host_testing_test_tasks_add_fault",
        Ret::Void,
    ),
    e("host_testing.clock", "buri_rt_host_testing_clock", Ret::Out),
    e("host_testing.TestClock.at", "buri_rt_host_testing_test_clock_at", Ret::Out),
    e(
        "host_testing.TestClock.nowMillis",
        "buri_rt_host_testing_test_clock_now_millis",
        Ret::Scalar,
    ),
    e(
        "host_testing.TestClock.sleepMillis",
        "buri_rt_host_testing_test_clock_sleep_millis",
        Ret::Void,
    ),
    e("host_testing.rand", "buri_rt_host_testing_rand", Ret::Out),
    e("host_testing.TestRand.seed", "buri_rt_host_testing_test_rand_seed", Ret::Out),
    e("host_testing.TestRand.nextInt", "buri_rt_host_testing_test_rand_next_int", Ret::Scalar),
    e(
        "host_testing.TestRand.nextFloat",
        "buri_rt_host_testing_test_rand_next_float",
        Ret::Scalar,
    ),
    e("host_testing.env", "buri_rt_host_testing_env", Ret::Out),
    e("host_testing.TestEnv.variables", "buri_rt_host_testing_test_env_variables", Ret::Out),
    e("host_testing.TestEnv.arguments", "buri_rt_host_testing_test_env_arguments", Ret::Out),
    e("host_testing.TestEnv.variable", "buri_rt_host_testing_test_env_variable", Ret::Opt),
    e("host_testing.TestEnv.args", "buri_rt_host_testing_test_env_args", Ret::Out),
    // The one key here that no Buri declaration produces: `middle::monomorphize`
    // emits it after every `test` body, so that "a fault whose call never
    // happens fails the test" is checked once for all three backends rather than
    // three times in three test-binary entry points. Its twin
    // `buri_rt_test_enter` is called from those entry points instead, because it
    // is the *runner's* protocol — which block to run — and this is the
    // *program's* rule.
    e("test.leave", "buri_rt_test_leave", Ret::Void),
    // The other half of the same lowering, emitted after it: whether to run this
    // body again. `TestTasks.everyOrder` reruns the body once per completion
    // order, and answering yes here is how — the body calls itself, so the
    // reruns are one tree on all three backends rather than a loop in each of
    // three entry points.
    e("test.replay", "buri_rt_test_replay", Ret::Scalar),
];

/// The entry for a key, or `None` where this backend has no body for it.
pub fn entry(key: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::backend::runtime_native::symbol_for;

    /// Every entry's symbol is the one the contract's rule produces, so the
    /// table is a *subset* of the contract rather than a second opinion about
    /// it.
    #[test]
    fn every_symbol_is_the_rule_applied_to_the_key() {
        for entry in ENTRIES {
            assert_eq!(symbol_for(entry.key), entry.symbol, "{}", entry.key);
        }
        assert_eq!(symbol_for("host.HostFs.readFile"), "buri_rt_host_fs_read_file");
        assert_eq!(symbol_for("host.HostStdout.println"), "buri_rt_host_stdout_println");
        assert_eq!(symbol_for("str.splitOnce"), "buri_rt_str_split_once");
    }

    /// A key the archive has no body for must not be in the table. If one of
    /// these gains a symbol, this test is the reminder to add its shape rather
    /// than to let the mangler invent it.
    #[test]
    fn the_unimplemented_surface_is_not_claimed() {
        for absent in [
            "list.map",
            "list.fold",
            "list.sortBy",
            "list.zip",
            "list.flatten",
            "json.decode",
            // `cli/runtime/host.rs` has a body for every one of these, and
            // this backend still has no row: the gap is the row, not the
            // body. Not a *shape* either — `TestFs`'s are the same
            // `Result<T, IoError>` and are in the table above — which is the
            // distinction this list exists to keep.
            "host.HostFs.readFile",
            "host.HostFs.writeFile",
            "host.HostFs.readDir",
            "host.HostFs.readFileBytes",
            "host.HostFs.writeFileBytes",
            "host.HostFs.appendFile",
            "host.HostFs.renameFile",
            "host.HostFs.removeFile",
            "host.HostFs.makeDir",
            "host.HostFs.syncFile",
            // `host.HostNet.fetch` is absent for a *third* reason, and it is
            // the one this list exists to distinguish. The archive has a body
            // (`cli/runtime/host.rs`), and the shape is not merely missing: it
            // is not expressible. `Ret::Res` names the error variant by index
            // and §2.1 restricts that variant to carrying no fields, while
            // `NetError` carries a `Str` on `BadUrl` and on `Transport`. A row
            // here needs §2.1 widened first, not a `Ret` picked from the ones
            // that exist.
            "host.HostNet.fetch",
            // `core/host/testing`'s `net()` answers the same shape and needs
            // no row at all: `TestNet` carries its responder as a value and
            // `TestNet.fetch` is a Buri body that calls it, so no key is
            // produced for it here. That is the same wall read from the other
            // side — a responder is a `{ code, env }` pair the archive has no
            // way to invoke, and its answer is the `Result<Response, NetError>`
            // §2.1 cannot name — and it is why widening §2.1 later changes
            // nothing about the double. Its *log* is a different question and
            // has five rows above: `newNet`, `netRebind`, `netWithPlan`,
            // `recordFetch` and `netCalls` cross nothing §2.1 restricts. So does
            // its **fault plan**, which is the same wall a third time: the plan
            // is a list of Buri values holding a `NetError`, so it stays in the
            // program and only its rendering and its fired flags cross.
            //
            // `host_testing.fs` is absent for a different reason and is not a
            // gap: `fs()` is a Buri body too now, because a `TestFs` is a handle
            // and a plan. `newFs` is the row that mints the handle.
            // Open-coded, and named here so that "it has no symbol" and "the
            // backend cannot compile it" stay two different statements: the
            // allocator is two instructions on both native backends.
            "host_testing.alloc",
            "host_testing.TestAlloc.allocate",
            // Buri bodies, for the reason two paragraphs up.
            "host_testing.fs",
            "host_testing.TestFs.readFile",
            "host_testing.TestFs.faults",
            // `TestTasks.faults` is a Buri body over `replan` and `addFault`,
            // for the reason its two twins are: a plan is walked one entry at a
            // time, and the walk is the program's.
            "host_testing.faults",
            "host_testing.TestTasks.faults",
        ] {
            assert!(entry(absent).is_none(), "{absent}");
        }
    }

    /// `str.concat` has a body in the archive and deliberately no row, which
    /// is the one case where those two are not the same question. [`ENTRIES`]'
    /// own comment is the reason; this is the assertion that it stays true in
    /// both directions.
    #[test]
    fn str_concat_has_a_symbol_and_no_row() {
        assert_eq!(symbol_for("str.concat"), "buri_rt_str_concat");
        assert!(entry("str.concat").is_none());
    }

    /// `host.HostNet.fetch`, in both directions.
    ///
    /// The archive exports exactly the symbol the mangling rule produces — so
    /// a row added later needs no invention — and this table has no row for
    /// it, because `NetError`'s two payload-carrying variants put it outside
    /// `lib.rs` §2.1's `Result` shape. Asserted as a pair so that "the body
    /// exists" and "the backend can call it" stay two separate claims, exactly
    /// as `str.concat`'s pair does one row above.
    #[test]
    fn host_net_fetch_has_a_symbol_and_no_row() {
        assert_eq!(symbol_for("host.HostNet.fetch"), "buri_rt_host_net_fetch");
        assert!(entry("host.HostNet.fetch").is_none());
    }

    /// The module a key's first segment names, for the keys whose operations
    /// are *declared* in Buri.
    ///
    /// `test.leave` and `test.replay` are absent because they are not: both
    /// runner hooks are built by `middle::monomorphize::leaving` and no
    /// declaration spells them, which both runtime tables already say.
    const DECLARED_IN: &[(&str, &str)] = &[
        ("actor", "core/actor"),
        ("alloc", "core/alloc"),
        ("bytes", "core/bytes"),
        ("char", "core/char"),
        ("host", "core/host"),
        ("host_testing", "core/host/testing"),
        ("list", "core/list"),
        ("math", "core/math"),
        ("str", "core/str"),
    ];

    /// Every `fn <name>` in `source`, answered as the index of its `ctx`
    /// parameter — `None` where it has none.
    ///
    /// A set rather than one answer because the name is looked up without its
    /// owner: `Str.repeat` and `list.repeat` are two declarations of `repeat`,
    /// and they are in two modules, so within one module the answers agree or
    /// the lookup was not specific enough to assert on. The caller checks that.
    fn declared_ctx(source: &str, name: &str) -> Vec<Option<usize>> {
        let mut out = Vec::new();
        for (at, _) in source.match_indices("fn ") {
            // `asfn foo` is not a declaration of `foo`.
            if source
                .get(..at)
                .and_then(|before| before.chars().next_back())
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                continue;
            }
            let Some(rest) = source.get(at.saturating_add(3)..) else { continue };
            let Some(tail) = rest.strip_prefix(name) else { continue };
            // `fn splitOnce` must not answer for `fn split`, so the character
            // after the name has to end it.
            if tail.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            // Past the generics, if any, to the parameter list's own `(`.
            let Some(open) = tail.find('(') else { continue };
            let Some(head) = tail.get(..open) else { continue };
            if head.contains(')') || head.contains('{') {
                continue;
            }
            let Some(after) = tail.get(open.saturating_add(1)..) else { continue };
            let mut depth = 0usize;
            let mut params: Vec<String> = Vec::new();
            let mut piece = String::new();
            for c in after.chars() {
                match c {
                    // `>` closes a generic argument list and also ends the `=>`
                    // of a function type; the saturating decrement is what lets
                    // one loop read both without counting arrows.
                    '(' | '[' | '<' => {
                        depth = depth.saturating_add(1);
                        piece.push(c);
                    }
                    ')' if depth == 0 => break,
                    ')' | ']' | '>' => {
                        depth = depth.saturating_sub(1);
                        piece.push(c);
                    }
                    ',' if depth == 0 => params.push(std::mem::take(&mut piece)),
                    _ => piece.push(c),
                }
            }
            if !piece.trim().is_empty() {
                params.push(piece);
            }
            out.push(
                params.iter().position(|p| p.split(':').next().is_some_and(|n| n.trim() == "ctx")),
            );
        }
        out
    }

    /// [`Entry::ctx`] is the index of the **declaration's** `ctx` parameter,
    /// checked against the declaration rather than against a second list.
    ///
    /// This is the test that would have caught the bug the column exists for.
    /// The rule it replaced asked the *argument's type* — "is it a `Ty::Ctx`?"
    /// — which is the same answer only while every `C: Alloc` is instantiated
    /// at a `context { … }` record; a value that merely implements `Alloc`
    /// satisfies the bound (SPEC 10.1, 10.8) and slipped through as an extra
    /// leaf. A column can be wrong the same way a type test was, so it is
    /// derived here from the one place that cannot be: the signature.
    #[test]
    fn the_context_column_is_the_declarations_ctx_parameter() {
        let module = |path: &str| {
            crate::compiler::standard_library::MODULES
                .iter()
                .find(|m| m.path == path)
                .map(|m| m.source)
        };
        let mut checked = 0usize;
        for entry in ENTRIES {
            let Some((_, path)) = DECLARED_IN
                .iter()
                .find(|(prefix, _)| entry.key.split('.').next() == Some(prefix))
            else {
                continue;
            };
            let source = module(path).unwrap_or_else(|| panic!("no module at {path}"));
            let name = entry.key.rsplit('.').next().unwrap_or(entry.key);
            let found = declared_ctx(source, name);
            // `str.eq` and `str.hash` are `semantics/builtins.rs`'s, declared
            // on every primitive rather than written in `core/str` — so there
            // is nothing here to read, and neither takes a context.
            if found.is_empty() {
                assert_eq!(entry.ctx, None, "{} has no declaration and a ctx column", entry.key);
                continue;
            }
            let first = found.first().copied().flatten();
            assert!(
                found.iter().all(|a| *a == found[0]),
                "{}: two declarations of `{name}` in {path} disagree about `ctx`",
                entry.key
            );
            assert_eq!(entry.ctx, first, "{}", entry.key);
            checked += 1;
        }
        // A scan that matched nothing would pass every assertion above.
        assert!(checked > 140, "only {checked} rows were read against a declaration");
        // Twenty-nine until F6, and the nine `core/actor` rows are the jump:
        // every one of them is a module function whose first parameter is the
        // context, which is the second of the two shapes below.
        assert_eq!(ENTRIES.iter().filter(|e| e.ctx.is_some()).count(), 38);
    }

    /// The two shapes the column takes, by example, so that the indices are
    /// legible without opening `core/list`.
    #[test]
    fn a_receiver_shifts_the_context_by_one() {
        assert_eq!(entry("list.push").and_then(|e| e.ctx), Some(1));
        assert_eq!(entry("list.repeat").and_then(|e| e.ctx), Some(0));
        assert_eq!(entry("str.fromInt").and_then(|e| e.ctx), Some(0));
        assert_eq!(entry("str.split").and_then(|e| e.ctx), Some(1));
        // `get` takes no context — it allocates nothing.
        assert_eq!(entry("list.get").and_then(|e| e.ctx), None);
        assert_eq!(entry("host.HostStdout.println").and_then(|e| e.ctx), None);
    }

    /// No two rows may claim the same key: `entry` answers the first, so a
    /// duplicate would be a row that silently never runs.
    #[test]
    fn no_key_appears_twice() {
        let mut keys: Vec<&str> = ENTRIES.iter().map(|e| e.key).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "a key is in the table twice");
    }
}
