//! The `buri_rt_*` table the frame-threaded backends share: which intrinsic
//! keys have a symbol, and what shape the call has.
//!
//! `cli/runtime/lib.rs`'s module comment is the contract; this is the
//! transcription of it that Cranelift's and the copy-and-patch backend's
//! generated code is emitted against. It is one table because a table
//! describes `cli/runtime`, which is one library — two transcriptions of one
//! library disagreeing about a row is one of them being wrong, and the two
//! were kept in step by hand until they were this file.
//!
//! The LLVM backend keeps its own (`llvm/runtime.rs`), and that one is a
//! different table rather than a third copy: it reconstructs the C argument
//! list from the row, so every entry there carries an `args` column that the
//! two backends below have no use for. The two tables also do not name quite
//! the same keys, which is a fact about what each backend has implemented and
//! not a naming convention.
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
//! Steps 2 and 3 are what [`Extra`] and [`Ret`] select. What each backend
//! supplies for itself is where an argument *is*: Cranelift builds a value
//! list, and the copy-and-patch backend's convention is frame-threaded, so a
//! leaf there is a byte offset copied into a scratch area (`stencil/rtcall.rs`).
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
    // `MemFs`'s four **are** here now. Three answer a `Result<T, IoError>`,
    // which was the shape this table had no `Ret` for and the reason all four
    // were held back; §2.1 is that shape and [`Ret::Res`] is the row for it.
    // `host.HostFs.readFile` is still absent, and for a different reason: the
    // archive has no body for it (`cli/runtime/host.rs`), so it is a gap rather
    // than a shape.
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
            // The archive has no body for these: `core/fs`'s real filesystem
            // is `cli/runtime/host.rs`'s business and it stops at
            // `fileExists`. Not a *shape* — `MemFs`'s three are the same
            // `Result<T, IoError>` and are in the table above — which is the
            // distinction this list exists to keep.
            "host.HostFs.readFile",
            "host.HostFs.writeFile",
            "host.HostFs.readDir",
            // Open-coded, and named here so that "it has no symbol" and "the
            // backend cannot compile it" stay two different statements.
            "testing_context.alloc",
            "testing_context.TestAlloc.allocate",
        ] {
            assert!(entry(absent).is_none(), "{absent}");
        }
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
