//! `core/str`, natively: the whole of the surface `str.buri` declares without a
//! body, minus the ones the backends open-code — which is now `str.len` and
//! `str.format` alone, because [`buri_rt_str_concat`] is here for the
//! copy-and-patch backend to call (MEMORY.md §5.3).
//!
//! The arbiter of every answer here is `backend/js/runtime.js`'s `$str_*`
//! (lines 666-849) and the conformance suite that pins it, not this file's own
//! idea of what a string operation should do. Three consequences worth stating
//! before the code, because each one would otherwise look like a mistake:
//!
//! * **Indices are Unicode scalars, offsets are bytes.** `str.len`, `charAt`,
//!   `slice`, `indexOf` and the two `pad`s all speak in scalar counts
//!   (`str.buri:18`), while a `BuriStr` is a byte range. Every entry below that
//!   takes an index converts, and the ASCII flag (VALUE-MODEL.md §3.1) is what
//!   makes that free on the input that matters: set, and a scalar index *is* a
//!   byte offset.
//! * **A pure operation returns a view.** `slice`, `trim`, `trimStart`,
//!   `trimEnd` and `splitOnce` are declared without an `Alloc` bound, which is
//!   `core/str`'s way of saying they do not copy. So they answer a `BuriStr`
//!   pointing into the *caller's* allocation, and they incref its base before
//!   doing so — which is `lib.rs` §3's "a result is owned" applied to a value
//!   whose block it shares rather than owns. A literal has a null base and the
//!   incref is a no-op, so a literal still touches no allocator.
//! * **Comparison and hashing are in UTF-16.** `$str_compare` is JavaScript's
//!   `<`, which orders by code unit, and `$hashInto` mixes `charCodeAt(i)`.
//!   Those differ from byte order and from scalar order for strings mixing
//!   astral characters with U+E000..U+FFFF, and VALUE-MODEL.md §12 asks for
//!   *identical* answers rather than defensible ones.
//!
//! # The ABI, in one paragraph
//!
//! `lib.rs` §2 applies without exception: a `Str` parameter is three scalars
//! (`base`, `ptr`, `len`), an aggregate result is written through an
//! out-pointer, and a sum-typed result returns [`BURI_OK`] or an error index and
//! writes its payload through out-pointers. An `Option` is the sum with one
//! error variant, so *absent* is `0` and *present* is [`BURI_OK`]; that is the
//! whole of the convention, and both backends read it the same way.
//!
//! A `base` parameter is taken even where the body never reads it, so that the
//! signature is `Arg::Str` three times over and can be checked against the IR
//! mechanically rather than per entry.
//!
//! A `len` parameter arrives with VALUE-MODEL.md §3.1's ASCII flag masked off,
//! because what an entry here is handed is a byte count. [`buri_rt_str_concat`]
//! is the one exception and says why at its own definition.

use crate::memory::{
    buri_rt_alloc, buri_rt_incref, buri_rt_unique_cap, BURI_RT_GROWTH_FLOOR,
};
use crate::value::{str_of, BuriList, BuriStr, BURI_RT_STR_ASCII, BURI_RT_STR_LEN_MASK};
use crate::BURI_OK;

/// The discriminant an `Option`-returning entry answers when the value is
/// absent. `.None` is `Option`'s second variant (`option.buri:9-12`), so its
/// index in declaration order is `1` — but `lib.rs` §2 rule 3 numbers the
/// *error* arms from zero and calls the success arm [`BURI_OK`], and an
/// `Option` has exactly one error arm.
pub const BURI_ABSENT: i32 = 0;

/// Rebuild a borrowed `Str` argument.
///
/// # Safety
/// `ptr` must point at `len & BURI_RT_STR_LEN_MASK` readable bytes, or be null
/// with a zero length.
unsafe fn view<'a>(ptr: *const u8, len: u64) -> &'a [u8] {
    let n = (len & BURI_RT_STR_LEN_MASK) as usize;
    if ptr.is_null() || n == 0 {
        return &[];
    }
    // SAFETY: the caller promises `n` readable bytes at `ptr`.
    unsafe { std::slice::from_raw_parts(ptr, n) }
}

/// The same, as text. Lossless in practice — every `BuriStr` in a running
/// program came from a literal, from `from_utf8_lossy`, or from a slice of one
/// taken at a scalar boundary.
///
/// # Safety
/// As [`view`].
unsafe fn text<'a>(ptr: *const u8, len: u64) -> std::borrow::Cow<'a, str> {
    // SAFETY: forwarded.
    String::from_utf8_lossy(unsafe { view(ptr, len) })
}

/// A view of `bytes[from..to]` sharing `base`'s block, with the count taken.
///
/// The ASCII flag survives a slice of an ASCII string and is *not* recomputed
/// for a slice of a non-ASCII one: rescanning on every slice would cost exactly
/// what slicing exists to avoid (VALUE-MODEL.md §3.1).
///
/// # Safety
/// `base` is null or a live payload pointer; `ptr + from .. ptr + to` is inside
/// the same allocation.
unsafe fn slice_of(base: *mut u8, ptr: *const u8, len: u64, from: usize, to: usize) -> BuriStr {
    let n = (len & BURI_RT_STR_LEN_MASK) as usize;
    let from = from.min(n);
    let to = to.clamp(from, n);
    if to == from || ptr.is_null() {
        return BuriStr::empty();
    }
    // SAFETY: `base` is the block this view lives in, and a view handed out is
    // owned by its receiver (`lib.rs` §3).
    unsafe { buri_rt_incref(base) };
    BuriStr {
        base,
        // SAFETY: `from <= n`, so the offset is inside the allocation.
        ptr: unsafe { ptr.add(from) },
        len: (to.saturating_sub(from)) as u64 | (len & BURI_RT_STR_ASCII),
    }
}

