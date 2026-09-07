//! `ui/testing`'s `snapshot` — paint the scene, then compare or record.
//!
//! `ui/node`'s `describe` resolves a tree to a scene document and hands three
//! strings across: the snapshot's name, the scene, and the pseudo-class to
//! paint in. [`crate::paint`] turns those into PNG bytes. What is left is the
//! verdict, and it is all here.
//!
//! `buri test` sets three environment variables, and
//! `cli/src/commands/test.rs` is the other half of each:
//!
//!   * `BURI_SNAPSHOT_DIR` — the package's `test/__snapshots__`. **Absent means
//!     nobody set the directory up**, which is a toolchain bug rather than a
//!     failing test, so it aborts and says so.
//!   * `BURI_SNAPSHOT_UPDATE=1` — `buri test --update`. Write the golden
//!     instead of comparing it.
//!   * `BURI_SNAPSHOT_SHEET` — a file holding the stylesheet the compiler
//!     extracted. Absent, or unreadable, is an empty sheet: a program with no
//!     static styles writes none, and that has to paint rather than fail.
//!
//! A changed snapshot **fails the test**; it does not report a toolchain
//! problem. That is the same path a failed assertion takes — [`crate::abort::die`],
//! which notes the block for the runner before it exits — so a changed
//! snapshot reads like a failed `assert` and the run's counts are right.

/// The directory a package's goldens live in.
const DIR: &str = "BURI_SNAPSHOT_DIR";

/// Set to `1` when `buri test --update` asked for the golden to be written.
const UPDATE: &str = "BURI_SNAPSHOT_UPDATE";

/// The file holding the stylesheet this artifact's static styles extracted to.
const SHEET: &str = "BURI_SNAPSHOT_SHEET";

/// `ui/testing`'s `paint(name, scene, state)`.
///
/// # Safety
/// Each pointer must address its byte length, or be null with a zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_ui_testing_paint(
    _name_base: *mut u8,
    name_ptr: *const u8,
    name_len: u64,
    _scene_base: *mut u8,
    scene_ptr: *const u8,
    scene_len: u64,
    _state_base: *mut u8,
    state_ptr: *const u8,
    state_len: u64,
) {
    // SAFETY: the caller promises each pointer addresses its length.
    let (name, scene, state) = unsafe {
        (
            crate::host::text(name_ptr, name_len),
            crate::host::text(scene_ptr, scene_len),
            crate::host::text(state_ptr, state_len),
        )
    };
    compare(&name, &scene, &state);
}

/// Paints the scene, then either records the golden or compares against it.
fn compare(name: &str, scene: &str, state: &str) {
    if let Some(complaint) = unusable(name) {
        crate::abort::die(&[complaint.as_bytes()]);
    }
    let Ok(dir) = std::env::var(DIR) else {
        crate::abort::die(&[b"no snapshot directory: buri test did not set BURI_SNAPSHOT_DIR"])
    };
    let sheet = stylesheet();
    let request = crate::paint::Request { scene, stylesheet: &sheet, state };
    let actual = match crate::paint::render(&request) {
        Ok(bytes) => bytes,
        Err(why) => crate::abort::die(&[
            b"cannot paint the snapshot \"",
            name.as_bytes(),
            b"\": ",
            why.as_bytes(),
        ]),
    };
    let golden = std::path::Path::new(&dir).join(format!("{name}.png"));
    let beside = std::path::Path::new(&dir).join(format!("{name}.diff.png"));

    if std::env::var(UPDATE).is_ok_and(|v| v == "1") {
        let _ = std::fs::create_dir_all(&dir);
        if let Err(e) = std::fs::write(&golden, &actual) {
            crate::abort::die(&[
                b"cannot write ",
                golden.display().to_string().as_bytes(),
                b": ",
                e.to_string().as_bytes(),
            ]);
        }
        // A golden that has just been recorded has no difference to show.
        let _ = std::fs::remove_file(&beside);
        return;
    }

    let Ok(recorded) = std::fs::read(&golden) else {
        crate::abort::die(&[
            b"no snapshot for \"",
            name.as_bytes(),
            b"\": run buri test --update to record one",
        ])
    };
    match crate::paint::diff(&recorded, &actual) {
        // Equal, and any difference left over from a previous run goes with it,
        // so a fixed snapshot does not leave a diff behind.
        Ok(None) => {
            let _ = std::fs::remove_file(&beside);
        }
        Ok(Some(image)) => {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&beside, &image);
            crate::abort::die(&[
                b"the snapshot \"",
                name.as_bytes(),
                b"\" changed: see test/__snapshots__/",
                name.as_bytes(),
                b".diff.png",
            ]);
        }
        Err(why) => crate::abort::die(&[
            b"cannot compare the snapshot \"",
            name.as_bytes(),
            b"\": ",
            why.as_bytes(),
        ]),
    }
}

/// The stylesheet, out of the file `buri test` named.
fn stylesheet() -> String {
    let Ok(path) = std::env::var(SHEET) else { return String::new() };
    match std::fs::read(&path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => String::new(),
    }
}

/// What is wrong with `name`, or `None` when it is a file name and nothing
/// else.
///
/// A snapshot names a file in one directory. A separator or a `..` in it would
/// name a file somewhere else, and a test that writes outside its own package
/// is not a test. An empty name would name the directory itself.
///
/// **Two sentences rather than one**, because a name has two ways to be wrong
/// and a reader who wrote `snapshot(ctx, "", …)` is not helped by being told
/// about path separators.
fn unusable(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("a snapshot name may not be empty");
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Some("a snapshot name may not contain a path separator or `..`");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_name_is_usable() {
        assert_eq!(unusable("card"), None);
        assert_eq!(unusable("card.hovered"), None);
        assert_eq!(unusable("card-2_x"), None);
        // A name is a file name, not an ASCII one.
        assert_eq!(unusable("снимок"), None);
    }

    #[test]
    fn a_name_that_could_leave_the_directory_is_refused() {
        for name in ["a/b", "a\\b", "..", "../golden", "a/../b"] {
            assert_eq!(
                unusable(name),
                Some("a snapshot name may not contain a path separator or `..`"),
                "{name}"
            );
        }
    }

    #[test]
    fn an_empty_name_says_it_is_empty() {
        assert_eq!(unusable(""), Some("a snapshot name may not be empty"));
    }
}
