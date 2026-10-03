//! The `buri_rt_*` boundary: the shared table (`backend/runtime_table.rs`),
//! and the runtime symbols this backend calls without an intrinsic key.
//!
//! `cli/runtime/lib.rs`'s module comment is the contract, and
//! `backend/runtime_table.rs` is the transcription of it both native backends
//! emit against. This backend walks a row's [`Arg`] list to build the C
//! argument list at each argument's own position.

pub use crate::compiler::backend::runtime_table::{entry, Arg, Entry, Ret, BURI_OK, ENTRIES};

// ---------------------------------------------------------------------------
// The symbols this backend emits without an intrinsic key behind them
// ---------------------------------------------------------------------------

/// `buri_rt_abort(msg, len)` — `-> !`. Two parameters and no `base`: an abort
/// message is a static, and the runtime exits before anything could own it
/// (`abort.rs:52`).
pub const ABORT: &str = "buri_rt_abort";
/// `buri_rt_abort_div_zero()` — `-> !`. Division by zero (SPEC 6.2), with the
/// message shared between the two backends so `cli/tests/crash/` pins one
/// string.
pub const ABORT_DIV_ZERO: &str = "buri_rt_abort_div_zero";
/// `buri_rt_abort_shift()` — `-> !`. A `core/bits` shift count outside
/// `0 ..< bits`, with the message shared between the two backends and with
/// `$shiftCount` (`runtime.js:925`) so one string is pinned.
pub const ABORT_SHIFT: &str = "buri_rt_abort_shift";
/// `buri_rt_abort_unreachable()` — `-> !`, for `Profile::defensive_aborts`.
pub const ABORT_UNREACHABLE: &str = "buri_rt_abort_unreachable";
/// `buri_rt_abort_assert(kind, len)` — `-> !`, a failing `core/testing/assert`.
///
/// `(ptr, len)` and no `base`: the assertion kind is a `Str` the entry only
/// reads, so `lib.rs` §2 rule 1's third word is not passed and no count changes
/// hands — the process ends before either could matter.
pub const ABORT_ASSERT: &str = "buri_rt_abort_assert";
/// `buri_rt_test_enter(index) -> i32` — whether this process is to run the
/// `test` block at `index`, and a note of which one it is for the record an
/// abort writes. `cli/runtime/testing.rs` states the protocol.
pub const TEST_ENTER: &str = "buri_rt_test_enter";
/// `buri_rt_test_leave(index)` — the end of a `test` block: every fault the
/// block planned has happened, or the block fails now with the ones that did
/// not. Reached through the table above rather than from `test_entry_point`,
/// because `middle::monomorphize` emits the call inside the body's own function
/// and so all three backends get it from one place.
pub const TEST_LEAVE: &str = "buri_rt_test_leave";
/// `buri_rt_test_fail_compared(kind, len, actual, len, expected, len)` — `-> !`,
/// a failed comparison with both values already rendered by the `Show`
/// `middle::derives` generated at their type.
/// `buri_rt_test_replay(index) -> u8` — whether to run this `test` body again,
/// which `TestTasks.everyOrder` answers yes to once per completion order.
/// Emitted by `middle::monomorphize` immediately after [`TEST_LEAVE`], and
/// reached through the table for the same reason.
pub const TEST_REPLAY: &str = "buri_rt_test_replay";
pub const TEST_FAIL_COMPARED: &str = "buri_rt_test_fail_compared";
/// `buri_rt_test_fail_expected(kind, len, shown, len)` — `-> !`, `failExpected`
/// with its one value rendered.
pub const TEST_FAIL_EXPECTED: &str = "buri_rt_test_fail_expected";
/// `buri_rt_alloc(payload) -> *mut u8`.
pub const ALLOC: &str = "buri_rt_alloc";
/// `buri_rt_free(p)`.
pub const FREE: &str = "buri_rt_free";
/// `buri_rt_incref(p)` — the **shared** arm of `incref` (MEMORY.md §5.1).
///
/// The unshared arm is open-coded and always will be; this is reached only from
/// the fork on `cap`'s bit 63, which nothing sets, and it is a call because the
/// atomic sequence behind it is cold, is written once in the runtime, and is
/// twelve instructions this backend would otherwise put in front of the
/// optimizer at every reference operation in the program — measured at a
/// median +46 % of native release lowering, against +21 % for the call
/// (`design/PERFORMANCE.md` §6.6).
pub const INCREF: &str = "buri_rt_incref";
/// `buri_rt_decref(p, drop_glue)` — the **shared** arm of `decref`, and the
/// free that follows it. `drop_glue` is null for a type holding no references.
pub const DECREF: &str = "buri_rt_decref";
/// `buri_rt_argv_init(argc, argv)` — the emitted `main`'s first statement.
pub const ARGV_INIT: &str = "buri_rt_argv_init";
/// `buri_rt_flush()` — required before every return path from `main`.
pub const FLUSH: &str = "buri_rt_flush";
/// `buri_rt_frames_are_per_thread()` — the artifact's one statement about
/// itself, made once at startup (`cli/runtime/lib.rs` §6).
///
/// **This backend makes it and the frame-threaded one does not**, and that is
/// a fact about where a Buri frame lives rather than a difference of opinion
/// about scheduling. Here a Buri function is an ordinary LLVM function and its
/// locals are `alloca`s, so a thread's 512 KiB stack is its own and the
/// runtime may run two `Tasks.parallel` steps at once. There a program has one
/// Buri stack — the `buri$stencil$stack` block its `main` guards — and a step
/// runs in a frame the *call site* set aside, so two of them would share it.
/// Saying nothing is the safe answer, which is why the call is here and not a
/// parameter of one over there.
pub const FRAMES_PER_THREAD: &str = "buri_rt_frames_are_per_thread";
/// `buri_rt_values_may_cross_tasks()` — the artifact's other statement about
/// itself, made once at startup and **before it allocates anything**
/// (`cli/runtime/lib.rs` §6).
///
/// **Both native backends make it**, and only for a program
/// `middle::rc::crosses_tasks` says can reach a task boundary. It is not the
/// same fact as [`FRAMES_PER_THREAD`] and the two are deliberately not one
/// call: that one is about *where a frame lives*, which is a property of the
/// backend, and this one is about *whether a block can be reached from two
/// threads*, which is a property of the program. A backend that cannot fan
/// out still makes this call, because the day it learns to is not a day
/// anybody should have to remember a second edit.
///
/// Its effect is that every block the program allocates carries
/// `middle::layout::CAP_SHARED_FLAG`, so every reference operation takes G2's
/// atomic arm and no uniquely-owned in-place write fires. Silence is the safe
/// answer: the runtime's fan-out is gated on the same latch, so an entry point
/// that lost this call runs its tasks one at a time.
pub const VALUES_MAY_CROSS_TASKS: &str = "buri_rt_values_may_cross_tasks";

