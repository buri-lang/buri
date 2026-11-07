//! Taking back the scratch root from runs that are over.
//!
//! Most scratch is a [`super::Scratch`], which removes itself when it drops.
//! The exception is the native suites' per-process trees — `native-stencil-<pid>`,
//! `backend-agreement-<pid>`, `float-parity-<pid>` and their siblings — which
//! hold a runtime archive and a hundred-odd linked executables, are named for the
//! process so that two overlapping `cargo test` runs cannot share one, and are
//! deleted by nothing. There is no hook to delete them from: a test binary has no
//! teardown, and the reason the trees exist is that a *live* run needs them.
//!
//! So they are collected by the next run instead, which is the one thing that can
//! see them. About 180 MB per full `--test native` run accumulated to 14 GB during
//! two heavy sessions and filled a disk, and a benchmark taken on a full disk is a
//! benchmark of the disk.
//!
//! Two rules keep this from deleting evidence:
//!
//! * **`BURI_KEEP` skips the sweep entirely.** It is the flag that means "leave
//!   the scratch trees behind", and a sweep in the same process would be the
//!   toolchain's own tests arguing with it.
//! * **Age, not ownership.** A tree is taken only when nothing has written to it
//!   for [`STALE`], which no live run can manage. A directory a panicking test
//!   kept therefore survives that run and every run for hours after it, which is
//!   when somebody is reading it — `cli/tests/README.md` states the contract and
//!   this is the bound on it.
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

#[path = "kept.rs"]
pub mod kept;

/// How long a scratch tree must have been untouched to be nobody's.
///
/// A full `cargo test -p buri --features backend-llvm` is minutes, and the
/// longest single test is a soaked `BURI_FUZZ_SECONDS` run. Two hours is past
/// every one of those and short enough that a day of repeated native runs does
/// not accumulate a disk's worth.
const STALE: Duration = Duration::from_secs(2 * 60 * 60);

/// A name every process of one test run shares, for what the run shares.
///
/// `cargo nextest` starts a process per test and hands each the same
/// `NEXTEST_RUN_ID`. Under `cargo test` a binary is one process, so its id is
/// the run's.
pub fn run_name() -> String {
    std::env::var("NEXTEST_RUN_ID").unwrap_or_else(|_| std::process::id().to_string())
}

/// What `compute` answers, computed once per test run however many processes
/// ask, in `<name>-<run>` under the scratch root.
///
/// The first process to take the directory's lock computes and leaves the
/// answer beside it; every later one waits for the lock and reads it. A
/// `compute` that panics leaves no answer, so the next process to ask
/// computes again and panics the same way.
pub fn once_per_run(name: &str, compute: impl FnOnce() -> String) -> String {
    once();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", run_name()));
    std::fs::create_dir_all(&dir).unwrap();
    let lock = std::fs::File::options()
        .create(true)
        .append(true)
        .open(dir.join("lock"))
        .unwrap();
    lock.lock().unwrap();
    let answer = dir.join("answer");
    if let Ok(said) = std::fs::read_to_string(&answer) {
        return said;
    }
    let said = compute();
    // Renamed into place, so a process killed while writing leaves no half
    // of an answer for the next one to read.
    let partial = dir.join("answer.partial");
    std::fs::write(&partial, &said).unwrap();
    std::fs::rename(&partial, &answer).unwrap();
    said
}

/// Sweeps once per test binary, whenever it is called.
///
/// Best effort throughout: a tree another process is deleting at the same
/// moment, a permission error, an unreadable root — none of them is this
/// binary's problem, and none of them may fail a test. Idempotent, so a caller
/// can put it in front of every directory it creates without counting.
pub fn once() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(sweep);
}

/// One test's directory, `<tree>-<pid>/<name>` under the scratch root.
///
/// It goes when the test passes, so a run holds only the running tests' trees.
/// Like a `Scratch`, it stays when the test fails or `BURI_KEEP` is set.
pub struct Owned(PathBuf);

/// A fresh, empty [`Owned`] directory.
pub fn owned(tree: &str, name: &str) -> Owned {
    once();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("{tree}-{}", std::process::id()))
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Owned(dir)
}

