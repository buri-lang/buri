//! The musl sysroot this toolchain was built with, baked in.
//!
//! `cli/build.rs` builds `libburi_rt.a` for `<arch>-unknown-linux-musl` on a
//! Linux host, and copies the bytes that finish that link — musl's `libc.a`,
//! `libunwind.a`, and the crt objects — into `OUT_DIR` beside the archive. This
//! module is the reading end: `include_bytes!` of the same shape
//! `runtime_native::ARCHIVE` uses, for the same reason it uses it.
//!
//! **Why the bytes travel with the toolchain rather than being found on the
//! machine.** The whole point of linking against musl is that the executable
//! `buri build` produces runs on a Linux that is not this one — no
//! `ld-linux.so` to find, no glibc version to be newer than. A toolchain that
//! went looking for `/usr/lib/musl/lib/libc.a` at build time would make the
//! output depend on which musl the *developer's* machine happened to have, and
//! would fail outright on the many that have none. Baking is what makes the
//! link hermetic in the same sense the runtime archive already is.
//!
//! **What it costs is about 6.6 MB in the binary**, and that number was
//! measured rather than assumed. The obvious hope — that `rustc` bundles musl
//! into the staticlib it builds for a musl target, leaving only the crt objects
//! to bake — is false: `nm --defined-only` over that archive finds no `malloc`,
//! no `free`, no `memcpy` and no `mmap`, because rustc archives its own crates'
//! objects and leaves libc to the link. `_Unwind_*` comes back undefined by the
//! same measurement, which is why `libunwind.a` is here too even though the
//! runtime is `panic = "abort"`: `std`'s backtrace machinery references the
//! unwinder whether or not a panic ever unwinds.
//!
//! **This module is bytes, accessors and one enum, and nothing else.** Choosing
//! flags, staging the files into a directory, and deciding what to do when the
//! sysroot is absent are the linker's business and live with the linker.

/// musl's `libc.a`, or empty on a toolchain with no baked sysroot.
///
/// Empty is the signal, exactly as it is for `runtime_native::ARCHIVE`: there
/// is no `--cfg` for a `check-cfg` list to have to know about, and the file
/// exists on every host because this `include_bytes!` is unconditional.
pub const LIBC_A: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/musl/libc.a"));

/// musl's unwinder. See the module header for why a `panic = "abort"` runtime
/// still needs one.
pub const LIBUNWIND_A: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/musl/libunwind.a"));

/// The C runtime startup object for a **static PIE**, which is the one a
/// hermetic Linux executable is linked as.
pub const RCRT1_O: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/musl/rcrt1.o"));

/// The startup object for a non-PIE static executable.
pub const CRT1_O: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/musl/crt1.o"));

/// The startup object for a dynamic PIE.
pub const SCRT1_O: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/musl/Scrt1.o"));

/// The prologue half of the init/fini bracket, needed by all three modes.
pub const CRTI_O: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/musl/crti.o"));

/// The epilogue half.
pub const CRTN_O: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/musl/crtn.o"));

/// The inner bracket, for a non-PIE.
///
/// These four were left out of the first baking on the theory that
/// `crtbegin`/`crtend` belong to the compiler's runtime and that clang brings
/// its own. Measured on Debian trixie with clang 19, it does not: clang locates
/// them through a *GCC installation* found relative to `--sysroot`, and a
/// sysroot staged from these bytes has no GCC installation in it, so a link
/// without them ends at `mold: fatal: cannot open crtbeginS.o`. See
/// `cli/build.rs`'s `SYSROOT_FILES` for the full measurement.
pub const CRTBEGIN_O: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/musl/crtbegin.o"));

/// The inner bracket's prologue for a **PIE**, which is the one a hermetic
/// Linux executable uses.
pub const CRTBEGINS_O: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/musl/crtbeginS.o"));

/// The non-PIE epilogue.
pub const CRTEND_O: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/musl/crtend.o"));

/// The PIE epilogue.
pub const CRTENDS_O: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/musl/crtendS.o"));

/// One digest over all eleven members, written by `cli/build.rs` in the order
/// [`FILES`] lists them.
///
/// Sixty-four hex digits and no newline, so this is the digest and not a
/// rendering of it — the convention `runtime_native::ARCHIVE_SHA256` sets. It
/// is one digest rather than eleven because there is one question ("is this the
/// same sysroot") and it enters one cache key: a link whose sysroot changed
/// must miss, and no finer grain than that is useful.
const SYSROOT_SHA256: &str = include_str!(concat!(env!("OUT_DIR"), "/musl/sysroot.sha256"));

/// The baked sysroot's digest, for the `link` cache key.
///
/// A link's output depends on these bytes exactly as it depends on the runtime
/// archive's, so a toolchain built against a different musl must not reuse a
/// cached executable from this one.
pub fn sysroot_hash() -> String {
    SYSROOT_SHA256.to_string()
}

/// Whether this toolchain carries a musl sysroot.
///
/// False on macOS, and false on a Linux host whose musl `rust-std` was not
/// installed when the toolchain was built — `cli/build.rs` names the
/// `rustup target add` in a `cargo:warning` in that second case. [`libc`]
/// is the fuller answer; this is the one bit of it that is about *these bytes*.
pub const BAKED: bool = !LIBC_A.is_empty();

/// Every member of the sysroot, paired with the name it must be staged under.
///
/// The names are musl's own and not this repository's choice: a linker driver
/// given `-B <dir>` looks for `crt1.o` and its siblings by exactly these names,
/// so a staging step that renamed them would have to rename them back.
///
/// The order is the one [`SYSROOT_SHA256`] was taken in. Nothing reads this
/// positionally, but a reader comparing the two should not have to check.
pub const FILES: [(&str, &[u8]); 11] = [
    ("libc.a", LIBC_A),
    ("libunwind.a", LIBUNWIND_A),
    ("rcrt1.o", RCRT1_O),
    ("crt1.o", CRT1_O),
    ("Scrt1.o", SCRT1_O),
    ("crti.o", CRTI_O),
    ("crtn.o", CRTN_O),
    ("crtbegin.o", CRTBEGIN_O),
    ("crtbeginS.o", CRTBEGINS_O),
    ("crtend.o", CRTEND_O),
    ("crtendS.o", CRTENDS_O),
];

/// The C library this toolchain's runtime archive was built against.
///
/// Read from `libburi_rt.a.libc` by [`runtime_native::libc`], which is where
/// the file is; the type lives here because the sysroot above is the thing the
/// interesting variant refers to.
///
/// [`runtime_native::libc`]: crate::compiler::backend::runtime_native::libc
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Libc {
    /// musl, and this binary carries the copy to link against. The intended
    /// state of every Linux toolchain: executables are static-PIE and run on
    /// any Linux.
    MuslBaked,
    /// musl, but the sysroot is the machine's rather than this binary's. Alpine
    /// with a distro `rustc` that has no self-contained musl of its own — the
    /// archive is a musl archive and the system has the libc that finishes it.
    MuslSystem,
    /// glibc. The degraded Linux toolchain: the musl `rust-std` was missing at
    /// build time, so the runtime was built for the host triple and executables
    /// depend on this machine's glibc. `cli/build.rs` warned when it happened.
    Glibc,
    /// There is no Linux libc question here. macOS, and any host that wrote an
    /// empty archive — the fourth variant the three above needed, because "not
    /// applicable" is a different answer from any of them and reporting it as
    /// `Glibc` would put a warning in front of every macOS build.
    Absent,
}
