//! What more than one backend suite needs, and none of them owns.
//!
//! `cranelift`, `llvm` and `stencil` compile the same programs through three
//! pipelines and assert the same things about what came out. Where the
//! assertion is shared, the machinery has to be too: three copies of an
//! allocation probe cannot be said to agree on an allocation count, and three
//! copies of the corpus loader are three chances for one backend to be reading
//! a different repository.

// Which backends are built decides which of these are read.
#![allow(dead_code)]

use buri::build::workspace::Workspace;
use buri::diagnostics::{Diagnostics, SourceMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// A C shim linked beside the program, whose destructor reports the runtime's
/// allocation counters once `main` has returned.
///
/// `buri_rt_heap_stats` is not reachable from Buri and should not be, so an
/// assertion about *how many times a program allocated* has to be made from
/// outside it. A destructor rather than a wrapper around `main`: the emitted
/// entry point is the one `cli/runtime/lib.rs` §6 describes, and replacing it
/// would be measuring a different program.
pub const ALLOC_PROBE: &str = r#"
#include <stdio.h>
#include <stdint.h>
typedef struct { uint64_t live_blocks, live_bytes, total_blocks, total_bytes; } Stats;
extern void buri_rt_heap_stats(Stats *out);
__attribute__((destructor)) static void buri_probe(void) {
  Stats s; buri_rt_heap_stats(&s);
  fprintf(stderr, "blocks=%llu live=%llu\n",
          (unsigned long long)s.total_blocks, (unsigned long long)s.live_blocks);
}
"#;

/// `(total_blocks, live_blocks)` from an [`ALLOC_PROBE`]-linked run.
pub fn probed(stderr: &str) -> (u64, u64) {
    let line = stderr
        .lines()
        .find_map(|l| l.strip_prefix("blocks="))
        .unwrap_or_else(|| panic!("the probe printed nothing: {stderr:?}"));
    let (total, rest) = line.split_once(" live=").unwrap();
    (total.trim().parse().unwrap(), rest.trim().parse().unwrap())
}

/// What running one program produced.
pub struct Ran {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Runs a linked executable and collects what it said.
pub fn ran(binary: &Path) -> Ran {
    let out = Command::new(binary).output().unwrap();
    Ran {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

/// The conformance corpus as the repository it is, opened once per process.
///
/// Eleven files import `//lib/<package>`, so a harness that compiled each as a
/// bare snippet would be refused by the *front end* and would say nothing
/// about a backend.
pub fn conformance_repository() -> Option<&'static Workspace> {
    static REPOSITORY: OnceLock<Option<Workspace>> = OnceLock::new();
    REPOSITORY.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance");
        let mut map = SourceMap::new();
        let mut diagnostics = Diagnostics::new();
        let workspace = Workspace::load(&root, &mut map, &mut diagnostics).ok()?;
        if diagnostics.has_errors() {
            return None;
        }
        Some(workspace)
    })
    .as_ref()
}

/// The corpus root the file walkers start from.
pub fn conformance_corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance/lib")
}
