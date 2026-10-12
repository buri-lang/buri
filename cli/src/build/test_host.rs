//! The test host: the one executable every debug native test process on macOS
//! arm64 starts as (`design/PERFORMANCE.md` §6.85).
//!
//! macOS checks each new executable before its first start, so a runner linked
//! per edit paid that check on every edit. The host is linked once per
//! toolchain instead, from all of `libburi_rt.a` and a `main` that hands over
//! to `buri_rt_test_host` (`cli/runtime/loader.rs`), and a suite's code reaches
//! it as an image (`build/image.rs`):
//!
//! ```text
//! $BURI_HOME/test-host/<key>/test-host   the executable
//!                           /symbols     its external symbols, `name offset`
//!                           /probe       `ok`, or why it can't write code
//! ```
//!
//! The key is the runtime archive, the linker and the image format, so a new
//! toolchain gets a new host and an unchanged one never relinks it. Where the
//! host can't be had, a suite links a runner as before, and [`fell_back`] says
//! why for `--verbose` and `--explain`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use crate::build::buildfile::{Arch, Platform};
use crate::build::image::{self, HostSymbols};
use crate::build::link::CDriver;
use crate::compiler::backend::{Profile, Target};

/// Asks for linked runners instead. Only a build with the `test-hooks`
/// feature reads it, so tests can pin what a linked runner does.
#[cfg(feature = "test-hooks")]
pub const LINKED: &str = "BURI_TEST_LINKED";

pub struct Host {
    pub path: PathBuf,
    pub key: String,
    pub symbols: HostSymbols,
}

/// Why the first suite that linked a runner instead did.
static FELL_BACK: Mutex<Option<String>> = Mutex::new(None);

/// Whether a suite built for `target` under `profile` would load into the host.
pub fn wanted(target: Target, profile: Profile) -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64", feature = "backend-stencil"))
        & ((profile, target.platform, target.arch.unwrap_or(Arch::Arm64)) == (Profile::Debug, Platform::Macos, Arch::Arm64))
}

/// Notes that a suite linked a runner, and why.
pub fn fall_back(why: &str) {
    FELL_BACK.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert_with(|| why.to_string());
}

