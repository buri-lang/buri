//! `Str` and `[T]`, as the runtime hands them across the ABI.
//!
//! VALUE-MODEL.md §3 and §4. Only the *construction* half lives here — the
//! runtime builds a `Str` whenever it reads one out of the world, and it needs
//! a `[Str]` for `readDir` and `arguments`. The `str.*` and `list.*` intrinsic
//! surface is generated code and later waves.

use crate::memory::buri_rt_alloc;

/// Bit 63 of `BuriStr::len`, set when every byte in the view is below `0x80`.
///
/// VALUE-MODEL.md §3.1. The low 63 bits are the **byte** length; the flag says
/// what `str.len()` — which is a count of Unicode scalar values, not of bytes
/// (`str.buri:17-18`) — costs. Set, and the scalar count *is* the byte count
/// and `len()` is a mask; clear, and it is a scan for bytes with
/// `(b & 0xC0) != 0x80`.
///
/// Slicing an ASCII string yields an ASCII string, so the flag survives `trim`,
/// `slice` and `splitOnce` for free. Slicing a non-ASCII string leaves the flag
/// clear even where the slice happens to be ASCII: rescanning on every slice
/// would cost the thing slicing exists to avoid.
pub const BURI_RT_STR_ASCII: u64 = 1 << 63;

/// The byte-length mask — everything except the ASCII flag.
pub const BURI_RT_STR_LEN_MASK: u64 = BURI_RT_STR_ASCII - 1;

/// `struct Str { base, ptr, len }` — 24 bytes, VALUE-MODEL.md §3.
///
/// `base` is the **payload pointer of the allocation the bytes live in**, which
/// is what `incref`/`decref` take, and is null for a literal or a static (which
/// are `IMMORTAL` anyway, so a literal string is three immediate constants and
/// touches no allocator). `ptr` is where *this view* starts, and may be in the
/// middle of somebody else's allocation — which is the whole reason `base`
/// exists, since the count cannot be found by subtracting 16 from a view.
#[repr(C)]
pub struct BuriStr {
    pub base: *mut u8,
    pub ptr: *const u8,
    pub len: u64,
}

/// `struct List { ptr, len }` — 16 bytes, VALUE-MODEL.md §4.
///
/// A list is **never a view** — every one of `slice`, `take`, `drop`, `concat`,
/// `push`, `reverse` and `filter` is `Alloc`-bounded, which is the language
/// saying they allocate — so `ptr` is always a payload start, the header is at
/// `ptr - 16`, and `len` is the element count exactly.
#[repr(C)]
pub struct BuriList {
    pub ptr: *mut u8,
    pub len: u64,
}

/// One byte, so that the empty string has an address.
///
/// VALUE-MODEL.md §6 niches `Option<Str>` on `Str::ptr`, so a null `ptr` *is*
/// `.None`. The compiler already honours that — `cranelift/emit.rs`'s `bytes`
/// appends a NUL to every literal precisely so the empty one has a non-null
/// address — and a runtime that answered a null `ptr` for an empty slice would
/// make `"".splitOnce(",")`'s first half read back as `.None`. So the empty
/// string points here.
static EMPTY: [u8; 1] = [0];

impl BuriStr {
    /// The empty string: no allocation, no base, ASCII by vacuity, and a
    /// non-null `ptr` for the reason [`EMPTY`] gives.
    pub fn empty() -> BuriStr {
        BuriStr { base: std::ptr::null_mut(), ptr: EMPTY.as_ptr(), len: BURI_RT_STR_ASCII }
    }

    /// Copy `bytes` into a fresh block and describe it.
    ///
    /// The caller is handed the only reference, with `rc == 1`, per the
    /// ownership rule in `lib.rs` §3.
    pub fn copy_from(bytes: &[u8]) -> BuriStr {
        if bytes.is_empty() {
            return BuriStr::empty();
        }
        let n = bytes.len();
        let base = buri_rt_alloc(n as u64);
        // SAFETY: `base` is a fresh block of at least `n` payload bytes and
        // cannot overlap `bytes`, which the caller owns.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), base, n) };
        BuriStr { base, ptr: base, len: n as u64 | ascii_flag(bytes) }
    }

    /// The bytes this view covers.
    ///
    /// # Safety
    /// The view must be live: `ptr` readable for its byte length.
    pub unsafe fn bytes(&self) -> &[u8] {
        let n = (self.len & BURI_RT_STR_LEN_MASK) as usize;
        if self.ptr.is_null() || n == 0 {
            return &[];
        }
        // SAFETY: the caller promises `n` readable bytes at `ptr`.
        unsafe { std::slice::from_raw_parts(self.ptr, n) }
    }

    /// The view as text.
    ///
    /// Lossless: everything that builds a `BuriStr` in this runtime goes
    /// through `from_utf8_lossy` first, so the bytes are valid UTF-8 by the
    /// time they are here. A view produced by generated code is a slice of one
    /// of those, and `core/str` slices only at scalar boundaries.
    ///
    /// # Safety
    /// As [`BuriStr::bytes`].
    pub unsafe fn as_str(&self) -> std::borrow::Cow<'_, str> {
        // SAFETY: forwarded to the caller's promise.
        String::from_utf8_lossy(unsafe { self.bytes() })
    }
}

/// Copy a `String`'s bytes into a Buri block. Consumes the Rust allocation.
pub fn str_of(s: &str) -> BuriStr {
    BuriStr::copy_from(s.as_bytes())
}

