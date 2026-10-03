//! What the scratch root keeps from one run to the next: the runtime archive,
//! and every linked executable, by its bytes.
//!
//! macOS checks an executable file the first time it runs. The check takes
//! about 0.2 s, checks run one at a time, and the result is kept for that
//! file. A copy is a new file and is checked again. A harness that links each
//! program to a new path on every run pays for one check per program per run.
//!
//! So a linked program runs from the file that first held its bytes.
//! [`settle`] hashes a fresh executable. If an earlier link, in this run or an
//! earlier one, produced the same bytes, the fresh file is replaced with a
//! symbolic link to that earlier file. The test still compiles and links every
//! time, and it runs exactly the bytes it linked.
//!
//! A symbolic link rather than a hard link, because macOS does not keep its
//! check for a file with more than one name. Such a file is checked again each
//! time it runs after the kernel has let go of it, which under load is a few
//! minutes. A symbolic link leaves the kept file with one name.
//!
//! Two things make the bytes repeat:
//!
//! * **The runtime archive's path is the same in every run.** The macOS
//!   linker writes the path of each archive member it uses into the debug map
//!   of the executable. When the archive's directory was named for the run,
//!   no two runs linked the same bytes. [`runtime_archive`] names it for the
//!   archive's digest instead.
//! * **A program's stylesheet is kept with it.** The runtime reads
//!   `<executable>.css`, so the sheet is part of what the entry holds and part
//!   of its key.
//!
//! [`sweep_store`] takes back entries nothing has used for the sweep's bound.
//! Under a lock, so it never takes an entry while [`settle`] is handing it out.

// Every binary that sweeps compiles this, and only the native ones link.
#![allow(dead_code)]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The directory under the scratch root that holds one entry per distinct
/// executable.
pub const PROGRAMS: &str = "native-programs";

/// The runtime archive's directory, named for the archive's digest.
pub fn archive_dir_name() -> String {
    let digest = buri::compiler::backend::runtime_native::archive_hash();
    format!("runtime-archive-{}", digest.get(..16).unwrap_or(&digest))
}

/// The runtime archive, written once and kept at a path every run shares.
///
/// It's 16 MB. Processes race to write it, so each writes its own copy and
/// renames it into place, which is atomic.
pub fn runtime_archive() -> PathBuf {
    use buri::compiler::backend::runtime_native::{ARCHIVE, ARCHIVE_NAME};
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(archive_dir_name());
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(ARCHIVE_NAME);
    if !path.exists() {
        let partial = dir.join(format!("{ARCHIVE_NAME}.{}", std::process::id()));
        std::fs::write(&partial, ARCHIVE).unwrap();
        std::fs::rename(&partial, &path).unwrap();
    }
    path
}

fn store() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(PROGRAMS)
}

/// Makes `binary` run as the kept file with its bytes.
///
/// The first time a program's bytes are seen, they become a new entry in
/// [`PROGRAMS`]. Every time, `binary` is then replaced with a symbolic link to
/// the entry's file. The path does not change, so the directory a program is
/// started in and the files beside it stay where the caller put them.
///
/// Call it straight after the link, before anything runs `binary`. Best
/// effort: when the store cannot be used, `binary` stays the file the linker
/// wrote, which runs just the same.
pub fn settle(binary: &Path) {
    let Ok(bytes) = std::fs::read(binary) else { return };
    let sheet = std::fs::read(binary.with_extension("css")).ok();
    let mut keyed = bytes.clone();
    if let Some(sheet) = &sheet {
        keyed.extend_from_slice(b"\0stylesheet\0");
        keyed.extend_from_slice(sheet);
    }
    let entry = store().join(buri::build::sha256::hash_bytes(&keyed));
    let kept = entry.join("program");
    // A sweep can take the entry between two of these steps, so a failed step
    // starts again, a bounded number of times.
    for _ in 0..4 {
        if !entry.is_dir() {
            create(&entry, binary, sheet.as_deref());
        }
        let Ok(lock) = File::options().append(true).open(entry.join("lock")) else { continue };
        if lock.lock_shared().is_err() {
            continue;
        }
        // Under the shared lock, no sweep is taking the entry. The touch keeps
        // the next sweep from taking it for the sweep's bound.
        match std::fs::read(&kept) {
            Ok(held) if held == bytes => touch(&entry),
            // The same digest over different bytes. Nothing to share.
            Ok(_) => return,
            Err(_) => continue,
        }
        drop(lock);
        let partial = beside(binary);
        let _ = std::fs::remove_file(&partial);
        if std::os::unix::fs::symlink(&kept, &partial).is_ok()
            && std::fs::rename(&partial, binary).is_err()
        {
            let _ = std::fs::remove_file(&partial);
        }
        return;
    }
}