/// Why suites linked runners rather than loading into the host, if any did.
pub fn fell_back() -> Option<String> {
    FELL_BACK.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// The host for `target`, linked and probed if this toolchain has none yet.
pub fn for_target(target: Target) -> Result<Arc<Host>, String> {
    let linker = crate::build::link::select(target).map_err(|r| r.message)?;
    get(&linker)
}

/// The host `linker` links, once per process.
pub fn get(linker: &CDriver) -> Result<Arc<Host>, String> {
    static HOST: OnceLock<Result<Arc<Host>, String>> = OnceLock::new();
    HOST.get_or_init(|| find(linker).map(Arc::new)).clone()
}

fn find(linker: &CDriver) -> Result<Host, String> {
    #[cfg(feature = "test-hooks")]
    if std::env::var_os(LINKED).is_some() {
        return Err(format!("{LINKED} is set"));
    }
    let home = crate::build::runtime_cross::buri_home().map_err(|_| "there is no home directory to keep it in")?;
    let key = key(linker);
    let dir = home.join("test-host").join(&key);
    if let Some(host) = ready(&dir, &key) {
        return host;
    }
    let work = build(&dir, &key, linker)?;
    #[cfg(feature = "test-hooks")]
    PLACING.held();
    // Another process may have put its own there first, which is as good.
    if std::fs::rename(&work, &dir).is_err() {
        let _ = std::fs::remove_dir_all(&work);
    }
    ready(&dir, &key).unwrap_or_else(|| Err(format!("cannot keep the test host in {}", dir.display())))
}

/// Where a test holds `buri` with a host linked and not yet in place
/// ([`crate::hold`]).
#[cfg(feature = "test-hooks")]
static PLACING: crate::hold::Point = crate::hold::Point::new("test-host");

/// The host in `dir`, or why it can't be used, or `None` when there isn't one.
fn ready(dir: &Path, key: &str) -> Option<Result<Host, String>> {
    let probe = std::fs::read_to_string(dir.join("probe")).ok()?;
    if probe.trim() != "ok" {
        return Some(Err(probe.trim().to_string()));
    }
    let symbols = std::fs::read_to_string(dir.join("symbols")).ok().and_then(|t| HostSymbols::from_text(&t));
    let path = dir.join("test-host");
    Some(match symbols {
        Some(symbols) if path.is_file() => Ok(Host { path, key: key.to_string(), symbols }),
        _ => Err(format!("the test host in {} is incomplete", dir.display())),
    })
}

/// The host's key: what its bytes and symbols are a function of.
fn key(linker: &CDriver) -> String {
    let id = linker.identity();
    let text = format!(
        "test-host\0{}\0{}\0{}\0{}\0{}",
        image::VERSION,
        crate::build::actions::runtime_archive_hash(),
        id.name,
        id.version,
        id.link
    );
    crate::build::cache::hash_bytes(text.as_bytes())
}

/// Links and probes a host in a directory of its own beside `dir`, and answers
/// that directory.
fn build(dir: &Path, key: &str, linker: &CDriver) -> Result<PathBuf, String> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let parent = dir.parent().ok_or("the test host has no directory")?;
    let work = parent.join(format!(".{key}.{}.{n}", std::process::id()));
    let made = (|| -> Result<(), String> {
        let io = |e: std::io::Error| format!("cannot write the test host into {}: {e}", work.display());
        std::fs::create_dir_all(&work).map_err(io)?;
        std::fs::write(work.join("main.o"), main_object()?).map_err(io)?;
        std::fs::write(work.join(crate::compiler::backend::runtime_native::ARCHIVE_NAME), crate::compiler::backend::runtime_native::ARCHIVE)
            .map_err(io)?;
        linker.link_test_host(&work).map_err(|e| format!("the test host didn't link: {e}"))?;
        let host = work.join("test-host");
        let symbols = HostSymbols::of_executable(&std::fs::read(&host).map_err(io)?)?;
        std::fs::write(work.join("symbols"), symbols.to_text()).map_err(io)?;
        let _ = std::fs::remove_file(work.join("main.o"));
        let _ = std::fs::remove_file(work.join(crate::compiler::backend::runtime_native::ARCHIVE_NAME));
        crate::build::counted::placed(&host);
        std::fs::write(work.join("probe"), probe(&host)).map_err(io)?;
        Ok(())
    })();
    match made {
        Ok(()) => Ok(work),
        Err(why) => {
            let _ = std::fs::remove_dir_all(&work);
            Err(why)
        }
    }
}

/// `ok` when the host can write and run code, and otherwise what it said.
fn probe(host: &Path) -> String {
    let mut command = std::process::Command::new(host);
    command.env_clear().env("BURI_TEST_HOST_PROBE", "1");
    match crate::build::spawn::output(&mut command) {
        Ok(out) if out.status.success() => String::from("ok"),
        Ok(out) => {
            let said = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if said.is_empty() {
                format!("the test host's probe {}", crate::build::generators::how_it_ended(&out.status))
            } else {
                said
            }
        }
        Err(e) => format!("the test host can't be started: {e}"),
    }
}

/// The host's `main`: a branch to `buri_rt_test_host`, which takes `main`'s
/// arguments as they are.
#[cfg(feature = "backend-stencil")]
fn main_object() -> Result<Vec<u8>, String> {
    use crate::compiler::backend::stencil::object::{self, Definition, Reloc, RelKind, Section, Symbol};
    let section = Section {
        name: "__text",
        segment: "__TEXT",
        align: 2,
        zerofill: 0,
        attributes: object::CODE_ATTRIBUTES,
        // `b buri_rt_test_host`.
        data: 0x1400_0000u32.to_le_bytes().to_vec(),
        group: None,
    };
    let symbols = [
        Symbol { name: "main".into(), defined: Some(Definition { section: 0, offset: 0 }), global: true, weak: false },
        Symbol { name: "buri_rt_test_host".into(), defined: None, global: true, weak: false },
    ];
    let relocs = [Reloc { section: 0, offset: 0, kind: RelKind::Branch26, symbol: 1, addend: 0 }];
    object::write(&[section], &symbols, &relocs)
}

#[cfg(not(feature = "backend-stencil"))]
fn main_object() -> Result<Vec<u8>, String> {
    Err(String::from("this toolchain has no stencil backend"))
}