/// The byte offset of scalar `index`, or the byte length when it is past the
/// end.
///
/// O(1) when the ASCII flag is set, and a walk over the non-continuation bytes
/// otherwise — the same fast path `str.len` takes.
fn byte_offset(bytes: &[u8], ascii: bool, index: usize) -> usize {
    if ascii {
        return index.min(bytes.len());
    }
    let mut seen = 0usize;
    for (at, b) in bytes.iter().enumerate() {
        if (b & 0xC0) != 0x80 {
            if seen == index {
                return at;
            }
            seen = seen.saturating_add(1);
        }
    }
    bytes.len()
}

/// The number of Unicode scalars in `bytes`.
fn scalar_len(bytes: &[u8], ascii: bool) -> usize {
    if ascii {
        bytes.len()
    } else {
        bytes.iter().filter(|b| (**b & 0xC0) != 0x80).count()
    }
}

/// The first byte offset at which `needle` occurs in `haystack`.
///
/// The empty needle occurs at 0, which is what `String.prototype.indexOf` says.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > haystack.len() {
        return None;
    }
    let last = haystack.len().saturating_sub(needle.len());
    (0..=last).find(|i| haystack.get(*i..i.saturating_add(needle.len())) == Some(needle))
}

/// JavaScript's `WhiteSpace` and `LineTerminator`, which is what `String.trim`
/// removes — and which is *not* Unicode `White_Space`.
///
/// ECMA-262 §12.2: the two sets differ in both directions. U+FEFF (ZWNBSP) is
/// JavaScript whitespace and is not `White_Space`; U+0085 (NEL) is
/// `White_Space` and is not JavaScript whitespace. Using Rust's
/// `char::is_whitespace` would therefore trim a different string in two places,
/// and "a different string in two places" is exactly the class of divergence
/// VALUE-MODEL.md §12 exists to rule out.
fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\u{9}'..='\u{d}'
            | '\u{20}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

/// The byte range left after trimming, as `(start, end)`.
fn trim_range(bytes: &[u8], start: bool, end: bool) -> (usize, usize) {
    let text = String::from_utf8_lossy(bytes);
    let mut from = 0usize;
    let mut to = bytes.len();
    if start {
        for c in text.chars() {
            if !is_js_space(c) {
                break;
            }
            from = from.saturating_add(c.len_utf8());
        }
    }
    if end {
        for c in text.chars().rev() {
            if !is_js_space(c) || to <= from {
                break;
            }
            to = to.saturating_sub(c.len_utf8());
        }
    }
    (from, to.max(from))
}

/// A `[Str]` whose elements are **views** into one shared block.
///
/// One allocation for the spine; every element takes a count on `base`, because
/// the generated `release` glue for a `[Str]` decrefs each element
/// independently.
///
/// # Safety
/// Every range in `pieces` must be inside the allocation `base` owns, at `ptr`.
unsafe fn list_of_views(
    base: *mut u8,
    ptr: *const u8,
    ascii: u64,
    pieces: &[(usize, usize)],
) -> BuriList {
    let stride = size_of::<BuriStr>();
    if pieces.is_empty() {
        return BuriList { ptr: std::ptr::null_mut(), len: 0 };
    }
    let block = buri_rt_alloc((pieces.len().saturating_mul(stride)) as u64);
    for (i, (from, to)) in pieces.iter().enumerate() {
        let n = to.saturating_sub(*from);
        let element = if n == 0 || ptr.is_null() {
            BuriStr::empty()
        } else {
            // SAFETY: the caller promises the range is inside `base`'s block.
            unsafe { buri_rt_incref(base) };
            // SAFETY: as above.
            BuriStr { base, ptr: unsafe { ptr.add(*from) }, len: n as u64 | ascii }
        };
        // SAFETY: `i * stride` is inside the spine block just allocated.
        unsafe { block.add(i.saturating_mul(stride)).cast::<BuriStr>().write(element) };
    }
    BuriList { ptr: block, len: pieces.len() as u64 }
}

// ---------------------------------------------------------------------------
// Pure: no allocation, and every `Str` result is a view
// ---------------------------------------------------------------------------

/// `str.charAt(self, index) -> Option<Char>`.
///
/// # Safety
/// `ptr` covers `len` bytes; `out` is writable and aligned for a `u32`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_char_at(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    index: i64,
    out: *mut u32,
) -> i32 {
    if index < 0 {
        return BURI_ABSENT;
    }
    // SAFETY: the caller promises `len` readable bytes.
    let s = unsafe { text(ptr, len) };
    let Some(c) = s.chars().nth(index as usize) else {
        return BURI_ABSENT;
    };
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(c as u32) };
    BURI_OK
}

/// `str.slice(self, start, end) -> Str`, in scalar indices, clamped at both
/// ends — `$str_slice` (`runtime.js:706-713`).
///
/// # Safety
/// `ptr` covers `len` bytes; `out` is writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_slice(
    base: *mut u8,
    ptr: *const u8,
    len: u64,
    start: i64,
    end: i64,
    out: *mut BuriStr,
) {
    // SAFETY: the caller promises `len` readable bytes.
    let bytes = unsafe { view(ptr, len) };
    let ascii = len & BURI_RT_STR_ASCII != 0;
    let lo = start.max(0) as usize;
    let hi = end.max(0) as usize;
    let from = byte_offset(bytes, ascii, lo);
    let to = byte_offset(bytes, ascii, hi.max(lo));
    // SAFETY: `from` and `to` are byte offsets this function derived from
    // `bytes`, so they are inside the block `base` owns.
    unsafe { out.write(slice_of(base, ptr, len, from, to)) }
}

/// `str.trim(self) -> Str`.
///
/// # Safety
/// As [`buri_rt_str_slice`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_trim(
    base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut BuriStr,
) {
    // SAFETY: forwarded.
    unsafe { trim_into(base, ptr, len, true, true, out) }
}

/// `str.trimStart(self) -> Str`.
///
/// # Safety
/// As [`buri_rt_str_slice`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_trim_start(
    base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut BuriStr,
) {
    // SAFETY: forwarded.
    unsafe { trim_into(base, ptr, len, true, false, out) }
}

/// `str.trimEnd(self) -> Str`.
///
/// # Safety
/// As [`buri_rt_str_slice`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_trim_end(
    base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut BuriStr,
) {
    // SAFETY: forwarded.
    unsafe { trim_into(base, ptr, len, false, true, out) }
}

