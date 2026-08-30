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
//!   3. the out-pointer                                   (lib.rs §2 rules 2, 3)
//! ```
//!
//! Step 1 needs no table entry at all: a `Str` argument spreads to three
//! values because a `Str` *is* three leaves, and a zero-sized `ctx` spreads to
//! none because it occupies no bytes. That is why there is no per-argument
//! column here — the IR is the argument list, and a second description of it
//! would be a thing to disagree with.
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
//!   no row here.
//!
//! An intrinsic that is generic and has none of the three is a miscompile with
//! no diagnostic, so the set of keys allowed to be generic is a written list —
//! `GENERIC_INTRINSICS` in `middle/monomorphize.rs` — and a generic intrinsic
//! outside it is an internal error at monomorphization. **A new row here for a
//! generic key needs a row there too**, and the question that list is asking is
//! which of the three above carries the type.
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
    /// `MemFs.writeFile`'s `Result<(), IoError>`. A parameter for a value that
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
}

const fn e(key: &'static str, symbol: &'static str, ret: Ret) -> Entry {
    Entry { key, symbol, extra: Extra::None, ret, by_ref: None }
}

const fn el(key: &'static str, symbol: &'static str, ret: Ret) -> Entry {
    Entry { key, symbol, extra: Extra::Element, ret, by_ref: None }
}

