//! The `buri_rt_*` boundary: which intrinsics this backend has a symbol for,
//! and what that symbol's C signature is. **Wave 3d.**
//!
//! `cli/runtime/lib.rs`'s module comment is the contract; this file is the
//! transcription of it that generated code is emitted against. A disagreement
//! between the two is a miscompile that only shows up as a wrong answer at run
//! time, which is why the shapes are named ([`Arg`], [`Ret`]) rather than
//! spelled out per symbol as parameter counts.
//!
//! # Why a table and not a mangling
//!
//! `cli/runtime/lib.rs` §1 states the rule — "every exported symbol is
//! `buri_rt_` followed by `snake_case`", so `host.HostFs.readFile` is
//! `buri_rt_host_fs_read_file` — and a fifteen-line mangler would produce that
//! string. It would also produce `buri_rt_str_concat` for `str.concat` and
//! `buri_rt_list_len` for `list.len`, **neither of which exists**: the archive
//! is deliberately not the whole of the 203-function intrinsic surface
//! (`cli/runtime/lib.rs` §0), and what is not in it is either generated code —
//! `str.concat` is an allocation and two copies — or a later wave.
//!
//! A mangler would therefore turn "this toolchain cannot compile that program
//! yet" into a link error naming a symbol, or worse into a call to something
//! that happens to exist. A table turns it into
//! [`super::Llvm::missing_intrinsics`], which is asked *before* a second is
//! spent in LLVM — which is the reason that hook is on the trait
//! (`backend/mod.rs`, `TODO.md:1755`).
//!
//! So the mangler is still here, as [`symbol_for`], and it is used only to
//! *name* the symbol a table entry claims; the table decides what exists.

/// One entry in a runtime function's C parameter list.
///
/// `cli/runtime/lib.rs` §2 rule 1: every parameter is a scalar leaf, flattened
/// in declaration order. A `Str` is three parameters and a `[T]` is two.
///
/// Most variants consume one Buri argument and emit its leaves. [`Arg::Stride`]
/// and [`Arg::Retain`] consume **no** Buri argument at all: they are §2 rule
/// 4's "a generic parameter is a pointer and a stride", where the two extra
/// words come from `middle::layout` and from the backend's own glue rather than
/// from the call. That is why this is a description of the *C* parameter list
/// walked with a cursor into the Buri one, and not a per-argument table: the
/// two lists have different lengths at every generic entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arg {
    /// `base`, `ptr`, `len` — three parameters.
    Str,
    /// `ptr`, `len` — two. `writeBytes` and `abort` take a byte range without
    /// the owning block, because neither can retain it.
    Bytes,
    /// `ptr`, `len` — two.
    List,
    /// One.
    Scalar,
    /// Zero: a zero-sized `self` or context, dropped from the signature
    /// (VALUE-MODEL.md §8).
    Dropped,
    /// `ptr`, `len` — two, and the argument's element type is remembered for
    /// the [`Arg::Stride`] and [`Arg::Retain`] that follow it. The same two
    /// words [`Arg::List`] emits, kept a separate variant because "which type
    /// is `T`" is the question the generic entries are built around and a
    /// second reading of the argument list to answer it would be a place for
    /// the two to disagree.
    Elems,
    /// One pointer to a stack copy of a Buri argument whose type the runtime
    /// cannot name (§2 rule 4). Also remembers the element type, which is what
    /// makes `list.repeat` — whose only mention of `T` is the item — work.
    Spilled,
    /// `middle::layout`'s element stride, as an immediate. Consumes no Buri
    /// argument.
    Stride,
    /// The per-element retain glue, or null where the element type holds no
    /// counted pointers (`cli/runtime/list.rs`'s header). Consumes no Buri
    /// argument.
    Retain,
}

impl Arg {
    /// How many C parameters this shape emits.
    pub fn leaves(self) -> usize {
        match self {
            Arg::Str => 3,
            Arg::Bytes | Arg::List | Arg::Elems => 2,
            Arg::Scalar | Arg::Spilled | Arg::Stride | Arg::Retain => 1,
            Arg::Dropped => 0,
        }
    }

