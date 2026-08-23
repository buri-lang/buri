//! The `buri_rt_*` boundary for this backend: which intrinsic keys have a
//! symbol, what shape the call has, and how an `Option` reaches its slot.
//!
//! `cli/runtime/lib.rs`'s module comment is the contract. This is the third
//! transcription of it — `cranelift/runtime.rs` and `llvm/runtime.rs` are the
//! other two — and it is a third table rather than a shared one for the reason
//! wave 2b gave for the second: what a backend has to know about an entry is
//! *how it emits the call*, and the three differ. What must not differ is the
//! set of keys, and `the_three_tables_name_the_same_keys` in
//! `cli/tests/native/` is what says so.
//!
//! # The one emission rule
//!
//! Identical to the other two, in this order and with nothing conditional in it
//! but the row:
//!
//! ```text
//!   1. the Buri arguments, flattened into scalar leaves  (lib.rs §2 rule 1)
//!   2. the element pair — stride, then retain glue       (lib.rs §2 rule 4)
//!   3. the out-pointer                                   (lib.rs §2 rules 2, 3)
//! ```
//!
//! What is different here is only where the arguments *are*: this backend's
//! calling convention is frame-threaded, so a leaf is a byte offset rather than
//! a register, and `emit::Lower::rt_call` copies them into the scratch area one
//! `crt` stencil then reads (`sources.rs::runtime_calls`).
//!
//! # Why a table and not a mangling
//!
//! Unchanged from the other two: the rule in `lib.rs` §1 would happily produce
//! `buri_rt_list_map` for `list.map`, which does not exist, and a program using
//! it would get a link error naming a symbol instead of
//! [`super::Stencil::missing_intrinsics`] naming the operation.

use crate::compiler::middle::layout::{EnumRepr, Layout, Repr};

/// The discriminant a fallible runtime entry returns for its success arm.
///
/// `cli/runtime/lib.rs`'s `BURI_OK`. Restated here rather than shared for the
/// reason `cranelift/runtime.rs` gives: the compiler and the runtime are two
/// crates that never link against each other.
pub const BURI_OK: i32 = -1;

/// What the backend appends after the flattened Buri arguments.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Extra {
    None,
    /// The element **stride** and the per-element **retain glue** of the `[T]`
    /// this call operates on (`lib.rs` §2 rule 4).
    Element,
}

/// What comes back.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ret {
    /// Nothing.
    Void,
    /// One scalar, already at the destination's register shape.
    Scalar,
    /// One `i32`, narrowed to the destination's own tag width.
    Tag,
    /// An aggregate, written through a trailing out-pointer.
    Out,
    /// An `Option<T>`: an `i32` discriminant, and the payload through a
    /// trailing out-pointer offset to `.Some`'s payload.
    Opt,
    /// A `Result<T, E>`: an `i32` discriminant, `.Ok`'s payload through a
    /// trailing out-pointer, and an error variant named by its index.
    Res,
    /// The call does not come back (SPEC 6.10).
    NoReturn,
}

/// One runtime entry this backend can emit a call to.
pub struct Entry {
    /// The intrinsic key `monomorphize` built: `str.slice`.
    pub key: &'static str,
    /// The exported symbol, per `cli/runtime/lib.rs` §1.
    pub symbol: &'static str,
    pub extra: Extra,
    pub ret: Ret,
    /// The argument index that crosses **by address** rather than flattened: a
    /// `T` whose width the runtime learns from the element stride instead of
    /// from a type (`lib.rs` §2 rule 4).
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

/// The entry a key names, or `None`.
pub fn entry(key: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.key == key)
}