const fn er(key: &'static str, symbol: &'static str, ret: Ret, by_ref: usize) -> Entry {
    Entry { key, symbol, extra: Extra::Element, ret, by_ref: Some(by_ref) }
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
    e("str.split", "buri_rt_str_split", Ret::Out),
    e("str.splitAny", "buri_rt_str_split_any", Ret::Out),
    e("str.lines", "buri_rt_str_lines", Ret::Out),
    e("str.replace", "buri_rt_str_replace", Ret::Out),
    e("str.repeat", "buri_rt_str_repeat", Ret::Out),
    e("str.toUpper", "buri_rt_str_to_upper", Ret::Out),
    e("str.toLower", "buri_rt_str_to_lower", Ret::Out),
    e("str.chars", "buri_rt_str_chars", Ret::Out),
    e("str.fromChars", "buri_rt_str_from_chars", Ret::Out),
    e("str.fromInt", "buri_rt_str_from_int", Ret::Out),
    e("str.fromFloat", "buri_rt_str_from_float", Ret::Out),
    e("str.padStart", "buri_rt_str_pad_start", Ret::Out),
    e("str.padEnd", "buri_rt_str_pad_end", Ret::Out),
    // -- core/list ----------------------------------------------------------
    //
    // `len` is open-coded (it is a load) and every entry taking a closure is
    // absent; `cli/runtime/list.rs`'s header says which and why.
    el("list.get", "buri_rt_list_get", Ret::Opt),
    el("list.concat", "buri_rt_list_concat", Ret::Out),
    // `push(self, ctx, item)` — the item is a `T`, so it goes by address.
    er("list.push", "buri_rt_list_push", Ret::Out, 2),
    el("list.reverse", "buri_rt_list_reverse", Ret::Out),
    el("list.slice", "buri_rt_list_slice", Ret::Out),
    el("list.take", "buri_rt_list_take", Ret::Out),
    el("list.drop", "buri_rt_list_drop", Ret::Out),
    // `repeat(ctx, item, times)` — likewise, one place earlier.
    er("list.repeat", "buri_rt_list_repeat", Ret::Out, 1),
    e("list.range", "buri_rt_list_range", Ret::Out),
    e("list.join", "buri_rt_list_join", Ret::Out),
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
    e("bytes.toUtf8", "buri_rt_bytes_to_utf8", Ret::Out),
    // `Result<Str, Utf8Error>` — §2.1's *second* error shape. `Utf8Error(Int)`
    // is a struct, so there is no variant index to name it with and the value
    // crosses through its own out-pointer.
    e("bytes.fromUtf8", "buri_rt_bytes_from_utf8", Ret::Res),
    e("bytes.f64ToBytes", "buri_rt_bytes_f64_to_bytes", Ret::Out),
    e("bytes.f64FromBytes", "buri_rt_bytes_f64_from_bytes", Ret::Opt),
    e("bytes.f32ToBytes", "buri_rt_bytes_f32_to_bytes", Ret::Out),
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
    e("host.HostStdout.print", "buri_rt_host_stdout_print", Ret::Void),
    e("host.HostStdout.println", "buri_rt_host_stdout_println", Ret::Void),
    e("host.HostStdout.writeBytes", "buri_rt_host_stdout_write_bytes", Ret::Void),
    e("host.HostStderr.eprint", "buri_rt_host_stderr_eprint", Ret::Void),
    e("host.HostStderr.eprintln", "buri_rt_host_stderr_eprintln", Ret::Void),
    // -- the scalar capabilities --------------------------------------------
    e("host.HostFs.fileExists", "buri_rt_host_fs_file_exists", Ret::Scalar),
    e("host.HostClock.nowMillis", "buri_rt_host_clock_now_millis", Ret::Scalar),
    e("host.HostClock.sleepMillis", "buri_rt_host_clock_sleep_millis", Ret::Void),
    e("host.HostRand.nextInt", "buri_rt_host_rand_next_int", Ret::Scalar),
    e("host.HostRand.nextFloat", "buri_rt_host_rand_next_float", Ret::Scalar),
    e("host.HostProc.exitWith", "buri_rt_host_proc_exit_with", Ret::NoReturn),
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
    // -- core/testing/context's stateful half ---------------------------------
    //
    // `cli/runtime/testing.rs`'s header is the argument for these being in the
    // archive rather than open-coded: each names a slot in one mutable table,
    // which is `runtime.js`'s `$t.h` written for a language that has statics.
    //
    // Every **constructor** is `Ret::Out` and not `Ret::Scalar`, and that is
    // the one non-obvious row here. `struct CaptureOut(I64)` is a struct, and
    // `middle/layout.rs` gives every struct `Repr::Aggregate` however few
    // fields it has — so the result is an aggregate and §2 rule 2 puts it
    // through an out-pointer. Declaring it as returning one word would agree
    // with the archive by accident on both supported targets and be an ABI
    // disagreement nothing diagnoses.
    //
    // `alloc` and `TestAlloc.allocate` are **not** here: they read no state and
    // both backends open-code them (`emit.rs`).
    //
    // `MemFs`'s eleven **are** here now. Most answer a `Result<T, IoError>`,
    // which was the shape this table had no `Ret` for and the reason the
    // original four were held back; §2.1 is that shape and [`Ret::Res`] is the
    // row for it. `host.HostFs.readFile` is still absent, and for a different
    // reason: the archive has a body for it and this table has no row, which is
    // a gap rather than a shape.
    e("testing_context.captureOut", "buri_rt_testing_context_capture_out", Ret::Out),
    e("testing_context.captureErr", "buri_rt_testing_context_capture_err", Ret::Out),
    e("testing_context.CaptureOut.print", "buri_rt_testing_context_capture_out_print", Ret::Void),
    e(
        "testing_context.CaptureOut.println",
        "buri_rt_testing_context_capture_out_println",
        Ret::Void,
    ),
    e(
        "testing_context.CaptureOut.writeBytes",
        "buri_rt_testing_context_capture_out_write_bytes",
        Ret::Void,
    ),
    e(
        "testing_context.CaptureOut.captured",
        "buri_rt_testing_context_capture_out_captured",
        Ret::Out,
    ),
    e("testing_context.CaptureErr.eprint", "buri_rt_testing_context_capture_err_eprint", Ret::Void),
    e(
        "testing_context.CaptureErr.eprintln",
        "buri_rt_testing_context_capture_err_eprintln",
        Ret::Void,
    ),
    e(
        "testing_context.CaptureErr.capturedErr",
        "buri_rt_testing_context_capture_err_captured_err",
        Ret::Out,
    ),
    e("testing_context.stdin", "buri_rt_testing_context_stdin", Ret::Out),
    e("testing_context.stdinBytes", "buri_rt_testing_context_stdin_bytes", Ret::Out),
    e(
        "testing_context.TestStdin.readLine",
        "buri_rt_testing_context_test_stdin_read_line",
        Ret::Opt,
    ),
    e(
        "testing_context.TestStdin.readBytes",
        "buri_rt_testing_context_test_stdin_read_bytes",
        Ret::Opt,
    ),
    e("testing_context.data", "buri_rt_testing_context_data", Ret::Out),
    e("testing_context.files", "buri_rt_testing_context_files", Ret::Out),
    e("testing_context.filesBytes", "buri_rt_testing_context_files_bytes", Ret::Out),
    e(
        "testing_context.MemFs.readFile",
        "buri_rt_testing_context_mem_fs_read_file",
        Ret::Res,
    ),
    // `Result<(), IoError>` — `Ret::Res` with no out-pointer, because `()`
    // occupies no bytes.
    e(
        "testing_context.MemFs.writeFile",
        "buri_rt_testing_context_mem_fs_write_file",
        Ret::Res,
    ),
    e(
        "testing_context.MemFs.fileExists",
        "buri_rt_testing_context_mem_fs_file_exists",
        Ret::Scalar,
    ),
    e(
        "testing_context.MemFs.readDir",
        "buri_rt_testing_context_mem_fs_read_dir",
        Ret::Res,
    ),
    // The seven `core/fs` grew for issue #1. `Extra::None` at the three taking
    // a `[U8]`, for `core/bytes`' reason: the element type is fixed at `U8`, so
    // there is no `T` for rule 4's stride-and-glue pair to describe.
    e(
        "testing_context.MemFs.readFileBytes",
        "buri_rt_testing_context_mem_fs_read_file_bytes",
        Ret::Res,
    ),
    e(
        "testing_context.MemFs.writeFileBytes",
        "buri_rt_testing_context_mem_fs_write_file_bytes",
        Ret::Res,
    ),
    e(
        "testing_context.MemFs.appendFile",
        "buri_rt_testing_context_mem_fs_append_file",
        Ret::Res,
    ),
    e(
        "testing_context.MemFs.renameFile",
        "buri_rt_testing_context_mem_fs_rename_file",
        Ret::Res,
    ),
    e(
        "testing_context.MemFs.removeFile",
        "buri_rt_testing_context_mem_fs_remove_file",
        Ret::Res,
    ),
    e(
        "testing_context.MemFs.makeDir",
        "buri_rt_testing_context_mem_fs_make_dir",
        Ret::Res,
    ),
    e(
        "testing_context.MemFs.syncFile",
        "buri_rt_testing_context_mem_fs_sync_file",
        Ret::Res,
    ),
    e("testing_context.clockAt", "buri_rt_testing_context_clock_at", Ret::Out),
    e(
        "testing_context.TestClock.nowMillis",
        "buri_rt_testing_context_test_clock_now_millis",
        Ret::Scalar,
    ),
    e(
        "testing_context.TestClock.sleepMillis",
        "buri_rt_testing_context_test_clock_sleep_millis",
        Ret::Void,
    ),
    e("testing_context.TestClock.advance", "buri_rt_testing_context_test_clock_advance", Ret::Void),
    e("testing_context.randSeed", "buri_rt_testing_context_rand_seed", Ret::Out),
    e(
        "testing_context.TestRand.nextInt",
        "buri_rt_testing_context_test_rand_next_int",
        Ret::Scalar,
    ),
    e(
        "testing_context.TestRand.nextFloat",
        "buri_rt_testing_context_test_rand_next_float",
        Ret::Scalar,
    ),
    e("testing_context.envOf", "buri_rt_testing_context_env_of", Ret::Out),
    e(
        "testing_context.TestEnv.variable",
        "buri_rt_testing_context_test_env_variable",
        Ret::Opt,
    ),
    e(
        "testing_context.TestEnv.arguments",
        "buri_rt_testing_context_test_env_arguments",
        Ret::Out,
    ),
    // -- core/host/testing --------------------------------------------------
    //
    // `core/host`'s names for a test source, over the same handle table. The
    // shapes are `testing_context`'s shapes: the difference between the two
    // modules is in the *Buri* surface — a constructor takes no arguments and a
    // builder answers a new handle — and a shape column cannot see that.
    //
    // `alloc` and `TestAlloc.allocate` are open-coded, as
    // `testing_context`'s are, and are named in
    // [`the_unimplemented_surface_is_not_claimed`] for the same reason.
    e("host_testing.stdout", "buri_rt_host_testing_stdout", Ret::Out),
    e("host_testing.stderr", "buri_rt_host_testing_stderr", Ret::Out),
    e("host_testing.TestStdout.print", "buri_rt_host_testing_test_stdout_print", Ret::Void),
    e("host_testing.TestStdout.println", "buri_rt_host_testing_test_stdout_println", Ret::Void),
    e(
        "host_testing.TestStdout.writeBytes",
        "buri_rt_host_testing_test_stdout_write_bytes",
        Ret::Void,
    ),
    e("host_testing.TestStdout.captured", "buri_rt_host_testing_test_stdout_captured", Ret::Out),
    e("host_testing.TestStderr.eprint", "buri_rt_host_testing_test_stderr_eprint", Ret::Void),
    e("host_testing.TestStderr.eprintln", "buri_rt_host_testing_test_stderr_eprintln", Ret::Void),
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
    // `TestFs`'s seventeen. The eleven the `Fs` effect declares are `MemFs`'s
    // shapes, and the six above them are this module's: two builders, the
    // attenuator, and the three read-backs a test asserts through.
    //
    // `snapshot` is `Ret::Out` over a `[(Str, Str)]` — one block of two-`Str`
    // elements, which is the layout `str.splitOnce` already writes through an
    // out-pointer, one element wide.
    e("host_testing.fs", "buri_rt_host_testing_fs", Ret::Out),
    e("host_testing.TestFs.files", "buri_rt_host_testing_test_fs_files", Ret::Out),
    e("host_testing.TestFs.filesBytes", "buri_rt_host_testing_test_fs_files_bytes", Ret::Out),
    e("host_testing.TestFs.readOnly", "buri_rt_host_testing_test_fs_read_only", Ret::Out),
    e("host_testing.TestFs.read", "buri_rt_host_testing_test_fs_read", Ret::Res),
    e("host_testing.TestFs.snapshot", "buri_rt_host_testing_test_fs_snapshot", Ret::Out),
    e("host_testing.TestFs.calls", "buri_rt_host_testing_test_fs_calls", Ret::Out),
    e("host_testing.TestFs.readFile", "buri_rt_host_testing_test_fs_read_file", Ret::Res),
    e("host_testing.TestFs.writeFile", "buri_rt_host_testing_test_fs_write_file", Ret::Res),
    e(
        "host_testing.TestFs.fileExists",
        "buri_rt_host_testing_test_fs_file_exists",
        Ret::Scalar,
    ),
    e("host_testing.TestFs.readDir", "buri_rt_host_testing_test_fs_read_dir", Ret::Res),
    e(
        "host_testing.TestFs.readFileBytes",
        "buri_rt_host_testing_test_fs_read_file_bytes",
        Ret::Res,
    ),
    e(
        "host_testing.TestFs.writeFileBytes",
        "buri_rt_host_testing_test_fs_write_file_bytes",
        Ret::Res,
    ),
    e("host_testing.TestFs.appendFile", "buri_rt_host_testing_test_fs_append_file", Ret::Res),
    e("host_testing.TestFs.renameFile", "buri_rt_host_testing_test_fs_rename_file", Ret::Res),
    e("host_testing.TestFs.removeFile", "buri_rt_host_testing_test_fs_remove_file", Ret::Res),
    e("host_testing.TestFs.makeDir", "buri_rt_host_testing_test_fs_make_dir", Ret::Res),
    e("host_testing.TestFs.syncFile", "buri_rt_host_testing_test_fs_sync_file", Ret::Res),
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
    e("host_testing.TestEnv.args", "buri_rt_host_testing_test_env_args", Ret::Out),
    e("host_testing.TestEnv.variable", "buri_rt_host_testing_test_env_variable", Ret::Opt),
    e("host_testing.TestEnv.arguments", "buri_rt_host_testing_test_env_arguments", Ret::Out),
    e("host_testing.proc", "buri_rt_host_testing_proc", Ret::Out),
    e("host_testing.TestProc.exitWith", "buri_rt_host_testing_test_proc_exit_with", Ret::Void),
    e("host_testing.TestProc.exited", "buri_rt_host_testing_test_proc_exited", Ret::Opt),
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
            // body. Not a *shape* either — `MemFs`'s are the same
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
            // has three rows above: `newNet`, `recordFetch` and
            // `netCalls` cross nothing §2.1 restricts.
            // Open-coded, and named here so that "it has no symbol" and "the
            // backend cannot compile it" stay two different statements.
            "testing_context.alloc",
            "testing_context.TestAlloc.allocate",
            // `core/host/testing`'s allocator is the same two instructions and
            // is open-coded the same way.
            "host_testing.alloc",
            "host_testing.TestAlloc.allocate",
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