/// # Safety
/// As [`buri_rt_str_slice`].
unsafe fn trim_into(
    base: *mut u8,
    ptr: *const u8,
    len: u64,
    start: bool,
    end: bool,
    out: *mut BuriStr,
) {
    // SAFETY: the caller promises `len` readable bytes.
    let bytes = unsafe { view(ptr, len) };
    let (from, to) = trim_range(bytes, start, end);
    // SAFETY: the offsets came from `bytes`, so they are inside `base`'s block.
    unsafe { out.write(slice_of(base, ptr, len, from, to)) }
}

/// `str.startsWith(self, prefix) -> Bool`.
///
/// # Safety
/// Both ranges are readable for their lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_starts_with(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    _pbase: *mut u8,
    pptr: *const u8,
    plen: u64,
) -> u8 {
    // SAFETY: the caller promises both ranges.
    let (s, p) = unsafe { (view(ptr, len), view(pptr, plen)) };
    u8::from(s.starts_with(p))
}

/// `str.endsWith(self, suffix) -> Bool`.
///
/// # Safety
/// As [`buri_rt_str_starts_with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_ends_with(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    _sbase: *mut u8,
    sptr: *const u8,
    slen: u64,
) -> u8 {
    // SAFETY: the caller promises both ranges.
    let (s, p) = unsafe { (view(ptr, len), view(sptr, slen)) };
    u8::from(s.ends_with(p))
}

/// `str.contains(self, needle) -> Bool`.
///
/// # Safety
/// As [`buri_rt_str_starts_with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_contains(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    _nbase: *mut u8,
    nptr: *const u8,
    nlen: u64,
) -> u8 {
    // SAFETY: the caller promises both ranges.
    let (s, n) = unsafe { (view(ptr, len), view(nptr, nlen)) };
    u8::from(find(s, n).is_some())
}

/// `str.indexOf(self, needle) -> Option<Int>`, in **scalar** indices.
///
/// `$str_indexOf` counts the prefix rather than reporting the byte offset, and
/// so does this.
///
/// # Safety
/// Both ranges are readable; `out` is writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_index_of(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    _nbase: *mut u8,
    nptr: *const u8,
    nlen: u64,
    out: *mut i64,
) -> i32 {
    // SAFETY: the caller promises both ranges.
    let (s, n) = unsafe { (view(ptr, len), view(nptr, nlen)) };
    let Some(at) = find(s, n) else { return BURI_ABSENT };
    let ascii = len & BURI_RT_STR_ASCII != 0;
    let prefix = s.get(..at).unwrap_or(&[]);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(scalar_len(prefix, ascii) as i64) };
    BURI_OK
}

/// `str.splitOnce(self, separator) -> Option<(Str, Str)>` — two views.
///
/// # Safety
/// Both ranges are readable; `out` is writable and aligned for **two**
/// consecutive [`BuriStr`]s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_split_once(
    base: *mut u8,
    ptr: *const u8,
    len: u64,
    _sbase: *mut u8,
    sptr: *const u8,
    slen: u64,
    out: *mut BuriStr,
) -> i32 {
    // SAFETY: the caller promises both ranges.
    let (s, sep) = unsafe { (view(ptr, len), view(sptr, slen)) };
    let Some(at) = find(s, sep) else { return BURI_ABSENT };
    let after = at.saturating_add(sep.len());
    // SAFETY: both ranges are inside `base`'s block, and `out` covers two.
    unsafe {
        out.write(slice_of(base, ptr, len, 0, at));
        out.add(1).write(slice_of(base, ptr, len, after, s.len()));
    }
    BURI_OK
}

/// `str.compare(self, other) -> Order`, as the tag `0 | 1 | 2`.
///
/// **UTF-16 code-unit order**, because `$str_compare` is JavaScript's `<` and
/// that is what `<` does. It agrees with byte order and with scalar order
/// except where one string has an astral character exactly where the other has
/// one in U+E000..U+FFFF — a surrogate code unit is `0xD800..=0xDFFF`, which
/// sorts *below* those.
///
/// # Safety
/// Both ranges are readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_compare(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    _obase: *mut u8,
    optr: *const u8,
    olen: u64,
) -> i32 {
    // SAFETY: the caller promises both ranges.
    let (a, b) = unsafe { (text(ptr, len), text(optr, olen)) };
    match a.encode_utf16().cmp(b.encode_utf16()) {
        std::cmp::Ordering::Less => 0,
        std::cmp::Ordering::Equal => 1,
        std::cmp::Ordering::Greater => 2,
    }
}

/// `Eq` on `Str`, as bytes. Identical strings have identical UTF-8, so this
/// needs no decoding.
///
/// # Safety
/// Both ranges are readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_eq(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    _obase: *mut u8,
    optr: *const u8,
    olen: u64,
) -> u8 {
    // SAFETY: the caller promises both ranges.
    let (a, b) = unsafe { (view(ptr, len), view(optr, olen)) };
    u8::from(a == b)
}

/// `str.toInt(self) -> Option<Int>` — `$str_toInt`.
///
/// Two refusals: the text must be `[+-]?\d+` after trimming, `\d` is ASCII, and
/// the value must be an `I64`. There used to be a third — the value also had to
/// be a *safe* integer, because `Int` was a double on JavaScript and this
/// backend was made to refuse what that one could not represent. `Int` is a
/// `BigInt` there now (buri-lang/buri#8), so both parse the same range: the
/// type's own.
///
/// # Safety
/// `ptr` covers `len` bytes; `out` is writable and aligned for an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_to_int(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut i64,
) -> i32 {
    // SAFETY: the caller promises `len` readable bytes.
    let s = unsafe { text(ptr, len) };
    let (from, to) = trim_range(s.as_bytes(), true, true);
    let Some(t) = s.get(from..to) else { return BURI_ABSENT };
    let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return BURI_ABSENT;
    }
    // Rust's own parser takes it from here: it accepts the same optional sign
    // and refuses anything outside `i64`, which is the whole of the range.
    let Ok(v) = t.parse::<i64>() else { return BURI_ABSENT };
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(v) };
    BURI_OK
}