/// Every key this backend has a runtime body for.
///
/// The rows are `cranelift/runtime.rs`'s, key for key and shape for shape: the
/// table describes `cli/runtime`, which is one library, so two backends
/// disagreeing about a row would be one of them being wrong. That file carries
/// the reasoning for why each group is present and what is deliberately absent
/// from it; repeating it here would be two places for it to drift.
pub const ENTRIES: &[Entry] = &[
    // -- core/str, pure -----------------------------------------------------
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
    el("list.get", "buri_rt_list_get", Ret::Opt),
    el("list.concat", "buri_rt_list_concat", Ret::Out),
    er("list.push", "buri_rt_list_push", Ret::Out, 2),
    el("list.reverse", "buri_rt_list_reverse", Ret::Out),
    el("list.slice", "buri_rt_list_slice", Ret::Out),
    el("list.take", "buri_rt_list_take", Ret::Out),
    el("list.drop", "buri_rt_list_drop", Ret::Out),
    er("list.repeat", "buri_rt_list_repeat", Ret::Out, 1),
    e("list.range", "buri_rt_list_range", Ret::Out),
    e("list.join", "buri_rt_list_join", Ret::Out),
    // -- core/bytes ---------------------------------------------------------
    e("bytes.toUtf8", "buri_rt_bytes_to_utf8", Ret::Out),
    e("bytes.fromUtf8", "buri_rt_bytes_from_utf8", Ret::Res),
    e("bytes.f64ToBytes", "buri_rt_bytes_f64_to_bytes", Ret::Out),
    e("bytes.f64FromBytes", "buri_rt_bytes_f64_from_bytes", Ret::Opt),
    e("bytes.f32ToBytes", "buri_rt_bytes_f32_to_bytes", Ret::Out),
    e("bytes.f32FromBytes", "buri_rt_bytes_f32_from_bytes", Ret::Opt),
    // -- core/char ----------------------------------------------------------
    e("char.isDigit", "buri_rt_char_is_digit", Ret::Scalar),
    e("char.isAlpha", "buri_rt_char_is_alpha", Ret::Scalar),
    e("char.isSpace", "buri_rt_char_is_space", Ret::Scalar),
    e("char.isUpper", "buri_rt_char_is_upper", Ret::Scalar),
    e("char.isLower", "buri_rt_char_is_lower", Ret::Scalar),
    e("char.toUpper", "buri_rt_char_to_upper", Ret::Scalar),
    e("char.toLower", "buri_rt_char_to_lower", Ret::Scalar),
    e("char.toDigit", "buri_rt_char_to_digit", Ret::Opt),
    // -- core/math, the exactly-specified half --------------------------------
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
    e("alloc.newCounter", "buri_rt_alloc_new_counter", Ret::Scalar),
    e("alloc.charge", "buri_rt_alloc_charge", Ret::Scalar),
    e("alloc.count", "buri_rt_alloc_count", Ret::Scalar),
    e("alloc.total", "buri_rt_alloc_total", Ret::Scalar),
    // -- core/testing/context's stateful half ---------------------------------
    e("testing_context.captureOut", "buri_rt_testing_context_capture_out", Ret::Out),
    e("testing_context.captureErr", "buri_rt_testing_context_capture_err", Ret::Out),
    e("testing_context.CaptureOut.print", "buri_rt_testing_context_capture_out_print", Ret::Void),
    e("testing_context.CaptureOut.println", "buri_rt_testing_context_capture_out_println", Ret::Void),
    e("testing_context.CaptureOut.writeBytes", "buri_rt_testing_context_capture_out_write_bytes", Ret::Void),
    e("testing_context.CaptureOut.captured", "buri_rt_testing_context_capture_out_captured", Ret::Out),
    e("testing_context.CaptureErr.eprint", "buri_rt_testing_context_capture_err_eprint", Ret::Void),
    e("testing_context.CaptureErr.eprintln", "buri_rt_testing_context_capture_err_eprintln", Ret::Void),
    e("testing_context.CaptureErr.capturedErr", "buri_rt_testing_context_capture_err_captured_err", Ret::Out),
    e("testing_context.stdin", "buri_rt_testing_context_stdin", Ret::Out),
    e("testing_context.stdinBytes", "buri_rt_testing_context_stdin_bytes", Ret::Out),
    e("testing_context.TestStdin.readLine", "buri_rt_testing_context_test_stdin_read_line", Ret::Opt),
    e("testing_context.TestStdin.readBytes", "buri_rt_testing_context_test_stdin_read_bytes", Ret::Opt),
    e("testing_context.data", "buri_rt_testing_context_data", Ret::Out),
    e("testing_context.files", "buri_rt_testing_context_files", Ret::Out),
    e("testing_context.MemFs.readFile", "buri_rt_testing_context_mem_fs_read_file", Ret::Res),
    e("testing_context.MemFs.writeFile", "buri_rt_testing_context_mem_fs_write_file", Ret::Res),
    e("testing_context.MemFs.fileExists", "buri_rt_testing_context_mem_fs_file_exists", Ret::Scalar),
    e("testing_context.MemFs.readDir", "buri_rt_testing_context_mem_fs_read_dir", Ret::Res),
    e("testing_context.clockAt", "buri_rt_testing_context_clock_at", Ret::Out),
    e("testing_context.TestClock.nowMillis", "buri_rt_testing_context_test_clock_now_millis", Ret::Scalar),
    e("testing_context.TestClock.sleepMillis", "buri_rt_testing_context_test_clock_sleep_millis", Ret::Void),
    e("testing_context.TestClock.advance", "buri_rt_testing_context_test_clock_advance", Ret::Void),
    e("testing_context.randSeed", "buri_rt_testing_context_rand_seed", Ret::Out),
    e("testing_context.TestRand.nextInt", "buri_rt_testing_context_test_rand_next_int", Ret::Scalar),
    e("testing_context.TestRand.nextFloat", "buri_rt_testing_context_test_rand_next_float", Ret::Scalar),
    e("testing_context.envOf", "buri_rt_testing_context_env_of", Ret::Out),
    e("testing_context.TestEnv.variable", "buri_rt_testing_context_test_env_variable", Ret::Opt),
    e("testing_context.TestEnv.arguments", "buri_rt_testing_context_test_env_arguments", Ret::Out),
];

