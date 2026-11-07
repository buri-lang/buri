//! Copies the `cpu.rs` of the `graviola` this build compiles into `OUT_DIR`,
//! for `tls::tests::the_feature_check_matches_what_graviola_asserts` to
//! `include_str!`.
//!
//! Read here rather than by the test at run time because the test binary runs
//! where the cargo registry may not be: Memcheck builds its binaries in one job
//! and runs them in another. The build always has the source, since it
//! compiles it.
//!
//! Found on disk rather than through `cargo metadata`: that resolves the whole
//! workspace, so on a cold `~/.cargo` it needs crates this build never
//! downloaded, and `--offline` then fails.

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a build script fails by panicking, and cargo prints the message; \
              each one here names what was missing"
)]

use std::path::{Path, PathBuf};

fn main() {
    let manifest =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo's manifest dir"));
    let lock_path = manifest.join("../Cargo.lock");
    println!("cargo::rerun-if-changed={}", lock_path.display());
    println!("cargo::rerun-if-env-changed=CARGO_HOME");
    if std::env::var_os("CARGO_FEATURE_NET").is_none() {
        return;
    }
    let lock = std::fs::read_to_string(&lock_path).expect("the workspace's Cargo.lock");
    let version = lock
        .split("[[package]]")
        .find_map(|entry| {
            let mut fields = entry.lines().map(str::trim);
            fields.find(|line| *line == "name = \"graviola\"")?;
            fields
                .find_map(|line| line.strip_prefix("version = \""))?
                .strip_suffix('"')
        })
        .expect("Cargo.lock pins graviola");
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(&std::env::var_os("HOME").expect("HOME")).join(".cargo"));
    // One directory per registry, and cargo extracted graviola into one.
    let registries = home.join("registry/src");
    let source_dir = std::fs::read_dir(&registries)
        .into_iter()
        .flatten()
        .flatten()
        .map(|registry| registry.path().join(format!("graviola-{version}")))
        .find(|dir| dir.is_dir())
        .unwrap_or_else(|| {
            panic!(
                "graviola {version}'s source is not under {}",
                registries.display()
            )
        });
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").expect("cargo's target arch");
    let cpu = source_dir.join(format!("src/low/{arch}/cpu.rs"));
    let source = std::fs::read_to_string(&cpu).expect("graviola's cpu.rs");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo's OUT_DIR"));
    std::fs::write(out.join("graviola_cpu.rs"), source).expect("a copy in OUT_DIR");
    println!("cargo::rustc-env=GRAVIOLA_CPU_RS={}", cpu.display());
}