/// `str.toFloat(self) -> Option<Float>` — `$str_toFloat`
/// (`runtime.js:775-779`).
///
/// The grammar is the JavaScript regex, and it is narrower than either
/// language's float literal: no `Infinity`, no `NaN`, no hexadecimal, and no
/// bare exponent. Rust's `f64` parser and `Number()` are both correctly rounded,
/// so the accepted texts get the same double from each.
///
/// # Safety
/// `ptr` covers `len` bytes; `out` is writable and aligned for an `f64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_to_float(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut f64,
) -> i32 {
    // SAFETY: the caller promises `len` readable bytes.
    let s = unsafe { text(ptr, len) };
    let (from, to) = trim_range(s.as_bytes(), true, true);
    let Some(t) = s.get(from..to) else { return BURI_ABSENT };
    if !is_js_float_literal(t) {
        return BURI_ABSENT;
    }
    let Ok(v) = t.parse::<f64>() else { return BURI_ABSENT };
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(v) };
    BURI_OK
}

/// `^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$`, as a hand-rolled scan.
fn is_js_float_literal(t: &str) -> bool {
    let bytes = t.as_bytes();
    let mut at = 0usize;
    let digits = |bytes: &[u8], at: &mut usize| {
        let start = *at;
        while bytes.get(*at).is_some_and(u8::is_ascii_digit) {
            *at = at.saturating_add(1);
        }
        *at > start
    };
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        at = 1;
    }
    // `\d+\.?\d*` or `\.\d+`.
    if bytes.get(at) == Some(&b'.') {
        at = at.saturating_add(1);
        if !digits(bytes, &mut at) {
            return false;
        }
    } else {
        if !digits(bytes, &mut at) {
            return false;
        }
        if bytes.get(at) == Some(&b'.') {
            at = at.saturating_add(1);
            digits(bytes, &mut at);
        }
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        at = at.saturating_add(1);
        if matches!(bytes.get(at), Some(b'+' | b'-')) {
            at = at.saturating_add(1);
        }
        if !digits(bytes, &mut at) {
            return false;
        }
    }
    at == bytes.len()
}

// ---------------------------------------------------------------------------
// `Alloc`-bounded, and MEMORY.md §5.3's reuse
// ---------------------------------------------------------------------------

/// `str.concat(self, ctx, other) -> Str`, with MEMORY.md §5.3's in-place
/// growth.
///
/// The one entry in this file whose result is sometimes **not** a fresh block,
/// and the counterpart of [`crate::list::buri_rt_list_concat`]'s `append_dest`
/// for the other counted payload the language has.
///
/// # Why this is a runtime entry when the other backends open-code it
///
/// The LLVM backend emits the three paths below as instructions
/// (`llvm/emit.rs`'s `concat`), because a `ccc` call would cost more than the
/// sequence saves in a release build. The copy-and-patch backend has no way to
/// emit them cheaply — a header load, two compares, three arms and a `memmove`
/// are a dozen stencils and a block layout, against one `crt` stencil for a
/// call — and a backend that always allocated where the other appends is a
/// divergence in `core/alloc`'s observable `count` and `total`, not merely a
/// missing optimisation. So the paths live here once and that backend calls
/// them, which is the shape MEMORY.md §5.3 already gives `[T]` append.
///
/// # The three paths
///
///  1. **In place.** The left operand's block is uniquely owned and has room
///     for the result past the start of the view. The right operand's bytes go
///     at `ptr + len`, the block takes one more reference, and the answer is
///     the same `base` and `ptr` with a longer length. Nothing is allocated
///     and the left operand's own bytes are not copied.
///  2. **Grown.** Uniquely owned but out of room: `max(n * 2, GROWTH_FLOOR)`
///     bytes rather than exactly `n`, so the next concatenation in a chain
///     takes path 1 and a fold that concatenates allocates O(log n) times.
///  3. **Exact.** Shared, immortal, or a literal: exactly `n` bytes. A shared
///     string is not the one being built, so it gets no speculative capacity.
///
/// [`buri_rt_unique_cap`]'s doc comment is the argument for why path 1 is
/// unobservable, and there is no separate one to make here.
///
/// The capacity test allows for a view that starts inside its block, so what
/// has to fit is `(ptr - base) + n`. The copy is `copy` rather than
/// `copy_nonoverlapping`: where the right operand is a second view into this
/// same block the two ranges can touch, and the weaker instruction removes the
/// case from the argument instead of adding a test to it.
///
/// The lengths arrive **raw** — with VALUE-MODEL.md §3.1's ASCII flag still in
/// bit 63 — rather than masked the way every other entry here takes one. The
/// flag is an input to this operation and not a tag to be stripped: a
/// concatenation is all-ASCII exactly when both halves are, so the answer's
/// flag is the `and` of the two. `stencil/rtcall.rs`'s `str_concat` is the one
/// caller and passes the words unmasked for that reason.
///
/// # Safety
/// Each `(ptr, len)` pair is readable for its masked byte length; `a_base` is
/// null or the live payload pointer of the block `a_ptr` points into; `out` is
/// writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_concat(
    a_base: *mut u8,
    a_ptr: *const u8,
    a_len: u64,
    _b_base: *mut u8,
    b_ptr: *const u8,
    b_len: u64,
    out: *mut BuriStr,
) {
    let la = (a_len & BURI_RT_STR_LEN_MASK) as usize;
    let lb = (b_len & BURI_RT_STR_LEN_MASK) as usize;
    let n = la.saturating_add(lb);
    let ascii = a_len & b_len & BURI_RT_STR_ASCII;
    if n == 0 {
        // SAFETY: the caller promises a writable, aligned destination.
        unsafe { out.write(BuriStr::empty()) };
        return;
    }
    // SAFETY: the caller promises `a_base` is null or a live payload pointer.
    if let Some(cap) = unsafe { buri_rt_unique_cap(a_base) } {
        // The view may start inside the block, so the offset of its start is
        // part of what has to fit.
        let offset = (a_ptr as usize).saturating_sub(a_base as usize);
        if offset.saturating_add(n) as u64 <= cap {
            // SAFETY: the block has room for `offset + n` bytes, so the `lb`
            // bytes at `a_ptr + la` are inside it; the ranges may touch, which
            // is what `copy` allows.
            unsafe {
                std::ptr::copy(b_ptr, a_ptr.cast_mut().add(la), lb);
                buri_rt_incref(a_base);
                out.write(BuriStr { base: a_base, ptr: a_ptr, len: n as u64 | ascii });
            }
            return;
        }
        let block = buri_rt_alloc(grown(n as u64));
        // SAFETY: a fresh block of at least `n` bytes, disjoint from both
        // sources, and each source covers its own length.
        unsafe {
            std::ptr::copy_nonoverlapping(a_ptr, block, la);
            std::ptr::copy_nonoverlapping(b_ptr, block.add(la), lb);
            out.write(BuriStr { base: block, ptr: block, len: n as u64 | ascii });
        }
        return;
    }
    let block = buri_rt_alloc(n as u64);
    // SAFETY: as above, with a block sized exactly to the result.
    unsafe {
        std::ptr::copy_nonoverlapping(a_ptr, block, la);
        std::ptr::copy_nonoverlapping(b_ptr, block.add(la), lb);
        out.write(BuriStr { base: block, ptr: block, len: n as u64 | ascii });
    }
}