/// How an `Option<T>` is written, flattened out of `middle::layout` so that the
/// emitter never learns which niche the layout chose.
///
/// `cranelift/emit.rs::option_call` asks the layout the same four questions
/// inline; this backend needs them as data because the store is a stencil and
/// a stencil takes literals.
#[derive(Clone, Copy, Debug)]
pub struct OptRepr {
    /// Byte offset of the discriminant, and its width; for a niche, the offset
    /// of the pointer that is null when the value is `.None`, at width eight.
    pub tag: (u32, u32),
    /// Whether `.None` is a null pointer rather than a stored tag.
    pub niche: bool,
    /// Byte offset of `.Some`'s payload.
    pub payload: u32,
    /// The discriminants themselves.
    pub some: u64,
    pub none: u64,
}

impl OptRepr {
    /// Reads the four facts off a destination's layout, or answers `None` when
    /// the destination is not an enum with an empty variant — which is what an
    /// `Option` is, structurally, and the only thing this may be asked about.
    pub fn of(l: &Layout) -> Option<OptRepr> {
        let Repr::Enum { repr, variants } = &l.repr else { return None };
        // `Option` declares `Some` first, so `Some` is variant 0 — but read it
        // off the layout rather than assuming: the empty variant is the one
        // with no fields.
        let none = variants.iter().position(|v| v.is_empty())? as u64;
        let some = u64::from(none == 0);
        let payload = variants
            .get(some as usize)
            .and_then(|v| v.first())
            .copied()
            .unwrap_or(0);
        Some(match repr {
            EnumRepr::Bare { tag } => {
                OptRepr { tag: (0, tag.size()), niche: false, payload: 0, some, none }
            }
            EnumRepr::Tagged { tag, .. } => {
                OptRepr { tag: (0, tag.size()), niche: false, payload, some, none }
            }
            EnumRepr::Niche { null_at } => {
                OptRepr { tag: (*null_at, 8), niche: true, payload, some, none }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key the archive has no body for must not be in the table: "it has no
    /// symbol" and "this backend cannot compile it" are two different
    /// statements, and only the second may be answered by
    /// `missing_intrinsics`.
    #[test]
    fn the_unimplemented_surface_is_not_claimed() {
        for absent in ["list.map", "list.fold", "list.sortBy", "list.zip", "json.decode"] {
            assert!(entry(absent).is_none(), "{absent}");
        }
    }

    /// `entry` answers the first row a key matches, so a duplicate would be a
    /// row that silently never runs.
    #[test]
    fn no_key_appears_twice() {
        let mut keys: Vec<&str> = ENTRIES.iter().map(|e| e.key).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "a key is in the table twice");
    }

    /// Every symbol is `buri_rt_`-prefixed, which is the whole of `lib.rs` §1's
    /// naming rule that can be checked without restating the mangler.
    #[test]
    fn every_symbol_is_the_runtimes() {
        for entry in ENTRIES {
            assert!(
                entry.symbol.starts_with(crate::compiler::backend::runtime_native::SYMBOL_PREFIX),
                "{}",
                entry.key
            );
        }
    }
}
