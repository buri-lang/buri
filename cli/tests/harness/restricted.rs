//! Taking permissions away from a scratch path for the length of a check.
//!
//! A directory that can't be listed or written can't be deleted either, by
//! `remove_dir_all`, the sweep or a forced worktree removal. A test that
//! takes the permissions away and gives them back on a later line leaves
//! that directory behind whenever a line in between panics. Once, that kept
//! 21 GB on disk. So the permissions come back when a [`Restricted`] drops,
//! which happens on a panic too.
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Paths whose permissions were taken away, each with the mode it had.
/// Dropping it gives every one its mode back.
#[must_use = "the permissions come back as soon as this drops"]
pub struct Restricted(Vec<(PathBuf, u32)>);

/// Sets `path`, and nothing under it, to `mode` until the guard drops.
pub fn restrict(path: &Path, mode: u32) -> Restricted {
    let had = std::fs::metadata(path).unwrap().permissions().mode();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    Restricted(vec![(path.to_path_buf(), had)])
}

/// Sets each directory under `path`, `path` included, to `dirs`, and each file
/// to `files`, until the guard drops. Links are left alone.
pub fn restrict_tree(path: &Path, dirs: u32, files: u32) -> Restricted {
    let mut found = Vec::new();
    walk(path, &mut found);
    // Children before their directory, so a directory that loses its search
    // permission doesn't hide what's under it.
    for (path, _, is_dir) in found.iter().rev() {
        let mode = if *is_dir { dirs } else { files };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    Restricted(
        found
            .into_iter()
            .map(|(path, mode, _)| (path, mode))
            .collect(),
    )
}

/// `path` and everything under it, each before its children, with its mode
/// and whether it is a directory.
fn walk(path: &Path, found: &mut Vec<(PathBuf, u32, bool)>) {
    let meta = std::fs::symlink_metadata(path).unwrap();
    if meta.file_type().is_symlink() {
        return;
    }
    found.push((path.to_path_buf(), meta.permissions().mode(), meta.is_dir()));
    if meta.is_dir() {
        for entry in std::fs::read_dir(path).unwrap() {
            walk(&entry.unwrap().path(), found);
        }
    }
}

impl Drop for Restricted {
    fn drop(&mut self) {
        // Directories before their children, so each child can be reached.
        // Best effort: a panic here, during another one, would abort.
        for (path, mode) in &self.0 {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(*mode));
        }
    }
}