    /// Whether this shape takes the next Buri argument. The two shapes the
    /// backend supplies for itself do not.
    pub fn consumes(self) -> bool {
        !matches!(self, Arg::Stride | Arg::Retain)
    }
}

/// What comes back.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ret {
    /// Nothing, and the Buri result is `()`.
    Void,
    /// One scalar, at the Buri result's own register shape.
    Scalar,
    /// Nothing, and the call does not come back (`SPEC 6.10`).
    NoReturn,
    /// One integer of exactly this many bits, which is **not** the dest's
    /// register shape and is narrowed to it at the call site.
    ///
    /// The C boundary has no `i1` and no `i8` enum tag: `buri_rt_str_eq`
    /// returns `u8` and `buri_rt_str_compare` returns `i32`, while `Bool` is an
    /// `i1` and `Order` is whatever width `middle::layout` gave a three-variant
    /// bare tag. Declaring the import at the dest's width instead would be a
    /// return-register disagreement with the archive that nothing diagnoses —
    /// the values happen to be small enough that it would usually work, which
    /// is the worst kind of ABI bug.
    Int(u32),
    /// An aggregate written through a trailing out-pointer (§2 rule 2).
    ///
    /// The emitter allocates one entry-block `alloca` at the *dest's own*
    /// layout, passes it as the last parameter, and loads the dest's slots back
    /// out of it. `BuriStr` and `BuriList` are `#[repr(C)]` records of exactly
    /// the words `middle::layout` gives `Str` and `[T]`, which is what makes
    /// "the dest's layout" the right description of the buffer and saves the
    /// table from carrying a second spelling of it.
    Out,
    /// An `i32` discriminant plus a payload through a trailing out-pointer
    /// (§2 rule 3).
    ///
    /// [`BURI_OK`] is the success arm and `0` is `.None`. The out-pointer is
    /// the dest enum's own buffer offset to where `middle::layout` put
    /// `.Some`'s payload, so the runtime writes the payload *in place* and the
    /// backend only has to settle the discriminant — a tag store for
    /// `EnumRepr::Tagged`, nothing at all for `EnumRepr::Niche`, whose `.Some`
    /// is exactly "the payload with its niche pointer non-null".
    Sum,
}

/// One runtime entry this backend can emit a call to.
pub struct Entry {
    /// The intrinsic key `monomorphize` built: `host.HostStdout.println`.
    pub key: &'static str,
    /// The exported symbol, per `cli/runtime/lib.rs` §1.
    pub symbol: &'static str,
    /// One per Buri parameter, in declaration order, *including* the zero-sized
    /// `self` — so the list can be checked against the IR signature directly
    /// rather than after a filtering step whose result nothing verifies.
    pub args: &'static [Arg],
    pub ret: Ret,
}

/// The discriminant a fallible runtime entry returns for its success arm.
///
/// `cli/runtime/lib.rs`'s `BURI_OK`, transcribed rather than imported: the
/// runtime is a separate crate compiled for the *host* and linked as an
/// archive, so there is no path from here to its constants. The value is `-1`
/// rather than `0` so that an error variant's index is its index, and a backend
/// that gets the sign wrong fails immediately instead of silently reporting the
/// first error arm.
pub const BURI_OK: i64 = -1;