/// Path 2's size: doubling with MEMORY.md §5.3's floor.
///
/// `buri_rt_grown_capacity`'s `max(needed, old_cap * 2, floor)` is the `[T]`
/// policy and this is not it — `llvm/emit.rs`'s `concat` doubles the *result*,
/// and the two surviving backends have to allocate the same number of times
/// for `core/alloc`'s counts to agree.
fn grown(needed: u64) -> u64 {
    needed.saturating_mul(2).max(BURI_RT_GROWTH_FLOOR)
}

// ---------------------------------------------------------------------------
// `Alloc`-bounded: every result is a fresh block
// ---------------------------------------------------------------------------

/// `str.split(self, ctx, separator) -> [Str]`.
///
/// The empty separator splits into scalars, which is `$str_split`'s own special
/// case; otherwise the pieces are `String.prototype.split`'s, empties included,
/// and each is a **view** into the receiver.
///
/// # Safety
/// Both ranges are readable; `out` is writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_split(
    base: *mut u8,
    ptr: *const u8,
    len: u64,
    _sbase: *mut u8,
    sptr: *const u8,
    slen: u64,
    out: *mut BuriList,
) {
    // SAFETY: the caller promises both ranges.
    let (s, sep) = unsafe { (view(ptr, len), view(sptr, slen)) };
    let ascii = len & BURI_RT_STR_ASCII;
    let mut pieces: Vec<(usize, usize)> = Vec::new();
    if sep.is_empty() {
        // Per scalar, which is `$chars`.
        // SAFETY: forwarded.
        let text = unsafe { text(ptr, len) };
        let mut at = 0usize;
        for c in text.chars() {
            let next = at.saturating_add(c.len_utf8());
            pieces.push((at, next));
            at = next;
        }
    } else {
        let mut at = 0usize;
        while let Some(found) = find(s.get(at..).unwrap_or(&[]), sep) {
            let start = at.saturating_add(found);
            pieces.push((at, start));
            at = start.saturating_add(sep.len());
        }
        pieces.push((at, s.len()));
    }
    // SAFETY: every range came from `s`, so it is inside `base`'s block.
    unsafe { out.write(list_of_views(base, ptr, ascii, &pieces)) }
}

/// `str.splitAny(self, ctx, separators) -> [Str]` — split on any of the
/// scalars in `separators`, dropping empty pieces (`$str_splitAny`).
///
/// # Safety
/// As [`buri_rt_str_split`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_split_any(
    base: *mut u8,
    ptr: *const u8,
    len: u64,
    _sbase: *mut u8,
    sptr: *const u8,
    slen: u64,
    out: *mut BuriList,
) {
    // SAFETY: the caller promises both ranges.
    let (s, seps) = unsafe { (text(ptr, len), text(sptr, slen)) };
    let set: Vec<char> = seps.chars().collect();
    let ascii = len & BURI_RT_STR_ASCII;
    let mut pieces: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    let mut at = 0usize;
    for c in s.chars() {
        let next = at.saturating_add(c.len_utf8());
        if set.contains(&c) {
            if at > start {
                pieces.push((start, at));
            }
            start = next;
        }
        at = next;
    }
    if at > start {
        pieces.push((start, at));
    }
    // SAFETY: every range came from the receiver's bytes.
    unsafe { out.write(list_of_views(base, ptr, ascii, &pieces)) }
}

/// `str.lines(self, ctx) -> [Str]` — split on `\n`, empties kept, `\r` left
/// alone, which is what `$str_lines` does.
///
/// # Safety
/// `ptr` covers `len` bytes; `out` is writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_lines(
    base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut BuriList,
) {
    // SAFETY: the caller promises `len` readable bytes.
    let s = unsafe { view(ptr, len) };
    let ascii = len & BURI_RT_STR_ASCII;
    let mut pieces: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    for (at, b) in s.iter().enumerate() {
        if *b == b'\n' {
            pieces.push((start, at));
            start = at.saturating_add(1);
        }
    }
    pieces.push((start, s.len()));
    // SAFETY: every range came from `s`.
    unsafe { out.write(list_of_views(base, ptr, ascii, &pieces)) }
}

