//! The `buri_rt_*` boundary for this backend: the shared table, and the
//! symbols this backend emits with no intrinsic key behind them.
//!
//! `cli/runtime/lib.rs`'s module comment is the contract, and
//! `backend/runtime_table.rs` is the transcription of it that generated code
//! is emitted against — shared with the copy-and-patch backend, which emits
//! the same call from the same rows. That file's header carries the four
//! shapes and the emission rule.
//!
//! What is here is what only this backend names: the abort and allocator
//! entries `Lower` reaches for directly, the `show` and `hash` renderers
//! `middle::derives` leaves behind, and the wide-integer helpers. None of
//! those arrives as an intrinsic key, so none of them is a table row.

/// The table, and the shapes it is written in.
///
/// `backend/runtime_table.rs` holds them, because the copy-and-patch backend
/// emits the same call from the same rows. They are re-exported here so that
/// `runtime::` is still where this backend's emitter looks.
pub use crate::compiler::backend::runtime_table::{entry, Entry, Extra, Ret, BURI_OK, ENTRIES};

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
