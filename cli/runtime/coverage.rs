//! `buri test --coverage`'s counts (`design/COVERAGE.md`).
//!
//! `middle::coverage` puts a [`buri_rt_coverage_hit`] in front of every line it
//! counts, and only in a coverage build, so a plain binary never calls this.
//! On exit the counts go to a new file in the directory `BURI_COVERAGE` names,
//! one `key count` line each, which is what `runtime.js`'s `$coverage_hit`
//! writes too. An abort leaves through `exit`, so its lines count as well.

use std::collections::HashMap;
use std::io::Write as _;
use std::sync::{Mutex, PoisonError};

static COUNTS: Mutex<Option<HashMap<i64, u64>>> = Mutex::new(None);

unsafe extern "C" {
    fn atexit(f: extern "C" fn()) -> i32;
}

/// One more pass over the line `key` names.
#[unsafe(no_mangle)]
pub extern "C" fn buri_rt_coverage_hit(key: i64) {
    let mut counts = COUNTS.lock().unwrap_or_else(PoisonError::into_inner);
    let counts = counts.get_or_insert_with(|| {
        // SAFETY: `write` is an `extern "C" fn()` taking no arguments and
        // returning normally, which is the whole of `atexit`'s contract.
        unsafe { atexit(write) };
        HashMap::new()
    });
    let count = counts.entry(key).or_insert(0);
    *count = count.saturating_add(1);
}

extern "C" fn write() {
    let counts = COUNTS.lock().unwrap_or_else(PoisonError::into_inner);
    let (Some(counts), Some(dir)) = (counts.as_ref(), std::env::var_os("BURI_COVERAGE")) else {
        return;
    };
    let mut text = String::new();
    for (key, count) in counts {
        text.push_str(&format!("{key} {count}\n"));
    }
    let dir = std::path::PathBuf::from(dir);
    let pid = std::process::id();
    for i in 0u32.. {
        let path = dir.join(format!("{pid}-{i}.hits"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let _ = file.write_all(text.as_bytes());
                return;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return,
        }
    }
}