/// `str.replace(self, ctx, needle, replacement) -> Str`.
///
/// An empty needle answers the receiver unchanged — `$str_replace`'s guard,
/// which exists because `"".split("")` would otherwise interleave the
/// replacement between every scalar.
///
/// # Safety
/// All three ranges are readable; `out` is writable and aligned for a
/// [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_replace(
    base: *mut u8,
    ptr: *const u8,
    len: u64,
    _nbase: *mut u8,
    nptr: *const u8,
    nlen: u64,
    _rbase: *mut u8,
    rptr: *const u8,
    rlen: u64,
    out: *mut BuriStr,
) {
    // SAFETY: the caller promises all three ranges.
    let (s, needle, replacement) =
        unsafe { (view(ptr, len), view(nptr, nlen), view(rptr, rlen)) };
    if needle.is_empty() {
        let n = s.len();
        // SAFETY: the whole receiver is inside `base`'s block.
        unsafe { out.write(slice_of(base, ptr, len, 0, n)) };
        return;
    }
    let mut built: Vec<u8> = Vec::with_capacity(s.len());
    let mut at = 0usize;
    while let Some(found) = find(s.get(at..).unwrap_or(&[]), needle) {
        let start = at.saturating_add(found);
        built.extend_from_slice(s.get(at..start).unwrap_or(&[]));
        built.extend_from_slice(replacement);
        at = start.saturating_add(needle.len());
    }
    built.extend_from_slice(s.get(at..).unwrap_or(&[]));
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(BuriStr::copy_from(&built)) }
}

/// `str.repeat(self, ctx, times) -> Str`. A non-positive count is zero copies,
/// as `Math.max(0, n)` makes it on JavaScript.
///
/// # Safety
/// `ptr` covers `len` bytes; `out` is writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_repeat(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    times: i64,
    out: *mut BuriStr,
) {
    // SAFETY: the caller promises `len` readable bytes.
    let s = unsafe { view(ptr, len) };
    let n = times.max(0) as usize;
    let mut built: Vec<u8> = Vec::with_capacity(s.len().saturating_mul(n));
    for _ in 0..n {
        built.extend_from_slice(s);
    }
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(BuriStr::copy_from(&built)) }
}

/// `str.toUpper(self, ctx) -> Str` — full Unicode case mapping, as
/// `String.prototype.toUpperCase` is.
///
/// # Safety
/// As [`buri_rt_str_repeat`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_to_upper(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut BuriStr,
) {
    // SAFETY: the caller promises `len` readable bytes.
    let s = unsafe { text(ptr, len) };
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&s.to_uppercase())) }
}

/// `str.toLower(self, ctx) -> Str`.
///
/// # Safety
/// As [`buri_rt_str_repeat`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_to_lower(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut BuriStr,
) {
    // SAFETY: the caller promises `len` readable bytes.
    let s = unsafe { text(ptr, len) };
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&s.to_lowercase())) }
}

/// `str.chars(self, ctx) -> [Char]` — one `i32` scalar per element, stride 4.
///
/// # Safety
/// `ptr` covers `len` bytes; `out` is writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_chars(
    _base: *mut u8,
    ptr: *const u8,
    len: u64,
    out: *mut BuriList,
) {
    // SAFETY: the caller promises `len` readable bytes.
    let s = unsafe { text(ptr, len) };
    let chars: Vec<u32> = s.chars().map(|c| c as u32).collect();
    if chars.is_empty() {
        // SAFETY: the caller promises a writable, aligned destination.
        unsafe { out.write(BuriList { ptr: std::ptr::null_mut(), len: 0 }) };
        return;
    }
    let block = buri_rt_alloc((chars.len().saturating_mul(4)) as u64);
    for (i, c) in chars.iter().enumerate() {
        // SAFETY: `i * 4` is inside the block just allocated, and the payload
        // is 16-aligned so every `u32` slot is aligned.
        unsafe { block.add(i.saturating_mul(4)).cast::<u32>().write(*c) };
    }
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(BuriList { ptr: block, len: chars.len() as u64 }) }
}

/// `str.fromChars(ctx, cs) -> Str`.
///
/// # Safety
/// `ptr` points at `count` `u32`s, or is null with `count == 0`; `out` is
/// writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_from_chars(
    ptr: *const u8,
    count: u64,
    out: *mut BuriStr,
) {
    let mut built = String::new();
    for i in 0..count {
        if ptr.is_null() {
            break;
        }
        // SAFETY: the caller promises `count` `u32`s at `ptr`.
        let raw = unsafe { ptr.add((i as usize).saturating_mul(4)).cast::<u32>().read() };
        built.push(char::from_u32(raw).unwrap_or(char::REPLACEMENT_CHARACTER));
    }
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&built)) }
}

/// `str.fromInt(ctx, n) -> Str` — decimal, no separators, `$str_fromInt`.
///
/// # Safety
/// `out` is writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_from_int(n: i64, out: *mut BuriStr) {
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&n.to_string())) }
}

/// `str.padStart(self, ctx, width, fill) -> Str`.
///
/// `fill` is a `Char` and not a `Str` (`str.buri:83`), so `$str_padStart`'s
/// `fill.repeat(n)` adds exactly `n` scalars and the result is `width` scalars
/// wide whenever it grows at all. The deficit is counted in **scalars**, which
/// is why this reads the ASCII flag rather than the byte length.
///
/// # Safety
/// `ptr` covers `len` bytes; `out` is writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_pad_start(
    base: *mut u8,
    ptr: *const u8,
    len: u64,
    width: i64,
    fill: u32,
    out: *mut BuriStr,
) {
    // SAFETY: forwarded.
    unsafe { pad(base, ptr, len, width, fill, true, out) }
}

/// `str.padEnd(self, ctx, width, fill) -> Str`.
///
/// # Safety
/// As [`buri_rt_str_pad_start`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_pad_end(
    base: *mut u8,
    ptr: *const u8,
    len: u64,
    width: i64,
    fill: u32,
    out: *mut BuriStr,
) {
    // SAFETY: forwarded.
    unsafe { pad(base, ptr, len, width, fill, false, out) }
}