/// `void *buri_rt_copy_block(void *p, void (*glue)(void *))` — a fresh block
/// holding the same bytes, with `rc == 1` and nothing shared with the original.
///
/// G5's half of the copy out of a scope. The *type*-dependent half is the
/// per-type walk each backend generates (`Unit::copy_rc`,
/// `stencil/glue.rs`'s `Helper::Copy`); this is the block-dependent half, and
/// the division is where it is because this archive is compiled once against no
/// Buri type and a walk knows no header.
pub const COPY_BLOCK: &str = "buri_rt_copy_block";

/// `void buri_rt_copy_str(void *value)` — [`COPY_BLOCK`] for a `Str`, whose
/// `ptr` points *into* its block and has to be rebased onto the copy
/// (VALUE-MODEL.md §3).
pub const COPY_STR: &str = "buri_rt_copy_str";
/// `buri_rt_i128_divmod(a_lo, a_hi, b_lo, b_hi, signed, quot, rem)`.
pub const I128_DIVMOD: &str = "buri_rt_i128_divmod";
/// `buri_rt_i128_checked(op, a_lo, a_hi, b_lo, b_hi, signed, out) -> i32` and
/// `buri_rt_i128_saturating(op, ...)`, where `op` is `0` add, `1` sub, `2` mul,
/// `3` div.
///
/// A call rather than open-coded for [`I128_DIVMOD`]'s reason: the overflow test
/// both backends use at 64 bits is a widening multiply, which neither has at
/// `i128`, and a hand-rolled 128-bit one in two code generators is two places to
/// get it wrong. The `Entry` beside the first exists so that
/// `Unit::call_sum` — which translates an `i32` discriminant into whatever
/// `middle::layout` chose for the `Option` — can be driven by something that is
/// not a table row: this operation has no intrinsic key of its own, it is the
/// 128-bit arm of `number.I128.checkedAdd`.
pub const I128_SATURATING: &str = "buri_rt_i128_saturating";

/// The shape `buri_rt_i128_checked` answers in, as an [`Entry`] so the
/// sum-returning call path can be reused. `args` is empty because the argument
/// list is built at the call site from a 128-bit pair rather than from a Buri
/// signature, and the key is the one [`Entry::symbol`] mangles to that name.
pub const I128_CHECKED_ENTRY: Entry =
    Entry { key: "i128.checked", args: &[], ret: Ret::Sum, whole_value: false };

/// `buri_rt_str_scalar_len(ptr, byte_len) -> u64` — the slow half of
/// `str.length`, called only where the ASCII flag is clear.
pub const STR_SCALAR_LEN: &str = "buri_rt_str_scalar_len";