impl std::ops::Deref for Owned {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for Owned {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if !std::thread::panicking() && std::env::var_os("BURI_KEEP").is_none() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn sweep() {
    if std::env::var_os("BURI_KEEP").is_some() {
        return;
    }
    sweep_dir(Path::new(env!("CARGO_TARGET_TMPDIR")), STALE);
}

/// [`sweep`] with the root and the bound named, so the rule can be asserted
/// rather than waited out — the shape `actions::claim_runner_after` uses for
/// the same reason.
///
/// Three trees are kept across runs (`kept.rs`), so they are not taken whole.
/// The store of linked programs and the cross-build homes are swept entry by
/// entry, and this binary's runtime archive is left alone, however old it is.
fn sweep_dir(root: &Path, stale: Duration) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let archive = kept::archive_dir_name();
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_dir() {
            continue;
        }
        let name = entry.file_name();
        if name == kept::PROGRAMS || name == kept::CROSS_HOMES {
            kept::sweep_store(&entry.path(), stale);
            continue;
        }
        if name.to_str() == Some(archive.as_str()) {
            continue;
        }
        // A directory's mtime moves whenever an entry is added to or removed
        // from it, so a run that is still creating cases keeps its own tree
        // fresh. It stops moving when the run ends.
        let abandoned = meta
            .modified()
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age > stale);
        if abandoned {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod sweep_tests {
    use super::*;

    /// A test's own tree goes when the test passes, and stays when it fails.
    ///
    /// Nothing else takes it inside the run, and on Linux the native suite's
    /// trees came to 10 GB, which filled the x86_64 `release` job's disk (run
    /// 42000871073).
    #[test]
    fn a_tree_goes_when_its_test_passes_and_stays_when_it_fails() {
        let passed = owned("sweep-owned-test", "passed");
        std::fs::write(passed.join("file"), b"x").unwrap();
        let path = passed.to_path_buf();
        drop(passed);
        assert!(!path.exists(), "a passing test's tree was kept");

        let failed = std::thread::spawn(|| {
            let tree = owned("sweep-owned-test", "failed");
            let path = tree.to_path_buf();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                let _tree = tree;
                panic!("a failing test");
            }));
            path
        })
        .join()
        .unwrap();
        assert!(failed.is_dir(), "a failing test's tree was taken");
        let _ = std::fs::remove_dir_all(failed.parent().unwrap());
    }

    /// The rule, both ways round: a tree nothing has written to for longer than
    /// the bound is taken, and one that is younger than it is left.
    ///
    /// The second half is the one that matters. A sweep that took everything
    /// would delete the trees of the `cargo test` run happening in the other
    /// shell — the case the per-process naming exists to make safe — and it
    /// would delete the directory a test that failed a moment ago is being read
    /// out of. Stated with the bound as a parameter, because the rule *is* the
    /// comparison: a bound of zero makes every tree abandoned and a bound of an
    /// hour makes none of them, which is the same question asked twice with a
    /// clock this test controls.
    #[test]
    fn a_tree_is_taken_only_when_nothing_has_written_to_it() {
        let root = std::env::temp_dir().join(format!(
            "buri-sweep-{}-{}",
            env!("CARGO_CRATE_NAME"),
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let tree = root.join("native-stencil-1234");
        std::fs::create_dir_all(tree.join("case")).unwrap();
        std::fs::write(root.join("loose-file"), b"not a tree").unwrap();

        sweep_dir(&root, Duration::from_secs(3600));
        assert!(
            tree.is_dir(),
            "a tree written to a moment ago is somebody's"
        );

        sweep_dir(&root, Duration::ZERO);
        assert!(!tree.exists(), "a tree older than the bound is nobody's");
        // A file is not a tree, and the sweep is over the scratch root rather
        // than over everything: whatever else is there is left alone.
        assert!(root.join("loose-file").is_file());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A second ask in the same run reads the first one's answer and computes
    /// nothing. The second ask goes past the process's own memory to the
    /// file, so it is the ask another process makes.
    #[test]
    fn a_run_computes_an_answer_once() {
        let name = format!("once-per-run-test-{}", std::process::id());
        let computed = std::cell::Cell::new(0);
        let ask = |answer: &str| {
            once_per_run(&name, || {
                computed.set(computed.get() + 1);
                answer.to_string()
            })
        };
        let first = ask("é 🙂\nover two lines");
        let second = ask("another answer");
        assert_eq!(computed.get(), 1, "the second ask computed again");
        assert_eq!(first, "é 🙂\nover two lines");
        assert_eq!(second, first, "the second ask read something else");
        let _ = std::fs::remove_dir_all(
            Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", run_name())),
        );
    }

    /// The kept trees are never taken whole. A stale program in the store is
    /// taken on its own, and this binary's runtime archive stays.
    #[test]
    fn the_kept_trees_are_swept_by_entry() {
        let root = std::env::temp_dir().join(format!(
            "buri-sweep-kept-{}-{}",
            env!("CARGO_CRATE_NAME"),
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let program = root.join(kept::PROGRAMS).join("0123abcd");
        let archive = root.join(kept::archive_dir_name());
        std::fs::create_dir_all(&program).unwrap();
        std::fs::create_dir_all(&archive).unwrap();

        sweep_dir(&root, Duration::ZERO);
        assert!(archive.is_dir(), "this binary's archive is kept");
        assert!(root.join(kept::PROGRAMS).is_dir(), "the store is kept");
        assert!(
            !program.exists(),
            "a program nothing has run for the bound is taken"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