/// The entries this backend implements.
///
/// Deliberately still a subset. What is *in* it is now the whole of the shapes
/// `cli/runtime/lib.rs` §2 describes — a scalar result, an out-pointer (§2 rule
/// 2), a discriminant plus an out-pointer (§2 rule 3), and a generic parameter
/// carrying its stride and its retain glue (§2 rule 4) — because each of those
/// is one mechanism in [`super::emit`] rather than one special case per symbol.
///
/// What is deliberately still out, and why, so that a reader looking for one of
/// these finds the reason rather than an absence:
///
///  * **`str.concat`, `str.format`, `str.len`, `list.len`.** Open-coded in
///    [`super::emit::Unit::open_coded`]: each is an allocation and two copies,
///    a no-op, a masked load, or a word this backend already has the address
///    of, and a call would cost more than the sequence it stands for.
///  * **`list.empty`.** `cli/runtime/list.rs`'s `block` answers a **null**
///    `ptr` for an empty list, and there is no exported symbol for it anyway;
///    `[]` is `Inst::MakeArray` with no elements, which this backend already
///    emits as a real one-block allocation.
///  * **`json.*`, and every `list.*` entry taking a closure** — `map`,
///    `filter`, `fold`, `any`, `all`, `find`, `findIndex`, `count`, `sortBy`,
///    `zip`, `flatten`. `cli/runtime/list.rs`'s header states why they are not
///    in the archive: a Buri closure's `code` is a thunk at the *flattened*
///    signature of its own element type, so calling one from C would mean
///    synthesizing a parameter list that depends on `T`.
pub const ENTRIES: &[Entry] = &[
    // -- the text streams ---------------------------------------------------
    //
    // `self` is `HostStdout`, an empty struct, so it is dropped; the argument
    // is a `Template`, which is a `Str` (VALUE-MODEL.md §3.3).
    Entry {
        key: "host.HostStdout.print",
        symbol: "buri_rt_host_stdout_print",
        args: &[Arg::Dropped, Arg::Str],
        ret: Ret::Void,
    },
    Entry {
        key: "host.HostStdout.println",
        symbol: "buri_rt_host_stdout_println",
        args: &[Arg::Dropped, Arg::Str],
        ret: Ret::Void,
    },
    Entry {
        key: "host.HostStderr.eprint",
        symbol: "buri_rt_host_stderr_eprint",
        args: &[Arg::Dropped, Arg::Str],
        ret: Ret::Void,
    },
    Entry {
        key: "host.HostStderr.eprintln",
        symbol: "buri_rt_host_stderr_eprintln",
        args: &[Arg::Dropped, Arg::Str],
        ret: Ret::Void,
    },
    // Bytes, not text: no `base`, because the runtime writes them and keeps
    // nothing.
    Entry {
        key: "host.HostStdout.writeBytes",
        symbol: "buri_rt_host_stdout_write_bytes",
        args: &[Arg::Dropped, Arg::List],
        ret: Ret::Void,
    },
    // -- the scalar capabilities -------------------------------------------
    Entry {
        key: "host.HostClock.nowMillis",
        symbol: "buri_rt_host_clock_now_millis",
        args: &[Arg::Dropped],
        ret: Ret::Scalar,
    },
    Entry {
        key: "host.HostClock.sleepMillis",
        symbol: "buri_rt_host_clock_sleep_millis",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::Void,
    },
    Entry {
        key: "host.HostRand.nextInt",
        symbol: "buri_rt_host_rand_next_int",
        args: &[Arg::Dropped, Arg::Scalar, Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "host.HostRand.nextFloat",
        symbol: "buri_rt_host_rand_next_float",
        args: &[Arg::Dropped],
        ret: Ret::Scalar,
    },
    // The `Alloc` capability's charge, which is the identity function plus a
    // budget check in the runtime (`memory.rs:330`).
    Entry {
        key: "host.HostAlloc.allocate",
        symbol: "buri_rt_host_alloc_allocate",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "host.HostProc.exitWith",
        symbol: "buri_rt_host_proc_exit_with",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::NoReturn,
    },
    // -- core/alloc's counters ----------------------------------------------
    //
    // `GeneralPurpose`, `Arena` and `FixedBuffer` carry a handle into a table
    // in `cli/runtime/memory.rs`, so every one of these is scalars in and a
    // scalar out with no `self` to drop: the handle is passed, not the
    // allocator. `charge` can end the process and still returns `Ret::Scalar`,
    // because it returns on every request that fits its budget.
    Entry {
        key: "alloc.newCounter",
        symbol: "buri_rt_alloc_new_counter",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "alloc.charge",
        symbol: "buri_rt_alloc_charge",
        args: &[Arg::Scalar, Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "alloc.count",
        symbol: "buri_rt_alloc_count",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "alloc.total",
        symbol: "buri_rt_alloc_total",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    // -- core/str, the pure half (`cli/runtime/text.rs`) ---------------------
    //
    // Every one of these takes `self` as a full `Str`: the `base` is passed
    // even where the body ignores it, because §2 rule 1 flattens the *value*
    // and a runtime that later wants to retain a view has to have been handed
    // the block it is a view into.
    Entry {
        key: "str.charAt",
        symbol: "buri_rt_str_char_at",
        args: &[Arg::Str, Arg::Scalar],
        ret: Ret::Sum,
    },
    Entry {
        key: "str.slice",
        symbol: "buri_rt_str_slice",
        args: &[Arg::Str, Arg::Scalar, Arg::Scalar],
        ret: Ret::Out,
    },
    Entry { key: "str.trim", symbol: "buri_rt_str_trim", args: &[Arg::Str], ret: Ret::Out },
    Entry {
        key: "str.trimStart",
        symbol: "buri_rt_str_trim_start",
        args: &[Arg::Str],
        ret: Ret::Out,
    },
    Entry { key: "str.trimEnd", symbol: "buri_rt_str_trim_end", args: &[Arg::Str], ret: Ret::Out },
    // `u8` out of C and `i1` into the IR — see [`Ret::Int`].
    Entry {
        key: "str.startsWith",
        symbol: "buri_rt_str_starts_with",
        args: &[Arg::Str, Arg::Str],
        ret: Ret::Int(8),
    },
    Entry {
        key: "str.endsWith",
        symbol: "buri_rt_str_ends_with",
        args: &[Arg::Str, Arg::Str],
        ret: Ret::Int(8),
    },
    Entry {
        key: "str.contains",
        symbol: "buri_rt_str_contains",
        args: &[Arg::Str, Arg::Str],
        ret: Ret::Int(8),
    },
    Entry {
        key: "str.indexOf",
        symbol: "buri_rt_str_index_of",
        args: &[Arg::Str, Arg::Str],
        ret: Ret::Sum,
    },
    Entry {
        key: "str.splitOnce",
        symbol: "buri_rt_str_split_once",
        args: &[Arg::Str, Arg::Str],
        ret: Ret::Sum,
    },
    // `Order` is `{ Less, Equal, Greater }` with no payload, so it is
    // `EnumRepr::Bare` and the result *is* the tag — `0`, `1`, `2` in
    // declaration order, which is the same numbering `text.rs` answers. No
    // `Ret::Sum` translation, because there is no success arm to distinguish.
    Entry {
        key: "str.compare",
        symbol: "buri_rt_str_compare",
        args: &[Arg::Str, Arg::Str],
        ret: Ret::Int(32),
    },
    // Not declared in `core/str`: this is the `Str` arm of an `Inst::Binary`
    // at `Prim::Str`, which is what `middle::derives` emits for a derived `Eq`
    // over a type with a `Str` in it (`derives.rs`'s `fn eq`).
    Entry {
        key: "str.eq",
        symbol: "buri_rt_str_eq",
        args: &[Arg::Str, Arg::Str],
        ret: Ret::Int(8),
    },
    Entry { key: "str.toInt", symbol: "buri_rt_str_to_int", args: &[Arg::Str], ret: Ret::Sum },
    Entry { key: "str.toFloat", symbol: "buri_rt_str_to_float", args: &[Arg::Str], ret: Ret::Sum },
    // -- core/str, the `<C: Alloc>` half ------------------------------------
    //
    // The context is an empty record in every implementation `core/host`
    // supplies, so it is zero-sized and dropped (VALUE-MODEL.md §8) — it is
    // still listed, because the list is checked against the IR signature and a
    // silently shorter one would misalign every argument after it.
    Entry {
        key: "str.split",
        symbol: "buri_rt_str_split",
        args: &[Arg::Str, Arg::Dropped, Arg::Str],
        ret: Ret::Out,
    },
    Entry {
        key: "str.splitAny",
        symbol: "buri_rt_str_split_any",
        args: &[Arg::Str, Arg::Dropped, Arg::Str],
        ret: Ret::Out,
    },
    Entry {
        key: "str.lines",
        symbol: "buri_rt_str_lines",
        args: &[Arg::Str, Arg::Dropped],
        ret: Ret::Out,
    },
    Entry {
        key: "str.replace",
        symbol: "buri_rt_str_replace",
        args: &[Arg::Str, Arg::Dropped, Arg::Str, Arg::Str],
        ret: Ret::Out,
    },
    Entry {
        key: "str.repeat",
        symbol: "buri_rt_str_repeat",
        args: &[Arg::Str, Arg::Dropped, Arg::Scalar],
        ret: Ret::Out,
    },
    Entry {
        key: "str.toUpper",
        symbol: "buri_rt_str_to_upper",
        args: &[Arg::Str, Arg::Dropped],
        ret: Ret::Out,
    },
    Entry {
        key: "str.toLower",
        symbol: "buri_rt_str_to_lower",
        args: &[Arg::Str, Arg::Dropped],
        ret: Ret::Out,
    },
    Entry {
        key: "str.chars",
        symbol: "buri_rt_str_chars",
        args: &[Arg::Str, Arg::Dropped],
        ret: Ret::Out,
    },
    // `[Char]` in, and the runtime reads it as a block of `u32`s: `Arg::List`
    // rather than `Arg::Elems`, because `Char`'s stride is not a parameter
    // here — `buri_rt_str_from_chars` knows the element type it was written
    // for, which is the one `list.*` entry shape that is *not* generic.
    Entry {
        key: "str.fromChars",
        symbol: "buri_rt_str_from_chars",
        args: &[Arg::Dropped, Arg::List],
        ret: Ret::Out,
    },
    Entry {
        key: "str.fromInt",
        symbol: "buri_rt_str_from_int",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::Out,
    },
    Entry {
        key: "str.fromFloat",
        symbol: "buri_rt_str_from_float",
        args: &[Arg::Dropped, Arg::Scalar],
        ret: Ret::Out,
    },
    // `fill` is a `Char`, which is a `U32` scalar — one leaf, not a `Str`'s
    // three. `$str_padStart` repeats the *whole* fill for `width - len(self)`
    // iterations, so a multi-scalar fill overshoots the width; that is a
    // peculiarity of the JavaScript answer rather than a design, and the
    // runtime reproduces it exactly (`cli/runtime/text.rs`).
    Entry {
        key: "str.padStart",
        symbol: "buri_rt_str_pad_start",
        args: &[Arg::Str, Arg::Dropped, Arg::Scalar, Arg::Scalar],
        ret: Ret::Out,
    },
    Entry {
        key: "str.padEnd",
        symbol: "buri_rt_str_pad_end",
        args: &[Arg::Str, Arg::Dropped, Arg::Scalar, Arg::Scalar],
        ret: Ret::Out,
    },
    // `Hash`'s method, which starts from the FNV-1a seed and takes no
    // accumulator — unlike `derivePrimHash.Str`, which continues one and is
    // `buri_rt_hash_str`. Two symbols because they are two operations, and the
    // mangler's answer for this key happens to be the one the archive exports.
    Entry { key: "str.hash", symbol: "buri_rt_str_hash", args: &[Arg::Str], ret: Ret::Scalar },
    // -- core/list, the block-copying half (`cli/runtime/list.rs`) ----------
    //
    // `Arg::Stride` and `Arg::Retain` consume no Buri argument: they are the
    // backend's two words, and `cli/runtime/list.rs` puts them last, directly
    // before `out`, at every entry that takes them. So the Buri order and the C
    // order agree up to the point where the C one grows two more parameters —
    // which is what the cursor in `Unit::entry_args` walks.
    Entry {
        key: "list.get",
        symbol: "buri_rt_list_get",
        args: &[Arg::Elems, Arg::Scalar, Arg::Stride, Arg::Retain],
        ret: Ret::Sum,
    },
    Entry {
        key: "list.concat",
        symbol: "buri_rt_list_concat",
        args: &[Arg::Elems, Arg::Dropped, Arg::Elems, Arg::Stride, Arg::Retain],
        ret: Ret::Out,
    },
    Entry {
        key: "list.push",
        symbol: "buri_rt_list_push",
        args: &[Arg::Elems, Arg::Dropped, Arg::Spilled, Arg::Stride, Arg::Retain],
        ret: Ret::Out,
    },
    Entry {
        key: "list.reverse",
        symbol: "buri_rt_list_reverse",
        args: &[Arg::Elems, Arg::Dropped, Arg::Stride, Arg::Retain],
        ret: Ret::Out,
    },
    Entry {
        key: "list.slice",
        symbol: "buri_rt_list_slice",
        args: &[Arg::Elems, Arg::Dropped, Arg::Scalar, Arg::Scalar, Arg::Stride, Arg::Retain],
        ret: Ret::Out,
    },
    Entry {
        key: "list.repeat",
        symbol: "buri_rt_list_repeat",
        args: &[Arg::Dropped, Arg::Spilled, Arg::Scalar, Arg::Stride, Arg::Retain],
        ret: Ret::Out,
    },
    // `[Int]` and nothing generic: no stride, no retain, and an `Int` holds no
    // count to take.
    Entry {
        key: "list.range",
        symbol: "buri_rt_list_range",
        args: &[Arg::Dropped, Arg::Scalar, Arg::Scalar],
        ret: Ret::Out,
    },
    // -- core/math (`cli/runtime/math.rs`) ----------------------------------
    //
    // Nine of the twenty-two. The other thirteen are the transcendentals —
    // `cbrt`, `pow`, `exp`, `ln`, `log10`, `log2`, the six trigonometric ones
    // and `atan2` — and they are **deliberately** absent rather than a call to
    // libm. IEEE 754 does not fix their answers, so V8's fdlibm port and a
    // platform libm differ in the last bit, and a rendered `Float` shows all
    // seventeen digits of that difference. A named gap is a diagnostic; a libm
    // call is a conformance failure nobody can attribute.
    Entry { key: "math.sqrt", symbol: "buri_rt_math_sqrt", args: &[Arg::Scalar], ret: Ret::Scalar },
    Entry {
        key: "math.absFloat",
        symbol: "buri_rt_math_abs_float",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "math.floor",
        symbol: "buri_rt_math_floor",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry { key: "math.ceil", symbol: "buri_rt_math_ceil", args: &[Arg::Scalar], ret: Ret::Scalar },
    Entry {
        key: "math.trunc",
        symbol: "buri_rt_math_trunc",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    Entry {
        key: "math.round",
        symbol: "buri_rt_math_round",
        args: &[Arg::Scalar],
        ret: Ret::Scalar,
    },
    // `Bool` out of C is a `u8`, and an `i1` into the IR — see [`Ret::Int`].
    Entry {
        key: "math.isNan",
        symbol: "buri_rt_math_is_nan",
        args: &[Arg::Scalar],
        ret: Ret::Int(8),
    },
    Entry {
        key: "math.isInfinite",
        symbol: "buri_rt_math_is_infinite",
        args: &[Arg::Scalar],
        ret: Ret::Int(8),
    },
    Entry {
        key: "math.isFinite",
        symbol: "buri_rt_math_is_finite",
        args: &[Arg::Scalar],
        ret: Ret::Int(8),
    },
    // Declared in `core/list` and implemented in `text.rs`, because joining is
    // a `Str` producer that happens to read a `[Str]`. The elements are read as
    // `BuriStr`s at a stride the runtime knows, so this is `Arg::List`.
    Entry {
        key: "list.join",
        symbol: "buri_rt_list_join",
        args: &[Arg::List, Arg::Dropped, Arg::Str],
        ret: Ret::Out,
    },
];

pub fn entry(key: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.key == key)
}

/// The symbol `cli/runtime/lib.rs` §1's rule names for a key.
///
/// Used to *check* the table rather than to drive emission — see the module
/// header. A key with no dot is left alone but for the prefix, which is what
/// makes `str.concat` come out `buri_rt_str_concat` and be absent from
/// [`ENTRIES`] rather than silently linked against.
///
/// The rule is "`buri_rt_` followed by `snake_case`", plus one thing the
/// contract states by example rather than in words: `host.HostStdout.println`
/// is `buri_rt_host_stdout_println` and **not** `buri_rt_host_host_stdout_println`.
/// The capability type repeats its module in its name, and the symbol does not
/// repeat it twice — so a snake-cased segment that begins with the previous
/// segment drops that prefix. `host.HostAlloc.allocate` is
/// `buri_rt_host_alloc_allocate`, which is the same rule and keeps the
/// non-redundant repetition it happens to have.
pub fn symbol_for(key: &str) -> String {
    let mut out = String::from(super::super::runtime_native::SYMBOL_PREFIX);
    let mut previous = String::new();
    for (i, segment) in key.split('.').enumerate() {
        let mut piece = String::new();
        snake_into(segment, &mut piece);
        if let Some(rest) = piece.strip_prefix(&format!("{previous}_")) {
            if !previous.is_empty() {
                piece = rest.to_string();
            }
        }
        if i > 0 {
            out.push('_');
        }
        previous = piece.clone();
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
/// `buri_rt_alloc(payload) -> *mut u8`.
pub const ALLOC: &str = "buri_rt_alloc";
/// `buri_rt_free(p)`.
pub const FREE: &str = "buri_rt_free";
/// `buri_rt_argv_init(argc, argv)` — the emitted `main`'s first statement.
pub const ARGV_INIT: &str = "buri_rt_argv_init";
/// `buri_rt_flush()` — required before every return path from `main`.
pub const FLUSH: &str = "buri_rt_flush";
/// `buri_rt_i128_divmod(a_lo, a_hi, b_lo, b_hi, signed, quot, rem)`.
pub const I128_DIVMOD: &str = "buri_rt_i128_divmod";
/// `buri_rt_i128_checked(op, a_lo, a_hi, b_lo, b_hi, signed, out) -> i32` and
/// `buri_rt_i128_saturating(op, ...)`, where `op` is `0` add, `1` sub, `2` mul,
/// `3` div.
///
/// A call rather than open-coded for [`I128_DIVMOD`]'s reason: the overflow test
/// both backends use at 64 bits is a widening multiply, which Cranelift does not
/// define at `i128`, and a hand-rolled 128-bit one in two code generators is two
/// places to get it wrong. The `Entry` beside the first exists so that
/// `Unit::call_sum` — which translates an `i32` discriminant into whatever
/// `middle::layout` chose for the `Option` — can be driven by something that is
/// not an [`ENTRIES`] row: this operation has no intrinsic key of its own, it is
/// the 128-bit arm of `num.I128.checkedAdd`.
pub const I128_CHECKED: &str = "buri_rt_i128_checked";
pub const I128_SATURATING: &str = "buri_rt_i128_saturating";

/// The shape [`I128_CHECKED`] answers in, as an [`Entry`] so the sum-returning
/// call path can be reused. `args` is empty because the argument list is built
/// at the call site from a 128-bit pair rather than from a Buri signature.
pub const I128_CHECKED_ENTRY: Entry = Entry {
    key: "num.<128>.checked",
    symbol: I128_CHECKED,
    args: &[],
    ret: Ret::Sum,
};

/// `buri_rt_str_scalar_len(ptr, byte_len) -> u64` — the slow half of
/// `str.len`, called only where the ASCII flag is clear.
pub const STR_SCALAR_LEN: &str = "buri_rt_str_scalar_len";

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
/// `cranelift/runtime.rs`'s `SEED` are the same number for the same reason, and
/// `cli/runtime/hash.rs`'s own tests pin it — which is what keeps four copies
/// of one constant from drifting into four different hashes.
pub const HASH_SEED: u64 = 0x811c_9dc5;
/// `buri_rt_hash_str(h: u64, base, ptr, len) -> u64` — **not** the mangler's
/// answer for `str.hash`, which would be `buri_rt_str_hash`, and therefore
/// deliberately not an [`ENTRIES`] row: the archive groups it with the other
/// hashes rather than with `core/str`.
pub const HASH_STR: &str = "buri_rt_hash_str";

#[cfg(test)]
mod tests {
    use super::*;

    /// Every entry's symbol is the one the contract's rule produces, so the
    /// table is a *subset* of the contract rather than a second opinion about
    /// it. The three examples `cli/runtime/lib.rs` §1 and VALUE-MODEL.md §10
    /// spell out are checked directly.
    #[test]
    fn every_symbol_is_the_rule_applied_to_the_key() {
        for e in ENTRIES {
            assert_eq!(symbol_for(e.key), e.symbol, "{}", e.key);
        }
        assert_eq!(symbol_for("host.HostFs.readFile"), "buri_rt_host_fs_read_file");
        assert_eq!(symbol_for("host.HostProc.exitWith"), "buri_rt_host_proc_exit_with");
        assert_eq!(symbol_for("host.HostStdout.println"), "buri_rt_host_stdout_println");
        // The one that does not collapse, because the repetition is real:
        // `memory.rs:330` exports it under this name.
        assert_eq!(symbol_for("host.HostAlloc.allocate"), "buri_rt_host_alloc_allocate");
        assert_eq!(symbol_for("list.map"), "buri_rt_list_map");
    }

    /// A key the archive has no body for must not be in the table. If one of
    /// these ever gains a symbol, this test is the reminder to add its shape
    /// rather than to let the mangler invent it.
    ///
    /// `str.concat`, `str.format`, `str.len`, `list.len` and `list.empty` are
    /// open-coded; `derivePrimShow` and `derivePrimHash` arrive qualified by
    /// their primitive (`derivePrimShow.U8`) and so have no key of their own;
    /// `json.decode` and every `list.*` entry taking a closure are outside the
    /// archive for the reasons [`ENTRIES`]'s comment gives.
    #[test]
    fn the_generated_surface_is_not_claimed() {
        for absent in [
            "str.concat",
            "str.format",
            "str.len",
            "list.len",
            "json.decode",
            "derivePrimShow",
            "derivePrimHash",
            "list.empty",
            "list.map",
        ] {
            assert!(entry(absent).is_none(), "{absent}");
        }
    }

    /// The two shapes the backend supplies for itself emit a parameter and
    /// consume no argument, and everything else consumes exactly one. The
    /// emitter walks the C list with a cursor into the Buri list on precisely
    /// this invariant.
    #[test]
    fn only_the_generic_extras_consume_no_argument() {
        for shape in [Arg::Str, Arg::Bytes, Arg::List, Arg::Scalar, Arg::Dropped, Arg::Elems, Arg::Spilled] {
            assert!(shape.consumes(), "{shape:?}");
        }
        for shape in [Arg::Stride, Arg::Retain] {
            assert!(!shape.consumes(), "{shape:?}");
            assert_eq!(shape.leaves(), 1);
        }
    }

    /// `stride` and `retain` travel together, and only where the entry has a
    /// generic parameter to describe (`cli/runtime/lib.rs` §2 rule 4). An entry
    /// with one and not the other would be a call with a parameter missing,
    /// which the C boundary does not diagnose.
    #[test]
    fn stride_and_retain_come_in_pairs_behind_a_generic() {
        for e in ENTRIES {
            let strides = e.args.iter().filter(|a| **a == Arg::Stride).count();
            let retains = e.args.iter().filter(|a| **a == Arg::Retain).count();
            assert_eq!(strides, retains, "{}", e.key);
            let generic =
                e.args.iter().any(|a| matches!(a, Arg::Elems | Arg::Spilled));
            assert_eq!(strides > 0, generic, "{}", e.key);
        }
    }
}