/// Builds an entry beside the store and renames it in, so no process ever
/// sees an entry without its executable, its sheet or its lock. When another
/// process got there first, the rename fails and this copy goes.
fn create(entry: &Path, binary: &Path, sheet: Option<&[u8]>) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let fresh = store().join(format!(".new-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&fresh);
    let built = std::fs::create_dir_all(&fresh)
        .and_then(|()| std::fs::hard_link(binary, fresh.join("program")))
        .and_then(|()| match sheet {
            Some(sheet) => std::fs::write(fresh.join("program.css"), sheet),
            None => Ok(()),
        })
        .and_then(|()| std::fs::write(fresh.join("lock"), b""))
        .and_then(|()| std::fs::rename(&fresh, entry));
    if built.is_err() {
        let _ = std::fs::remove_dir_all(&fresh);
    }
}

/// A name beside `binary` that no other process is using.
fn beside(binary: &Path) -> PathBuf {
    let name = binary.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    binary.with_file_name(format!(".{name}.{}.kept", std::process::id()))
}

/// Marks an entry as used now. The directory's time is set rather than the
/// executable's, so the executable's file is unchanged.
fn touch(entry: &Path) {
    if let Ok(dir) = File::open(entry) {
        let _ = dir.set_modified(std::time::SystemTime::now());
    }
}

fn older_than(path: &Path, stale: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|m| m.elapsed().ok())
        .is_some_and(|age| age > stale)
}

/// Takes every entry in `store` that nothing has used for `stale`.
///
/// An entry is taken under its lock, held exclusively, and its age is read
/// again once the lock is held. [`settle`] touches an entry under the same
/// lock, held shared, so an entry it is handing out is never taken.
pub fn sweep_store(store: &Path, stale: Duration) {
    let Ok(entries) = std::fs::read_dir(store) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if !older_than(&path, stale) {
            continue;
        }
        match File::options().append(true).open(path.join("lock")) {
            Ok(lock) => {
                if lock.try_lock().is_ok() && older_than(&path, stale) {
                    let _ = std::fs::remove_dir_all(&path);
                }
            }
            // Only an entry that was never finished has no lock.
            Err(_) => {
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
}

#[cfg(test)]
mod kept_tests {
    use super::*;

    /// Two links that produced the same bytes run as one file, and a link that
    /// produced other bytes does not.
    #[test]
    fn the_same_bytes_are_the_same_file() {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("kept-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // Unique to this process, so no other run's entry can match.
        let text = format!("kept {} {:?}", std::process::id(), std::time::SystemTime::now());
        let (a, b, c) = (root.join("a"), root.join("b"), root.join("c"));
        std::fs::write(&a, &text).unwrap();
        std::fs::write(&b, &text).unwrap();
        std::fs::write(&c, format!("{text} and more")).unwrap();
        settle(&a);
        settle(&b);
        settle(&c);
        let file = |p: &Path| std::fs::canonicalize(p).unwrap();
        assert_eq!(file(&a), file(&b));
        assert_ne!(file(&a), file(&c));
        assert_eq!(std::fs::read_to_string(&b).unwrap(), text);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A program's stylesheet is part of what it is: the same executable with
    /// another sheet is another entry, and the sheet is beside the kept file.
    #[test]
    fn the_stylesheet_is_kept_with_the_program() {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("kept-sheet-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let text = format!("sheet {} {:?}", std::process::id(), std::time::SystemTime::now());
        let (a, b) = (root.join("a/program"), root.join("b/program"));
        for (binary, sheet) in [(&a, "p { color: red }"), (&b, "p { color: blue }")] {
            std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
            std::fs::write(binary, &text).unwrap();
            std::fs::write(binary.with_extension("css"), sheet).unwrap();
            settle(binary);
        }
        let file = |p: &Path| std::fs::canonicalize(p).unwrap();
        assert_ne!(file(&a), file(&b));
        assert_eq!(std::fs::read_to_string(file(&b).with_extension("css")).unwrap(), "p { color: blue }");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The sweep takes an entry nothing has used, and leaves one whose lock is
    /// held.
    #[test]
    fn a_held_entry_is_not_swept() {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("kept-sweep-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (held, free) = (root.join("held"), root.join("free"));
        for entry in [&held, &free] {
            std::fs::create_dir_all(entry).unwrap();
            std::fs::write(entry.join("lock"), b"").unwrap();
        }
        let lock = File::options().append(true).open(held.join("lock")).unwrap();
        lock.lock_shared().unwrap();
        sweep_store(&root, Duration::ZERO);
        assert!(held.is_dir(), "an entry being handed out was taken");
        assert!(!free.exists(), "an entry nothing used was kept");
        drop(lock);
        let _ = std::fs::remove_dir_all(&root);
    }
}