/// `BURI_RT_STR_ASCII` when every byte is below `0x80`, otherwise zero.
pub(crate) fn ascii_flag(bytes: &[u8]) -> u64 {
    if bytes.is_ascii() {
        BURI_RT_STR_ASCII
    } else {
        0
    }
}

/// A `[Str]` from a list of strings, as one block of 24-byte elements.
///
/// One allocation for the spine and one per string — VALUE-MODEL.md §5's "a
/// `[T]` of a struct is one block, not `n` structs" applies to the spine, and
/// each `Str`'s bytes are their own block because each has its own lifetime.
pub fn list_of_strs(items: &[String]) -> BuriList {
    let stride = std::mem::size_of::<BuriStr>();
    if items.is_empty() {
        return BuriList { ptr: std::ptr::null_mut(), len: 0 };
    }
    let bytes = items.len().saturating_mul(stride);
    let ptr = buri_rt_alloc(bytes as u64);
    for (i, item) in items.iter().enumerate() {
        // SAFETY: `i * stride` is within the `items.len() * stride` block, and
        // the destination is 8-aligned because the payload is 16-aligned and
        // the stride is a multiple of 8.
        unsafe { ptr.add(i * stride).cast::<BuriStr>().write(str_of(item)) };
    }
    BuriList { ptr, len: items.len() as u64 }
}

/// A `[U8]` from bytes — stride 1, so the payload is the bytes themselves.
pub fn list_of_bytes(bytes: &[u8]) -> BuriList {
    if bytes.is_empty() {
        return BuriList { ptr: std::ptr::null_mut(), len: 0 };
    }
    let ptr = buri_rt_alloc(bytes.len() as u64);
    // SAFETY: `ptr` is a fresh block of `bytes.len()` payload bytes, disjoint
    // from `bytes`.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len()) };
    BuriList { ptr, len: bytes.len() as u64 }
}

// ---------------------------------------------------------------------------
// The exported constructors
// ---------------------------------------------------------------------------

/// Copy `len` bytes into a fresh `Str`, replacing invalid UTF-8.
///
/// Lossy rather than fallible because that is what the JavaScript backend does:
/// `readFileSync(p, "utf8")` substitutes U+FFFD, and a `Str` that could hold
/// invalid UTF-8 would make every `chars()` on both backends fallible.
///
/// # Safety
/// `bytes` must point at `len` readable bytes, or be null with `len == 0`.
/// `out` must be writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_from_utf8(bytes: *const u8, len: u64, out: *mut BuriStr) {
    let src = if bytes.is_null() || len == 0 {
        &[][..]
    } else {
        // SAFETY: the caller promises `len` readable bytes.
        unsafe { std::slice::from_raw_parts(bytes, len as usize) }
    };
    let text = String::from_utf8_lossy(src);
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(str_of(&text)) }
}

/// The empty string, with no allocation.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_empty(out: *mut BuriStr) {
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(BuriStr::empty()) }
}

/// [`BURI_RT_STR_ASCII`] or zero, for generated code that built the bytes
/// itself and has to stamp the flag.
///
/// # Safety
/// `bytes` must point at `len` readable bytes, or be null with `len == 0`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_ascii_flag(bytes: *const u8, len: u64) -> u64 {
    if bytes.is_null() || len == 0 {
        return BURI_RT_STR_ASCII;
    }
    // SAFETY: the caller promises `len` readable bytes.
    ascii_flag(unsafe { std::slice::from_raw_parts(bytes, len as usize) })
}

/// The number of Unicode scalar values in a view — `str.len()`.
///
/// A mask when the ASCII flag is set, and a scan for non-continuation bytes
/// otherwise (VALUE-MODEL.md §3.1). O(1) on the input that matters, which is
/// the same shape `$str_len` has on JavaScript (`runtime.js:695-697`), where
/// the fast path is drawn at "no astral characters" instead.
///
/// # Safety
/// `bytes` must point at `len & BURI_RT_STR_LEN_MASK` readable bytes, or be
/// null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_str_scalar_len(bytes: *const u8, len: u64) -> u64 {
    let n = len & BURI_RT_STR_LEN_MASK;
    if len & BURI_RT_STR_ASCII != 0 || bytes.is_null() || n == 0 {
        return n;
    }
    // SAFETY: the caller promises `n` readable bytes.
    let src = unsafe { std::slice::from_raw_parts(bytes, n as usize) };
    src.iter().filter(|b| (**b & 0xC0) != 0x80).count() as u64
}

/// A fresh `[T]` of `count` elements at `stride` bytes each, uninitialized.
///
/// Returns the payload pointer for the caller to fill and writes the descriptor
/// to `out`. The header is at the returned pointer minus 16, as for every other
/// allocation.
///
/// # Safety
/// `out` must be writable and aligned for a [`BuriList`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_list_new(count: u64, stride: u64, out: *mut BuriList) -> *mut u8 {
    if count == 0 || stride == 0 {
        // SAFETY: the caller promises a writable, aligned destination.
        unsafe { out.write(BuriList { ptr: std::ptr::null_mut(), len: count }) };
        return std::ptr::null_mut();
    }
    let ptr = buri_rt_alloc(count.saturating_mul(stride));
    // SAFETY: as above.
    unsafe { out.write(BuriList { ptr, len: count }) };
    ptr
}
