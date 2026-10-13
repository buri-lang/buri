//! The test host's loader: a suite's code, copied into this process and run.
//!
//! On macOS arm64, `buri test` starts every native test process from one fixed
//! executable, the test host, rather than from a runner linked per edit that
//! macOS would check before its first start (`design/PERFORMANCE.md` §6.85).
//! The host is this archive force-loaded behind a `main` that branches to
//! [`buri_rt_test_host`]. `BURI_TEST_IMAGE` names the suite's image, which
//! `buri` linked from the objects a runner is linked from. Without it, the
//! host started by a symbolic link maps `<link>.image`, which is how
//! `cli/tests/native` runs its debug programs. The format is
//! `cli/src/build/image.rs`'s.
//!
//! The image's code goes into `MAP_JIT` memory, written while this thread has
//! it writable and therefore not executable, so no page is ever both. `mprotect`
//! is refused on `MAP_JIT` memory, so the constants and the stack are ordinary
//! mappings in the same reservation, which keeps every distance inside the
//! image the one `buri` patched. What `buri` couldn't patch is a fixup here: an
//! absolute address, or a reference into this executable or the C library.
//!
//! A failure ends the process with [`CANNOT_LOAD`] and a line on standard
//! error, which `buri` reports as a runner that didn't start.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "offsets and sizes inside an image whose header bounds them: each \
              is checked against the region it lands in before it is used, and \
              addresses are formed with wrapping operations whose results the \
              range checks that follow catch"
)]

use std::ffi::{c_char, c_int, c_void};

/// What an image begins with. `cli/src/build/image.rs` writes the same bytes.
const MAGIC: &[u8; 8] = b"BURIIMG\0";
/// The format both sides read. A host's key holds this archive's hash, so a
/// host and the `buri` that built it agree.
const VERSION: u32 = 1;

/// An absolute address, eight bytes.
const ABS64: u32 = 0;
/// A `b`/`bl`, straight to its target where it reaches and through the
/// import's stub where it doesn't.
const BRANCH26: u32 = 1;
/// An import that is an offset from the host's Mach header.
const HOST: u32 = 0;
/// An import the C library answers, by name.
const NAMED: u32 = 1;
/// A fixup whose target is the image's own first byte.
const SELF: u32 = u32::MAX;

/// The exit status of a host that couldn't load its image.
const CANNOT_LOAD: i32 = 96;

const PAGE: usize = 16 * 1024;
/// A stub: `ldr x16, #8; br x16`, then the target.
const STUB_BYTES: usize = 16;
const STUB_CODE: u64 = 0x5800_0050 | (0xd61f_0200 << 32);

/// The variable naming the image.
const IMAGE: &str = "BURI_TEST_IMAGE";
/// The variable that asks the host to show it can write code, and nothing else.
const PROBE: &str = "BURI_TEST_HOST_PROBE";

type Main = unsafe extern "C" fn(c_int, *const *const c_char, *const *const c_char) -> c_int;

/// The test host's `main`.
///
/// # Safety
///
/// Called once, by the host's `main`, with the process's own arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_test_host(
    argc: c_int,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> c_int {
    if std::env::var_os(PROBE).is_some() {
        return match probe() {
            Ok(()) => 0,
            Err(why) => fail(&why),
        };
    }
    // Where `buri test` started it, the image it named. Started by another
    // name, a symbolic link to the host, the image beside that name, as the
    // runtime finds `<executable>.css`.
    let path = match std::env::var_os(IMAGE) {
        Some(path) => {
            // SAFETY: one thread, before the image's code can start another.
            unsafe { std::env::remove_var(IMAGE) };
            std::path::PathBuf::from(path)
        }
        None => std::env::current_exe()
            .unwrap_or_default()
            .with_extension("image"),
    };
    let loaded = std::fs::read(&path)
        .map_err(|e| format!("cannot read {}: {e}", std::path::Path::new(&path).display()))
        .and_then(|bytes| load(&bytes));
    match loaded {
        // SAFETY: the image's entry is the shim a linked runner starts at,
        // which takes what a C `main` takes.
        Ok(main) => unsafe { main(argc, argv, envp) },
        Err(why) => fail(&why),
    }
}

#[expect(
    clippy::print_stderr,
    reason = "the host's one line, which `buri` reports as the failed start"
)]
fn fail(why: &str) -> c_int {
    eprintln!("buri test host: {why}");
    CANNOT_LOAD
}