/// # Safety
/// As [`buri_rt_str_pad_start`].
unsafe fn pad(
    base: *mut u8,
    ptr: *const u8,
    len: u64,
    width: i64,
    fill: u32,
    at_start: bool,
    out: *mut BuriStr,
) {
    // SAFETY: the caller promises `len` readable bytes.
    let s = unsafe { view(ptr, len) };
    let ascii = len & BURI_RT_STR_ASCII != 0;
    let have = scalar_len(s, ascii) as i64;
    let short = width.saturating_sub(have);
    if short <= 0 {
        let n = s.len();
        // SAFETY: the whole receiver is inside `base`'s block.
        unsafe { out.write(slice_of(base, ptr, len, 0, n)) };
        return;
    }
    let c = char::from_u32(fill).unwrap_or(char::REPLACEMENT_CHARACTER);
    let mut buf = [0u8; 4];
    let one = c.encode_utf8(&mut buf).as_bytes();
    let mut padding: Vec<u8> = Vec::with_capacity(one.len().saturating_mul(short as usize));
    for _ in 0..short {
        padding.extend_from_slice(one);
    }
    let mut built: Vec<u8> = Vec::with_capacity(padding.len().saturating_add(s.len()));
    if at_start {
        built.extend_from_slice(&padding);
        built.extend_from_slice(s);
    } else {
        built.extend_from_slice(s);
        built.extend_from_slice(&padding);
    }
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(BuriStr::copy_from(&built)) }
}

/// `list.join(xs, ctx, separator) -> Str`, for a `[Str]`.
///
/// Here rather than in `list.rs` because it is the one `list.*` entry whose
/// element type is fixed: `$list_join` is `xs.join(sep)`, declared on `[Str]`
/// alone, so the stride is a `BuriStr` and no descriptor is needed.
///
/// # Safety
/// `xs` points at `count` [`BuriStr`]s; the separator range is readable; `out`
/// is writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_list_join(
    xs: *const u8,
    count: u64,
    _sbase: *mut u8,
    sptr: *const u8,
    slen: u64,
    out: *mut BuriStr,
) {
    // SAFETY: the caller promises the separator range.
    let sep = unsafe { view(sptr, slen) };
    let stride = size_of::<BuriStr>();
    let mut built: Vec<u8> = Vec::new();
    for i in 0..count {
        if xs.is_null() {
            break;
        }
        if i > 0 {
            built.extend_from_slice(sep);
        }
        // SAFETY: the caller promises `count` elements at `xs`.
        let element = unsafe { &*xs.add((i as usize).saturating_mul(stride)).cast::<BuriStr>() };
        // SAFETY: an element of a live `[Str]` is a live view.
        built.extend_from_slice(unsafe { element.bytes() });
    }
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(BuriStr::copy_from(&built)) }
}

/// `str.hash(self) -> U64` — the seeded form.
///
/// `Hash::hash` takes no accumulator (`order.buri:34`): it *is* `$hash(v)`,
/// which is `$hashInto` from the FNV-1a offset basis. The seeded entry exists
/// so that a call site passes only what the Buri signature has, and the seed
/// stays one constant in one file rather than an immediate both backends
/// spell for themselves.
///
/// # Safety
/// `ptr` covers `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_hash(base: *mut u8, ptr: *const u8, len: u64) -> u64 {
    // SAFETY: forwarded.
    unsafe { crate::hash::buri_rt_hash_str(crate::hash::BURI_RT_HASH_SEED, base, ptr, len) }
}

