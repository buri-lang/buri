//! Taking back the scratch root from runs that are over.
//!
//! Most scratch is a [`super::Scratch`], which removes itself when it drops.
//! The exception is the native suites' per-process trees — `native-cranelift-<pid>`,
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
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

/// How long a scratch tree must have been untouched to be nobody's.
///
/// A full `cargo test -p buri --features backend-llvm` is minutes, and the
/// longest single test is a soaked `BURI_FUZZ_SECONDS` run. Two hours is past
/// every one of those and short enough that a day of repeated native runs does
/// not accumulate a disk's worth.
const STALE: Duration = Duration::from_secs(2 * 60 * 60);

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

fn sweep() {
    if std::env::var_os("BURI_KEEP").is_some() {
        return;
    }
    sweep_dir(Path::new(env!("CARGO_TARGET_TMPDIR")), STALE);
}

/// [`sweep`] with the root and the bound named, so the rule can be asserted
/// rather than waited out — the shape `actions::claim_runner_after` uses for
/// the same reason.
fn sweep_dir(root: &Path, stale: Duration) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_dir() {
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
        let root = std::env::temp_dir()
            .join(format!("buri-sweep-{}-{}", env!("CARGO_CRATE_NAME"), std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = root.join("native-cranelift-1234");
        std::fs::create_dir_all(tree.join("case")).unwrap();
        std::fs::write(root.join("loose-file"), b"not a tree").unwrap();

        sweep_dir(&root, Duration::from_secs(3600));
        assert!(tree.is_dir(), "a tree written to a moment ago is somebody's");

        sweep_dir(&root, Duration::ZERO);
        assert!(!tree.exists(), "a tree older than the bound is nobody's");
        // A file is not a tree, and the sweep is over the scratch root rather
        // than over everything: whatever else is there is left alone.
        assert!(root.join("loose-file").is_file());

        let _ = std::fs::remove_dir_all(&root);
    }
}