/// Writes and calls one function, so `buri` knows the host can before any suite
/// depends on it.
fn probe() -> Result<(), String> {
    // `mov w0, #42; ret`.
    let code = 0x5280_0540u64 | (0xd65f_03c0 << 32);
    let jit = Jit::map(std::ptr::null_mut(), PAGE)?;
    jit.writable(|text| put(text, 0, 8, |_| code))?;
    // SAFETY: the two instructions above, a C function of no arguments.
    let f = unsafe { std::mem::transmute::<*mut u8, unsafe extern "C" fn() -> c_int>(jit.base) };
    // SAFETY: as above.
    match unsafe { f() } {
        42 => Ok(()),
        other => Err(format!("the probe answered {other}")),
    }
}

/// A reader over the image's bytes.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).ok_or("the image is cut short")?;
        let got = self
            .bytes
            .get(self.at..end)
            .ok_or("the image is cut short")?;
        self.at = end;
        Ok(got)
    }

    fn u32(&mut self) -> Result<u32, String> {
        let mut word = [0u8; 4];
        word.copy_from_slice(self.take(4)?);
        Ok(u32::from_le_bytes(word))
    }

    fn u64(&mut self) -> Result<u64, String> {
        let mut word = [0u8; 8];
        word.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(word))
    }

    fn size(&mut self) -> Result<usize, String> {
        usize::try_from(self.u64()?).map_err(|_| "a size in the image is too large".to_string())
    }
}

/// One reference the image makes that `buri` couldn't patch.
struct Fixup {
    at: usize,
    kind: u32,
    import: u32,
    addend: i64,
}

/// The image's layout, out of its header.
struct Layout {
    text: usize,
    const_at: usize,
    consts: usize,
    data_at: usize,
    data: usize,
    total: usize,
    entry: usize,
    stubs: usize,
}

fn load(bytes: &[u8]) -> Result<Main, String> {
    let mut r = Reader { bytes, at: 0 };
    if r.take(8)? != MAGIC {
        return Err("the file is not a test image".into());
    }
    let version = r.u32()?;
    if version != VERSION {
        return Err(format!(
            "the image is format {version}, and this host reads {VERSION}"
        ));
    }
    let _flags = r.u32()?;
    // The host's key, which `buri` checks before it starts one.
    let _host = r.take(64)?;
    let l = Layout {
        text: r.size()?,
        const_at: r.size()?,
        consts: r.size()?,
        data_at: r.size()?,
        data: r.size()?,
        total: r.size()?,
        entry: r.size()?,
        stubs: r.size()?,
    };
    let imports = r.u32()?;
    let fixups = r.u32()?;
    let ordered = l.text <= l.const_at
        && l.const_at
            .checked_add(l.consts)
            .is_some_and(|end| end <= l.data_at)
        && l.data_at
            .checked_add(l.data)
            .is_some_and(|end| end <= l.total)
        && [l.const_at, l.data_at, l.total]
            .iter()
            .all(|at| at % PAGE == 0)
        && l.entry < l.text
        && l.stubs
            .checked_add(STUB_BYTES.saturating_mul(imports as usize))
            .is_some_and(|end| end <= l.text);
    if !ordered {
        return Err("the image's regions overlap".into());
    }
    let host = host_base();
    let mut targets = Vec::with_capacity(imports as usize);
    for _ in 0..imports {
        let kind = r.u32()?;
        let len = r.u32()? as usize;
        let value = r.u64()?;
        let name = r.take(len)?;
        targets.push(match kind {
            HOST => host.wrapping_add(value),
            NAMED => named(name)?,
            other => return Err(format!("an import of kind {other}")),
        });
    }
    let mut list = Vec::with_capacity(fixups as usize);
    for _ in 0..fixups {
        list.push(Fixup {
            at: r.size()?,
            kind: r.u32()?,
            import: r.u32()?,
            addend: r.u64()?.cast_signed(),
        });
    }
    let text = r.take(l.text)?;
    let consts = r.take(l.consts)?;
    let data = r.take(l.data)?;

    let image = Image::map(host, l.const_at, l.total)?;
    let base = image.text.base as u64;
    // SAFETY: the read-write mapping this value owns, `total - const_at` long,
    // which the header check above says holds the constants and the data.
    let rest = unsafe { std::slice::from_raw_parts_mut(image.rest, l.total - l.const_at) };
    rest.get_mut(..l.consts)
        .ok_or("the constants don't fit")?
        .copy_from_slice(consts);
    let data_at = l.data_at - l.const_at;
    rest.get_mut(data_at..data_at + l.data)
        .ok_or("the data doesn't fit")?
        .copy_from_slice(data);
    image.text.writable(|code| {
        code.get_mut(..l.text)
            .ok_or("the code doesn't fit")?
            .copy_from_slice(text);
        for f in &list {
            let target = match f.import {
                SELF => base,
                i => *targets.get(i as usize).ok_or("a fixup names no import")?,
            };
            fix(
                code,
                rest,
                &l,
                base,
                f,
                target.wrapping_add_signed(f.addend),
            )?;
        }
        Ok(())
    })?;
    image.seal_constants(l.consts)?;
    let main = (base + l.entry as u64) as usize;
    // SAFETY: the address of the image's entry shim, now executable.
    Ok(unsafe { std::mem::transmute::<usize, Main>(main) })
}