/// `str.fromFloat(ctx, x) -> Str`.
///
/// The mangling rule (`lib.rs` §1) names this symbol for the intrinsic key
/// `str.fromFloat`, and the body is [`crate::fmt::show_f64`] — the same
/// rendering a template hole and a derived `Show` get, because `$str_fromFloat`
/// is `$f64` on the JavaScript side too.
///
/// # Safety
/// `out` is writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_from_float(x: f64, out: *mut BuriStr) {
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&crate::fmt::show_f64(x))) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::ascii_flag;

    fn borrowed(s: &str) -> (*const u8, u64) {
        (s.as_ptr(), s.len() as u64 | ascii_flag(s.as_bytes()))
    }

    #[test]
    fn a_scalar_index_is_not_a_byte_offset() {
        let bytes = "aé漢".as_bytes();
        assert_eq!(byte_offset(bytes, false, 0), 0);
        assert_eq!(byte_offset(bytes, false, 1), 1);
        assert_eq!(byte_offset(bytes, false, 2), 3);
        assert_eq!(byte_offset(bytes, false, 3), 6);
        // Past the end clamps to the byte length rather than wrapping.
        assert_eq!(byte_offset(bytes, false, 9), 6);
        assert_eq!(scalar_len(bytes, false), 3);
    }

    #[test]
    fn the_javascript_whitespace_set_is_not_the_unicode_one() {
        // In `White_Space` and not in JavaScript's: NEL.
        assert!('\u{85}'.is_whitespace());
        assert!(!is_js_space('\u{85}'));
        // In JavaScript's and not in `White_Space`: the byte-order mark.
        assert!(!'\u{feff}'.is_whitespace());
        assert!(is_js_space('\u{feff}'));
    }

    #[test]
    fn the_float_grammar_is_the_javascript_one() {
        for ok in ["1", "-1", "+1.5", ".5", "1.", "1e10", "1.5E-3", "0.0"] {
            assert!(is_js_float_literal(ok), "{ok}");
        }
        for bad in ["", "Infinity", "NaN", "0x10", "1e", "1e+", ".", "1.2.3", "1 2"] {
            assert!(!is_js_float_literal(bad), "{bad}");
        }
    }

    #[test]
    fn to_int_refuses_what_javascript_refuses() {
        let mut out = 0i64;
        let check = |s: &str, out: &mut i64| {
            let (p, l) = borrowed(s);
            // SAFETY: `p` covers `l` bytes for the life of `s`.
            unsafe { buri_rt_str_to_int(std::ptr::null_mut(), p, l, out) }
        };
        assert_eq!(check(" 42 ", &mut out), BURI_OK);
        assert_eq!(out, 42);
        assert_eq!(check("-7", &mut out), BURI_OK);
        assert_eq!(out, -7);
        assert_eq!(check("4.5", &mut out), BURI_ABSENT);
        assert_eq!(check("", &mut out), BURI_ABSENT);
        assert_eq!(check("+", &mut out), BURI_ABSENT);
        // Every `I64`, and nothing wider.
        assert_eq!(check("9007199254740992", &mut out), BURI_OK);
        assert_eq!(out, 9_007_199_254_740_992);
        assert_eq!(check("9223372036854775807", &mut out), BURI_OK);
        assert_eq!(out, i64::MAX);
        assert_eq!(check("-9223372036854775808", &mut out), BURI_OK);
        assert_eq!(out, i64::MIN);
        assert_eq!(check("9223372036854775808", &mut out), BURI_ABSENT);
        assert_eq!(check("99999999999999999999", &mut out), BURI_ABSENT);
    }

    #[test]
    fn comparison_orders_by_utf16_code_unit() {
        // U+FFFD is a single code unit; U+10000 is a surrogate pair starting at
        // 0xD800, which sorts below it. Byte order would say the opposite.
        let (ap, al) = borrowed("\u{fffd}");
        let (bp, bl) = borrowed("\u{10000}");
        // SAFETY: both ranges are live for the length of the test.
        let c = unsafe {
            buri_rt_str_compare(std::ptr::null_mut(), ap, al, std::ptr::null_mut(), bp, bl)
        };
        assert_eq!(c, 2, "U+FFFD sorts after U+10000 in UTF-16 order");
    }

    #[test]
    fn an_empty_needle_is_found_at_the_start() {
        assert_eq!(find(b"abc", b""), Some(0));
        assert_eq!(find(b"", b""), Some(0));
        assert_eq!(find(b"abc", b"c"), Some(2));
        assert_eq!(find(b"abc", b"d"), None);
    }

    /// One concatenation per step onto a uniquely-owned string reallocates
    /// O(log n) times, and the bytes are right across every seam.
    ///
    /// MEMORY.md §5.3's growth policy, asserted where the policy lives. The
    /// backend-side half — that the *call* is emitted at all — is
    /// `cli/tests/native/stencil.rs`.
    #[test]
    fn a_unique_concat_appends_in_place() {
        // The in-place licence is `buri_rt_unique_cap`'s, and it is refused
        // for a marked block — so this case and any case that sets the
        // marking latch must not run at once. `memory::latch` names both
        // sides of that rule.
        let _latch = crate::memory::latch();
        let mut acc = BuriStr::empty();
        let (bp, bl) = borrowed("xy");
        let mut allocations = 0u32;
        for _ in 0..1000 {
            let mut out = BuriStr::empty();
            // SAFETY: `acc` is this test's only reference, and "xy" is live.
            unsafe {
                buri_rt_str_concat(acc.base, acc.ptr, acc.len, std::ptr::null_mut(), bp, bl, &raw mut out);
                crate::memory::buri_rt_decref(acc.base, None);
            }
            if out.base != acc.base {
                allocations += 1;
            }
            acc = out;
        }
        assert_eq!(acc.len & BURI_RT_STR_LEN_MASK, 2000);
        assert_eq!(acc.len & BURI_RT_STR_ASCII, BURI_RT_STR_ASCII);
        // SAFETY: the block holds the two thousand bytes just written.
        assert!(unsafe { acc.bytes() }.iter().eq(b"xy".iter().cycle().take(2000)));
        assert!(allocations <= 20, "a thousand concatenations allocated {allocations} times");
        // SAFETY: the last reference.
        unsafe { crate::memory::buri_rt_free(acc.base) };
    }

    /// A block a second value still holds is not one to write into: the count
    /// is above one, so both concatenations copy and the shared string keeps
    /// its own length.
    #[test]
    fn a_shared_concat_copies() {
        let (sp, sl) = borrowed("abcd");
        let mut base = BuriStr::empty();
        // SAFETY: "abcd" is live for the length of the test.
        unsafe {
            buri_rt_str_concat(std::ptr::null_mut(), sp, sl, std::ptr::null_mut(), sp, sl, &raw mut base);
            buri_rt_incref(base.base);
        }
        let (op, ol) = borrowed("-one");
        let (tp, tl) = borrowed("-two");
        let mut a = BuriStr::empty();
        let mut b = BuriStr::empty();
        // SAFETY: `base` holds two references, so neither call may append.
        unsafe {
            buri_rt_str_concat(base.base, base.ptr, base.len, std::ptr::null_mut(), op, ol, &raw mut a);
            buri_rt_str_concat(base.base, base.ptr, base.len, std::ptr::null_mut(), tp, tl, &raw mut b);
        }
        assert_ne!(a.base, base.base, "a shared block took the in-place path");
        assert_ne!(b.base, base.base, "a shared block took the in-place path");
        // SAFETY: all three describe live blocks.
        unsafe {
            assert_eq!(base.bytes(), b"abcdabcd");
            assert_eq!(a.bytes(), b"abcdabcd-one");
            assert_eq!(b.bytes(), b"abcdabcd-two");
            crate::memory::buri_rt_free(a.base);
            crate::memory::buri_rt_free(b.base);
            crate::memory::buri_rt_decref(base.base, None);
            crate::memory::buri_rt_free(base.base);
        }
    }

    /// The ASCII flag is the conjunction, and a view that starts inside its
    /// block still has to fit the whole result.
    #[test]
    fn the_ascii_flag_is_the_conjunction_of_both_halves() {
        let (ap, al) = borrowed("ab");
        let (wp, wl) = borrowed("\u{1f600}");
        let mut mixed = BuriStr::empty();
        let mut plain = BuriStr::empty();
        // SAFETY: both literals are live for the length of the test.
        unsafe {
            buri_rt_str_concat(std::ptr::null_mut(), ap, al, std::ptr::null_mut(), wp, wl, &raw mut mixed);
            buri_rt_str_concat(std::ptr::null_mut(), ap, al, std::ptr::null_mut(), ap, al, &raw mut plain);
        }
        assert_eq!(mixed.len & BURI_RT_STR_ASCII, 0);
        assert_eq!(plain.len & BURI_RT_STR_ASCII, BURI_RT_STR_ASCII);
        // SAFETY: both are live blocks this test owns.
        unsafe {
            assert_eq!(mixed.bytes(), "ab\u{1f600}".as_bytes());
            crate::memory::buri_rt_free(mixed.base);
            crate::memory::buri_rt_free(plain.base);
        }
    }
}
