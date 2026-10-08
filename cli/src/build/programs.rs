//! Test runners kept by their bytes, so macOS checks each program once.
//!
//! macOS holds a new executable file at its first `exec` until `syspolicyd` has
//! checked it: 0.2 s on a quiet machine, seconds under load, one file at a time
//! for the whole machine (`design/PERFORMANCE.md` §6.39). The check is per
//! file, so a runner relinked to the same bytes in a fresh repository, or after
//! `buri clean`, pays it again.
//!
//! So [`settle`] keeps the first file that held a runner's bytes in
//! `~/.buri/programs/<sha256>/program`, and turns the runner into a symbolic
//! link to it. A symbolic link rather than a hard link, because macOS drops its
//! check for a file with more than one name once nothing is running it.
//!
//! Only on macOS, where the check is. Every step is best effort: when the store
//! can't be used, the runner stays the file the linker wrote.
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How long an entry nothing has run is kept.
const STALE: Duration = Duration::from_secs(2 * 60 * 60);

/// How often a process looks for stale entries.
const SWEEP_EVERY: Duration = Duration::from_secs(10 * 60);

/// Makes the runner at `binary` run as the kept file with its bytes. Call it
/// after the runner is placed and before anything runs it.
pub fn settle(binary: &Path) {
    if !cfg!(target_os = "macos") {
        return;
    }
    if let Some(store) = store() {
        settle_in(&store, binary);
    }
}

fn store() -> Option<PathBuf> {
    Some(super::runtime_cross::buri_home().ok()?.join("programs"))
}

fn settle_in(store: &Path, binary: &Path) {
    // Already a link to an entry: `link::place_from` found the bytes there.
    if let Some(target) = std::fs::read_link(binary).ok().filter(|t| t.starts_with(store)) {
        if let Some(entry) = target.parent() {
            touch(entry);
        }
        return;
    }
    let Ok(bytes) = std::fs::read(binary) else { return };
    sweep(store);
    let entry = store.join(super::sha256::hash_bytes(&bytes));
    let kept = entry.join("program");
    // A sweep can take the entry between two of these steps, so a failed step
    // starts again, a bounded number of times.
    let mut created = false;
    for _ in 0..4 {
        if !entry.is_dir() {
            created = create(store, &entry, binary);
        }
        let Ok(lock) = File::options().append(true).open(entry.join("lock")) else { continue };
        if lock.lock_shared().is_err() {
            continue;
        }
        // Under the shared lock no sweep takes the entry, and the touch keeps
        // the next one from taking it for `STALE`.
        match std::fs::read(&kept) {
            Ok(held) if held == bytes => touch(&entry),
            // The same digest over different bytes, or an entry someone wrote
            // to. Nothing to share.
            Ok(_) => return,
            Err(_) => continue,
        }
        let partial = beside(binary);
        let _ = std::fs::remove_file(&partial);
        if std::os::unix::fs::symlink(&kept, &partial).is_ok() {
            // The runner's first start is now the kept file's: a new file only
            // when this process just made the entry from it.
            super::counted::moved(binary, created.then_some(kept.as_path()));
            if std::fs::rename(&partial, binary).is_err() {
                let _ = std::fs::remove_file(&partial);
            }
        }
        return;
    }
}

/// Builds an entry beside the store and renames it in, so no process sees an
/// entry without its program or its lock. When another process got there
/// first, the rename fails and this copy goes. Whether this process made it.
fn create(store: &Path, entry: &Path, binary: &Path) -> bool {
    let fresh = beside(&store.join("entry"));
    let _ = std::fs::remove_dir_all(&fresh);
    let built = std::fs::create_dir_all(&fresh)
        .and_then(|()| std::fs::hard_link(binary, fresh.join("program")))
        .and_then(|()| std::fs::write(fresh.join("lock"), b""))
        .and_then(|()| std::fs::rename(&fresh, entry));
    if built.is_err() {
        let _ = std::fs::remove_dir_all(&fresh);
    }
    built.is_ok()
}