/// Applies one fixup, whose place is in `code` (at the image's start) or in
/// `rest` (from the constants on), for an image at `base`.
fn fix(
    code: &mut [u8],
    rest: &mut [u8],
    l: &Layout,
    base: u64,
    f: &Fixup,
    target: u64,
) -> Result<(), String> {
    let place = base.wrapping_add(f.at as u64);
    match f.kind {
        ABS64 if f.at < l.text => put(code, f.at, 8, |_| target),
        ABS64 if f.at >= l.const_at => put(rest, f.at - l.const_at, 8, |_| target),
        BRANCH26 if f.at < l.text => {
            let to = if reaches(place, target) {
                target
            } else {
                let stub = l.stubs + STUB_BYTES * f.import as usize;
                put(code, stub, 8, |_| STUB_CODE)?;
                put(code, stub + 8, 8, |_| target)?;
                base + stub as u64
            };
            let words = (to.wrapping_sub(place).cast_signed() >> 2).cast_unsigned();
            put(code, f.at, 4, |insn| {
                (insn & 0xfc00_0000) | (words & 0x03ff_ffff)
            })
        }
        kind => Err(format!("a fixup of kind {kind} at {:#x}", f.at)),
    }
}

/// Whether a `b`/`bl` at `place` reaches `target`: 26 bits of words.
fn reaches(place: u64, target: u64) -> bool {
    (-(1i64 << 27)..(1i64 << 27)).contains(&target.wrapping_sub(place).cast_signed())
}

/// Rewrites `width` bytes at `at` with `f` of what they held.
fn put(buffer: &mut [u8], at: usize, width: usize, f: impl Fn(u64) -> u64) -> Result<(), String> {
    let field = at
        .checked_add(width)
        .and_then(|end| buffer.get_mut(at..end))
        .ok_or("a fixup is out of range")?;
    let mut old = [0u8; 8];
    old.get_mut(..width)
        .ok_or("a fixup is too wide")?
        .copy_from_slice(field);
    let new = f(u64::from_le_bytes(old)).to_le_bytes();
    field.copy_from_slice(new.get(..width).ok_or("a fixup is too wide")?);
    Ok(())
}

