//! A test leaves the checked-in tree as it found it.
//!
//! A test that ran `buri` in a fixture wrote the fixture's `.buri`, and a
//! `cargo package --list` walking `cli/` at the same moment failed reading a
//! directory that had just gone (`ci::the_published_crate_ships_the_runtime`).
//! So each test whose scenario once wrote into the tree holds an [`Unchanged`]
//! for the fixtures it reads, and fails if any of them changed. A test that
//! goes back to working in place fails itself, whatever order the suite runs
//! in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Every entry under `root`, directories included, with its modification
/// time and length. A new `.buri`, a file written into an old one, and an
/// edited source all change it.
fn snapshot(root: &Path, out: &mut BTreeMap<PathBuf, (SystemTime, u64)>) {
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        let meta = std::fs::symlink_metadata(&path).unwrap();
        out.insert(path.clone(), (meta.modified().unwrap(), meta.len()));
        if meta.is_dir() {
            snapshot(&path, out);
        }
    }
}

/// The fixtures a test reads, as they were when it started. When it is
/// dropped at the end of a test that passed, it asserts none of them changed.
pub struct Unchanged {
    fixtures: Vec<PathBuf>,
    before: BTreeMap<PathBuf, (SystemTime, u64)>,
}

impl Unchanged {
    /// `fixtures` are paths from the checkout's root.
    pub fn new(fixtures: &[&str]) -> Unchanged {
        let fixtures: Vec<PathBuf> = fixtures
            .iter()
            .map(|f| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .unwrap()
                    .join(f)
            })
            .collect();
        let mut before = BTreeMap::new();
        for f in &fixtures {
            snapshot(f, &mut before);
        }
        Unchanged { fixtures, before }
    }
}

impl Drop for Unchanged {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let mut after = BTreeMap::new();
        for f in &self.fixtures {
            snapshot(f, &mut after);
        }
        let changed: Vec<String> = after
            .iter()
            .filter(|(path, stamp)| self.before.get(*path) != Some(stamp))
            .map(|(path, _)| path.display().to_string())
            .chain(
                self.before
                    .keys()
                    .filter(|path| !after.contains_key(*path))
                    .map(|path| format!("{} (removed)", path.display())),
            )
            .collect();
        assert!(
            changed.is_empty(),
            "the test wrote into a checked-in fixture; it must work on a copy:\n{}",
            changed.join("\n")
        );
    }
}