/// A name beside `path` that no other placement, in this process or another,
/// is using.
fn beside(path: &Path) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!(".{name}.{}.{n}.partial", std::process::id()))
}

/// Takes stale entries, at most once per `SWEEP_EVERY` across every process.
fn sweep(store: &Path) {
    let marker = store.join(".swept");
    if !older_than(&marker, SWEEP_EVERY) && marker.exists() {
        return;
    }
    if std::fs::create_dir_all(store).is_err() || std::fs::write(&marker, b"").is_err() {
        return;
    }
    sweep_older_than(store, STALE);
}

/// Takes every entry nothing has used for `stale`, each under its lock held
/// exclusively, so an entry [`settle_in`] is handing out is never taken.
fn sweep_older_than(store: &Path, stale: Duration) {
    let Ok(entries) = std::fs::read_dir(store) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|t| t.is_dir()) || !older_than(&path, stale) {
            continue;
        }
        match File::options().append(true).open(path.join("lock")) {
            Ok(lock) => {
                if lock.try_lock().is_ok() && older_than(&path, stale) {
                    let _ = std::fs::remove_dir_all(&path);
                }
            }
            // A partial entry from a process that died while building it.
            Err(_) => {
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
}

fn touch(entry: &Path) {
    if let Ok(dir) = File::open(entry) {
        let _ = dir.set_modified(SystemTime::now());
    }
}

fn older_than(path: &Path, age: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|m| m.elapsed().ok())
        .is_some_and(|elapsed| elapsed > age)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("buri-programs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn runner(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::create_dir_all(&path).unwrap();
        let path = path.join("test-runner");
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// Two runners with the same bytes, in two repositories, run one file.
    #[test]
    fn the_same_bytes_run_from_one_file() {
        let dir = scratch("same");
        let store = dir.join("store");
        let first = runner(&dir, "a", b"program");
        let second = runner(&dir, "b", b"program");
        settle_in(&store, &first);
        settle_in(&store, &second);
        let kept = std::fs::read_link(&first).unwrap();
        assert!(kept.starts_with(&store));
        assert_eq!(std::fs::read_link(&second).unwrap(), kept);
        assert_eq!(std::fs::read(&second).unwrap(), b"program");
        // Settling a runner that is already a link changes nothing.
        settle_in(&store, &second);
        assert_eq!(std::fs::read_link(&second).unwrap(), kept);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// New bytes get an entry of their own, and the old entry keeps its bytes.
    #[test]
    fn new_bytes_get_their_own_file() {
        let dir = scratch("new");
        let store = dir.join("store");
        let first = runner(&dir, "a", b"one");
        let second = runner(&dir, "b", b"two");
        settle_in(&store, &first);
        settle_in(&store, &second);
        assert_ne!(std::fs::read_link(&first).unwrap(), std::fs::read_link(&second).unwrap());
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert_eq!(std::fs::read(&second).unwrap(), b"two");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An entry whose bytes no longer match its name is not handed out.
    #[test]
    fn an_entry_someone_wrote_to_is_not_shared() {
        let dir = scratch("tampered");
        let store = dir.join("store");
        let first = runner(&dir, "a", b"program");
        settle_in(&store, &first);
        std::fs::write(std::fs::read_link(&first).unwrap(), b"changed").unwrap();
        let second = runner(&dir, "b", b"program");
        settle_in(&store, &second);
        assert!(std::fs::read_link(&second).is_err());
        assert_eq!(std::fs::read(&second).unwrap(), b"program");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A sweep takes entries older than the bound and leaves younger ones.
    #[test]
    fn a_sweep_takes_only_stale_entries() {
        let dir = scratch("sweep");
        let store = dir.join("store");
        let first = runner(&dir, "a", b"program");
        settle_in(&store, &first);
        let entry = std::fs::read_link(&first).unwrap().parent().unwrap().to_path_buf();
        sweep_older_than(&store, Duration::from_secs(3600));
        assert!(entry.is_dir());
        sweep_older_than(&store, Duration::ZERO);
        assert!(!entry.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