unsafe extern "C" {
    fn mmap(
        addr: *mut c_void,
        len: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(addr: *mut c_void, len: usize) -> c_int;
    fn mprotect(addr: *mut c_void, len: usize, prot: c_int) -> c_int;
    fn pthread_jit_write_protect_np(enabled: c_int);
    fn sys_icache_invalidate(start: *mut c_void, len: usize);
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn _dyld_get_image_header(index: u32) -> *const u8;
}

const PROT_NONE: c_int = 0;
const PROT_READ: c_int = 1;
const PROT_WRITE: c_int = 2;
const PROT_EXEC: c_int = 4;
const MAP_PRIVATE: c_int = 0x2;
const MAP_FIXED: c_int = 0x10;
const MAP_ANON: c_int = 0x1000;
const MAP_JIT: c_int = 0x800;
const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
const LC_SEGMENT_64: u32 = 0x19;

fn failed(p: *mut c_void) -> bool {
    p as isize == -1
}

/// The host's Mach header, which every host offset is from.
fn host_base() -> u64 {
    // SAFETY: image 0 is the main executable, loaded for the process's life.
    unsafe { _dyld_get_image_header(0) as u64 }
}

/// Where the host's last segment ends, as mapped.
fn host_end(host: u64) -> u64 {
    let word = |at: u64| -> u64 {
        // SAFETY: inside the loaded header and load commands, which dyld
        // mapped for the process's life.
        unsafe { (at as *const u64).read_unaligned() }
    };
    let half = |at: u64| -> u32 {
        // SAFETY: as above.
        unsafe { (at as *const u32).read_unaligned() }
    };
    let ncmds = half(host + 16);
    let mut at = host + 32;
    let mut end = host;
    // Segments carry their link-time addresses. `__TEXT` starts at the
    // header, so the difference is the slide. `__PAGEZERO` maps nothing.
    let mut text: Option<u64> = None;
    for _ in 0..ncmds {
        let (cmd, size) = (half(at), half(at + 4));
        if cmd == LC_SEGMENT_64 {
            let (name, vmaddr, vmsize) = (word(at + 8), word(at + 24), word(at + 32));
            if name == u64::from_le_bytes(*b"__TEXT\0\0") {
                text = Some(vmaddr);
            }
            if let Some(t) = text.filter(|&t| vmaddr >= t) {
                end = end.max(host.wrapping_add(vmaddr - t).wrapping_add(vmsize));
            }
        }
        at += u64::from(size);
    }
    end
}

/// A C library symbol, by name.
fn named(name: &[u8]) -> Result<u64, String> {
    let text = String::from_utf8_lossy(name).into_owned();
    let c = std::ffi::CString::new(name).map_err(|_| format!("the import {text:?} holds a NUL"))?;
    // SAFETY: a lookup over every loaded image, by a NUL-terminated name.
    let address = unsafe { dlsym(RTLD_DEFAULT, c.as_ptr()) };
    if address.is_null() {
        return Err(format!("nothing in this process is called {text}"));
    }
    Ok(address as u64)
}

/// Code memory.
struct Jit {
    base: *mut u8,
    len: usize,
}

impl Jit {
    fn map(at: *mut c_void, len: usize) -> Result<Jit, String> {
        // SAFETY: a new private mapping, at a hint or anywhere.
        let p = unsafe {
            mmap(
                at,
                len,
                PROT_READ | PROT_WRITE | PROT_EXEC,
                MAP_PRIVATE | MAP_ANON | MAP_JIT,
                -1,
                0,
            )
        };
        if failed(p) {
            return Err(format!(
                "cannot map code memory: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Jit {
            base: p.cast(),
            len,
        })
    }

    /// Runs `f` over the code while this thread can write it, and so can't
    /// execute it, then makes what it wrote visible to instruction fetch.
    fn writable(&self, f: impl FnOnce(&mut [u8]) -> Result<(), String>) -> Result<(), String> {
        // SAFETY: `MAP_JIT` memory this value owns, writable for this thread
        // alone, and only between the two calls.
        unsafe {
            pthread_jit_write_protect_np(0);
            let done = f(std::slice::from_raw_parts_mut(self.base, self.len));
            pthread_jit_write_protect_np(1);
            sys_icache_invalidate(self.base.cast(), self.len);
            done
        }
    }
}

/// The image in memory: code first, then the constants, data and stack.
struct Image {
    text: Jit,
    rest: *mut u8,
    rest_len: usize,
}

impl Image {
    /// Reserves `total` bytes within reach of the host, `code` of them first.
    fn map(host: u64, code: usize, total: usize) -> Result<Image, String> {
        let end = host_end(host).next_multiple_of(PAGE as u64);
        let mut last = String::from("nothing was tried");
        // Right after the host, where a `bl` reaches the runtime, then further
        // out, all within a page reference's 4 GB.
        for step in 0..32u64 {
            let hint = end.wrapping_add(step * (64 << 20)) as *mut c_void;
            match Image::map_at(hint, code, total) {
                Ok(image)
                    if (image.text.base as u64).abs_diff(host) + total as u64 + (1 << 26)
                        < (1 << 32) =>
                {
                    return Ok(image);
                }
                Ok(image) => {
                    image.unmap();
                    last = "the system placed it out of reach".into();
                }
                Err(why) => last = why,
            }
        }
        Err(format!("cannot map the image near the host: {last}"))
    }

    /// A `MAP_JIT` mapping can't be placed with `MAP_FIXED`, so the reservation
    /// gives up its first `code` bytes and the code is asked for at that
    /// address, then checked.
    fn map_at(hint: *mut c_void, code: usize, total: usize) -> Result<Image, String> {
        // SAFETY: a new private reservation, then mappings over it alone.
        unsafe {
            let r = mmap(hint, total, PROT_NONE, MAP_PRIVATE | MAP_ANON, -1, 0);
            if failed(r) {
                return Err(format!(
                    "cannot reserve {total} bytes: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let rest = r.cast::<u8>().add(code);
            let rest_len = total - code;
            if rest_len > 0
                && failed(mmap(
                    rest.cast(),
                    rest_len,
                    PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANON | MAP_FIXED,
                    -1,
                    0,
                ))
            {
                munmap(r, total);
                return Err(format!(
                    "cannot map the image's data: {}",
                    std::io::Error::last_os_error()
                ));
            }
            munmap(r, code);
            match Jit::map(r, code) {
                Ok(text) if text.base == r.cast() => Ok(Image {
                    text,
                    rest,
                    rest_len,
                }),
                Ok(text) => {
                    munmap(text.base.cast(), code);
                    munmap(rest.cast(), rest_len);
                    Err("the code memory moved".into())
                }
                Err(why) => {
                    munmap(rest.cast(), rest_len);
                    Err(why)
                }
            }
        }
    }

    fn unmap(self) {
        // SAFETY: both mappings are this value's.
        unsafe {
            munmap(self.text.base.cast(), self.text.len);
            munmap(self.rest.cast(), self.rest_len);
        }
    }

    /// The constants become read-only. The data and the stack stay read-write;
    /// the image's entry puts the guard above the stack itself.
    fn seal_constants(&self, len: usize) -> Result<(), String> {
        let len = len.next_multiple_of(PAGE).min(self.rest_len);
        // SAFETY: the start of the read-write mapping this value owns.
        if len > 0 && unsafe { mprotect(self.rest.cast(), len, PROT_READ) } != 0 {
            return Err(format!(
                "cannot make the constants read-only: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Layout {
        Layout {
            text: 0x100,
            const_at: PAGE,
            consts: 16,
            data_at: 2 * PAGE,
            data: 0,
            total: 3 * PAGE,
            entry: 0,
            stubs: 0xe0,
        }
    }

    fn word(buffer: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(
            buffer
                .get(at..at + 4)
                .and_then(|b| b.try_into().ok())
                .unwrap_or_default(),
        )
    }

    /// A call the runtime is in reach of goes straight there.
    #[test]
    fn a_near_call_is_one_branch() {
        let (mut code, mut rest) = (vec![0u8; 0x100], vec![0u8; 2 * PAGE]);
        let bl = 0x9400_0000u32.to_le_bytes();
        if let Some(w) = code.get_mut(8..12) {
            w.copy_from_slice(&bl);
        }
        let f = Fixup {
            at: 8,
            kind: BRANCH26,
            import: 1,
            addend: 0,
        };
        assert!(
            fix(
                &mut code,
                &mut rest,
                &layout(),
                0x1_0000_0000,
                &f,
                0x1_0000_1008
            )
            .is_ok()
        );
        assert_eq!(word(&code, 8), 0x9400_0400);
        assert_eq!(word(&code, 0xe0 + 16), 0, "no stub");
    }

    /// One out of reach goes through the import's stub.
    #[test]
    fn a_far_call_goes_through_a_stub() {
        let (mut code, mut rest) = (vec![0u8; 0x100], vec![0u8; 2 * PAGE]);
        if let Some(w) = code.get_mut(8..12) {
            w.copy_from_slice(&0x9400_0000u32.to_le_bytes());
        }
        let f = Fixup {
            at: 8,
            kind: BRANCH26,
            import: 1,
            addend: 0,
        };
        let far = 0x2_0000_0000;
        assert!(fix(&mut code, &mut rest, &layout(), 0x1_0000_0000, &f, far).is_ok());
        // `bl` to the second stub, 0xf0 bytes on: 0xe8 bytes past the call.
        assert_eq!(word(&code, 8), 0x9400_0000 | (0xe8 >> 2));
        assert_eq!(word(&code, 0xf0), 0x5800_0050);
        assert_eq!(word(&code, 0xf4), 0xd61f_0200);
        assert_eq!(code.get(0xf8..0x100), Some(&far.to_le_bytes()[..]));
    }

    /// An address word gets the address.
    #[test]
    fn words() {
        let (mut code, mut rest) = (vec![0u8; 0x100], vec![0u8; 2 * PAGE]);
        let abs = Fixup {
            at: PAGE + 8,
            kind: ABS64,
            import: SELF,
            addend: 0,
        };
        assert!(fix(&mut code, &mut rest, &layout(), 0x1_0000_0000, &abs, 0x1234).is_ok());
        assert_eq!(rest.get(8..16), Some(&0x1234u64.to_le_bytes()[..]));
    }
}
