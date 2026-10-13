//! A `buri` knows which build of itself is running from its own executable's
//! header, and its cache is keyed by that. Copies of `buri` whose header is
//! unusual, but which still run, still cache.
use crate::harness::*;
use std::path::Path;

/// A repository with one Node binary, the cheapest thing to build twice.
fn repository() -> Scratch {
    let scratch = Scratch::repo("executable-identity");
    scratch.write(
        "cmd/app/BUILD.buri",
        "binary {\n    outputs: [{ platform: \"node\" }]\n}\n",
    );
    scratch.write(
        "cmd/app/main.buri",
        "from \"node\" import { NodeHost };\n\nexport fn main(host: NodeHost): Result<(), Str> {\n    .Ok(())\n}\n",
    );
    scratch
}

/// Builds twice with `exe` and asserts the second build was served from the
/// cache, so `exe` knew itself both times.
fn caches(scratch: &Scratch, exe: &Path) {
    // Under the coverage gate, a copy's profiles can't share the pooled file of
    // the `buri` it was copied from (`build_driver.rs` says why).
    let profile = std::env::var_os("LLVM_PROFILE_FILE")
        .map(|pool| Path::new(&pool).with_file_name("%m_no_build_id_%p.profraw"));
    let build = || {
        let mut command = std::process::Command::new(exe);
        if let Some(profile) = &profile {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        let out = heap_checked(&mut command)
            .current_dir(&scratch.root)
            .args(["build", "--color=never", "//cmd/app"])
            .env("BURI_HOME", scratch.path("home"))
            .env("XDG_CACHE_HOME", scratch.path("xdg"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    assert!(
        !build().contains("cached"),
        "{} found a build it never made",
        exe.display()
    );
    assert!(
        build().contains("cached"),
        "{} didn't know itself the second time",
        exe.display()
    );
}

/// A universal (fat) Mach-O has no `LC_UUID` at its start, so `buri` knows
/// itself by the hash of its bytes instead.
#[cfg(target_os = "macos")]
#[test]
fn a_universal_buri_still_caches() {
    let scratch = repository();
    let fat = scratch.path("bin/buri-universal");
    std::fs::create_dir_all(fat.parent().unwrap()).unwrap();
    let made = std::process::Command::new("lipo")
        .arg("-create")
        .arg(buri())
        .arg("-output")
        .arg(&fat)
        .status()
        .unwrap();
    assert!(made.success(), "lipo couldn't wrap buri");
    caches(&scratch, &fat);
}

/// A `buri` whose note segments claim 8-byte alignment reads its notes at that
/// alignment, and still caches.
#[cfg(target_os = "linux")]
#[test]
fn a_buri_with_widely_aligned_notes_still_caches() {
    let mut bytes = std::fs::read(buri()).unwrap();
    let le = |bytes: &[u8], at: usize, len: usize| {
        bytes[at..at + len]
            .iter()
            .rev()
            .fold(0usize, |acc, &b| acc << 8 | usize::from(b))
    };
    // The program header table: offset, entry size and count.
    let (table, entry, count) = (le(&bytes, 32, 8), le(&bytes, 54, 2), le(&bytes, 56, 2));
    let mut widened = 0;
    for header in (0..count).map(|i| table + i * entry) {
        // A `PT_NOTE` at 4-byte alignment. The loader doesn't read the field.
        if le(&bytes, header, 4) == 4 && le(&bytes, header + 48, 8) == 4 {
            bytes[header + 48] = 8;
            widened += 1;
        }
    }
    assert!(widened > 0, "buri has no 4-byte-aligned note segment");
    let scratch = repository();
    let copy = scratch.path("bin/buri-wide");
    write_executable(&copy, bytes);
    caches(&scratch, &copy);
}