/// `buri_rt_str_written(base)` — the in-place arm of the open-coded
/// `str.concat` is about to write into `base`'s block, so any scalar index the
/// runtime keeps of the block's bytes is dropped (`cli/runtime/scalars.rs`).
pub const STR_WRITTEN: &str = "buri_rt_str_written";

// -- rendering: a template hole, and `derivePrimShow` ------------------------
//
// These have no intrinsic *key* — a template hole arrives as
// `ir::Inst::Structural { op: Show }` and `derivePrimShow` arrives with the
// primitive only in the IR type of its argument — so they are named here rather
// than in [`ENTRIES`], which is keyed on something neither of them has. Every
// one writes its `Str` through a trailing out-pointer (`cli/runtime/lib.rs` §2
// rule 2), which is [`Ret::Out`]'s shape without [`Ret::Out`]'s table row.

/// `buri_rt_str_from_int(i64, out)` — decimal. The template-hole rendering of
/// every integer of 64 bits or fewer, after a widening by the *source's*
/// signedness.
pub const SHOW_INT: &str = "buri_rt_str_from_int";
/// `buri_rt_show_f32(f32, out)` / `buri_rt_show_f64(f64, out)` — the
/// shortest-round-trip formatter, which has to be one body because it has to
/// produce the same bytes as JavaScript (VALUE-MODEL.md §12).
pub const SHOW_F32: &str = "buri_rt_show_f32";
pub const SHOW_F64: &str = "buri_rt_show_f64";
/// `buri_rt_show_i128(lo, hi, out)` / `buri_rt_show_u128(lo, hi, out)` — a pair
/// of `u64`s, low half first, for `I128_DIVMOD`'s reason.
pub const SHOW_I128: &str = "buri_rt_show_i128";
pub const SHOW_U128: &str = "buri_rt_show_u128";
/// `buri_rt_char_to_str(u32, out)` — a `Char` as its own UTF-8, which is what a
/// template hole asks for.
pub const CHAR_TO_STR: &str = "buri_rt_char_to_str";
/// `buri_rt_show_char(u32, out)` — `'c'`, quoted and escaped. `derivePrimShow`
/// only; a template hole is [`CHAR_TO_STR`].
pub const SHOW_CHAR: &str = "buri_rt_show_char";
/// `buri_rt_show_str(ptr, len, out)` — `JSON.stringify`. `derivePrimShow` only;
/// a template hole of a `Str` is the string itself and is not a call at all.
pub const SHOW_STR: &str = "buri_rt_show_str";
/// `buri_rt_show_list(xs, count, out)` — `[` + already-rendered elements joined
/// by `, ` + `]`, for `deriveArrayShow`. No element descriptor: the backend has
/// already turned each element into a `Str`, so the block this reads is a
/// `[Str]` at every instantiation.
pub const SHOW_LIST: &str = "buri_rt_show_list";

// -- hashing: `derivePrimHash` ----------------------------------------------
//
// FNV-1a over UTF-16 code units, which is the one thing about hashing that has
// to be shared rather than open-coded: `hash()` is observable, and the two
// backends must answer the same number as `runtime.js` (`cli/runtime/hash.rs`).

/// `buri_rt_mix(h: u64, x: u32) -> u64` — one 32-bit word into the
/// accumulator. Every `Bool` and every integer goes through this, truncated.
pub const MIX: &str = "buri_rt_mix";
/// `buri_rt_hash_f64(h: u64, x: f64) -> u64`. An `F32` is promoted first, so
/// that `1.5f32` and `1.5f64` hash alike as they do on JavaScript, where there
/// is one number type.
pub const HASH_F64: &str = "buri_rt_hash_f64";
/// `buri_rt_hash_char(h: u64, c: u32) -> u64`.
pub const HASH_CHAR: &str = "buri_rt_hash_char";
/// The FNV-1a offset basis, and the accumulator `$hash` starts from.
///
/// Transcribed rather than reached: it is a Rust `const` in
/// `cli/runtime/hash.rs` and not an exported symbol, so a backend that wants to
/// start a hash has to write it down. `middle::derives`'s `HASH_SEED` and
/// `stencil/emit.rs`'s `HASH_SEED` are the same number for the same reason, and
/// `cli/runtime/hash.rs`'s own tests pin it — which is what keeps four copies
/// of one constant from drifting into four different hashes.
pub const HASH_SEED: u64 = 0x811c_9dc5;
/// `buri_rt_hash_str(h: u64, base, ptr, len) -> u64` — **not** the mangler's
/// answer for `str.hash`, which would be `buri_rt_str_hash`, and therefore
/// deliberately not an [`ENTRIES`] row: the archive groups it with the other
/// hashes rather than with `core/str`.
pub const HASH_STR: &str = "buri_rt_hash_str";
