//! The `buri_rt_*` boundary for this backend: which intrinsic keys have a
//! symbol, and what shape the call has. **Wave 3d.**
//!
//! `cli/runtime/lib.rs`'s module comment is the contract; this is the
//! transcription of it that generated code is emitted against. Wave 2a spelled
//! that transcription as a `DIRECT` list of eleven keys plus a mangler, which
//! worked while every entry had the same shape — flattened arguments in, one
//! scalar out. Three of the shapes `lib.rs` §2 describes are now in use and the
//! list cannot carry them, so it becomes a table, the same way `llvm/runtime.rs`
//! already is.
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
//! Step 1 is `Lower::spread` and needs no table entry at all: a `Str` argument
//! spreads to three values because a `Str` *is* three leaves, and a zero-sized
//! `ctx` spreads to none because it occupies no bytes. That is why there is no
//! per-argument column here — wave 2b's LLVM table has one, and it has to,
//! because that backend reconstructs the argument list from the entry rather
//! than from the IR. Here the IR is the argument list, and a second description
//! of it would be a thing to disagree with.
//!
//! Steps 2 and 3 are what [`Extra`] and [`Ret`] select.
//!
//! # Why a table and not a mangling
//!
//! Unchanged from wave 2b's statement of it (`llvm/runtime.rs`'s header): the
//! rule in `lib.rs` §1 would happily produce `buri_rt_list_map` for `list.map`,
//! which does not exist, and a program that used it would get a link error
//! naming a symbol instead of [`super::Cranelift::missing_intrinsics`] naming
//! the operation. The mangler is still here as [`symbol_for`], and it is used to
//! *check* the table rather than to drive emission.

/// The discriminant a fallible runtime entry returns for its success arm.
///
/// `cli/runtime/lib.rs`'s `BURI_OK`, restated here because the compiler and the
/// runtime are two crates that never link against each other — the archive is
/// `include_bytes!`d, not depended on — so a shared constant is impossible and
/// the two spellings are held together by `cli/tests/runtime_native.rs`'s C
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
];

/// The entry for a key, or `None` where this backend has no body for it.
pub fn entry(key: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.key == key)
}

/// The symbol `cli/runtime/lib.rs` §1's rule names for a key.
///
/// Used to *check* the table rather than to drive emission — see the module
/// header. The rule is "`buri_rt_` followed by `snake_case`", plus the one thing
/// the contract states by example rather than in words: a segment that begins
/// with the previous segment drops that prefix, so `host.HostStdout.println` is
/// `buri_rt_host_stdout_println` and not `buri_rt_host_host_stdout_println`.
pub fn symbol_for(key: &str) -> String {
    let mut out = String::from(crate::compiler::backend::runtime_native::SYMBOL_PREFIX);
    let mut previous = String::new();
    for (i, segment) in key.split('.').enumerate() {
        let mut piece = String::new();
        snake_into(segment, &mut piece);
        if !previous.is_empty() {
            if let Some(rest) = piece.strip_prefix(&format!("{previous}_")) {
                piece = rest.to_string();
            }
        }
        if i > 0 {
            out.push('_');
        }
        previous.clone_from(&piece);
        out.push_str(&piece);
    }
    out
}

/// `HostFs` -> `host_fs`, `readFile` -> `read_file`, `nowMillis` ->
/// `now_millis`. An underscore before an upper-case letter that follows a
/// lower-case one or a digit; runs of capitals are not split, because no key
/// has one.
fn snake_into(segment: &str, out: &mut String) {
    let mut previous_lower = false;
    for c in segment.chars() {
        if c.is_ascii_uppercase() {
            if previous_lower {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
            previous_lower = false;
        } else {
            out.push(c);
            previous_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        }
    }
}

// ---------------------------------------------------------------------------
// The symbols this backend emits without an intrinsic key behind them
// ---------------------------------------------------------------------------

/// `buri_rt_abort(msg, len)` — `-> !`.
pub const ABORT: &str = "buri_rt_abort";
/// `buri_rt_abort_div_zero()` — `-> !`, SPEC 6.2.
pub const ABORT_DIV_ZERO: &str = "buri_rt_abort_div_zero";
/// `buri_rt_alloc(payload) -> *mut u8`.
pub const ALLOC: &str = "buri_rt_alloc";
/// `buri_rt_i128_divmod(a_lo, a_hi, b_lo, b_hi, signed, quot, rem)`.
pub const I128_DIVMOD: &str = "buri_rt_i128_divmod";
/// `buri_rt_str_scalar_len(ptr, len) -> u64` — `str.len`'s slow path.
pub const STR_SCALAR_LEN: &str = "buri_rt_str_scalar_len";

/// The renderers `derivePrimShow` and a template hole reach, by primitive.
///
/// `show` of a `Str` is *two* different functions depending on which of them
/// asked: a template hole wants the string itself (`$str`), and a derived
/// `Show` wants it quoted and escaped (`$show`'s `"s"` arm, which is
/// `JSON.stringify`). The same split applies to `Char`. That is not a
/// native quirk — `runtime.js:72-82` and `runtime.js:200-210` are two
/// functions there too — and it is the reason these are named in pairs.
pub mod show {
    /// `$f64`: `NaN`, `inf`, `-inf`, `42.0`, or `Number::toString`.
    pub const F64: &str = "buri_rt_show_f64";
    /// The same, through the double an `F32` widens to.
    pub const F32: &str = "buri_rt_show_f32";
    /// 128-bit decimal, taking the value as `(lo, hi)`.
    pub const I128: &str = "buri_rt_show_i128";
    pub const U128: &str = "buri_rt_show_u128";
    /// A `Char` as its own one-scalar string — the template-hole rendering.
    pub const CHAR: &str = "buri_rt_char_to_str";
    /// A `Char` in single quotes — the derived one.
    pub const CHAR_QUOTED: &str = "buri_rt_show_char";
    /// A `Str`, quoted and escaped — the derived one. A template hole needs no
    /// call at all, because the answer is the argument.
    pub const STR_QUOTED: &str = "buri_rt_show_str";
    /// Decimal, for every integer at 64 bits or below.
    pub const INT: &str = "buri_rt_str_from_int";
}

/// The hashers `derivePrimHash` and `Hash::hash` reach.
///
/// Two forms of each, for the same reason `show` has two: `derivePrimHash` is
/// `(U64, T) -> U64` and threads an accumulator, while `Hash::hash` is
/// `(T) -> U64` and starts from the offset basis (`order.buri:34`). The seeded
/// form is a runtime symbol rather than an immediate at the call site so that
/// the constant lives in one file.
pub mod hash {
    /// `$mix(h, x)` — one 32-bit FNV-1a step. Every integer and `Bool` reaches
    /// it, truncated to 32 bits by the caller.
    pub const MIX: &str = "buri_rt_mix";
    /// `$hashInto` at a float: `ToUint32(Math.trunc(x) || 0)`, then a mix.
    pub const F64: &str = "buri_rt_hash_f64";
    /// `$hashInto` at a `Char`, which is one *string* of one character — so an
    /// astral scalar is two mixes.
    pub const CHAR: &str = "buri_rt_hash_char";
    /// `$hashInto` at a `Str`, one mix per UTF-16 code unit.
    pub const STR: &str = "buri_rt_hash_str";
    /// The FNV-1a offset basis, for the accumulator's first step.
    pub const SEED: i64 = 0x811c_9dc5;
}

#[cfg(test)]
mod tests {
    use super::*;

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
        for absent in ["list.map", "list.fold", "list.zip", "list.flatten", "json.decode"] {
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
