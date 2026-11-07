//! Copies the `cpu.rs` of the `graviola` this build compiles into `OUT_DIR`,
//! for `tls::tests::the_feature_check_matches_what_graviola_asserts` to
//! `include_str!`.
//!
//! Read here rather than by the test at run time because the test binary runs
//! where the cargo registry may not be: Memcheck builds its binaries in one job
//! and runs them in another. The build always has the source, since it
//! compiles it.
//!
//! Cargo says where that source is, because only cargo knows: the registry, a
//! vendored directory (`nix build`), or whatever a `[source]` replacement
//! names. It is asked through `cargo metadata` on a manifest that depends on
//! graviola alone, not on this workspace: the workspace's resolve needs every
//! crate it names, so on a cold `~/.cargo` it needs crates this build never
//! downloaded, and `--offline` then fails. `--filter-platform` keeps the
//! probe's resolve to the crates this target compiles.

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a build script fails by panicking, and cargo prints the message; \
              each one here names what was missing"
)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo's manifest dir"));
    let lock_path = manifest.join("../Cargo.lock");
    println!("cargo::rerun-if-changed={}", lock_path.display());
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
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo's OUT_DIR"));
    let source_dir = graviola_source(&out, version);
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").expect("cargo's target arch");
    let cpu = source_dir.join(format!("src/low/{arch}/cpu.rs"));
    let source = std::fs::read_to_string(&cpu).expect("graviola's cpu.rs");
    std::fs::write(out.join("graviola_cpu.rs"), source).expect("a copy in OUT_DIR");
    println!("cargo::rustc-env=GRAVIOLA_CPU_RS={}", cpu.display());
}

/// The directory cargo resolves graviola `version` to, asked from this crate's
/// directory so that cargo reads the same config, and so the same `[source]`
/// replacement, as the build that runs this script.
fn graviola_source(out: &Path, version: &str) -> PathBuf {
    let probe = out.join("graviola-probe");
    std::fs::create_dir_all(probe.join("src")).expect("the probe's directory");
    std::fs::write(probe.join("src/lib.rs"), "").expect("the probe's lib.rs");
    // `[workspace]`, because `OUT_DIR` is under this workspace's `target/`.
    std::fs::write(
        probe.join("Cargo.toml"),
        format!(
            "[package]\nname = \"graviola-probe\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n\
             [workspace]\n\n[dependencies]\ngraviola = {{ version = \"={version}\", \
             default-features = false }}\n"
        ),
    )
    .expect("the probe's Cargo.toml");
    let cargo = std::env::var_os("CARGO").expect("cargo's path");
    let target = std::env::var("TARGET").expect("cargo's target");
    let output = Command::new(cargo)
        .args(["metadata", "--offline", "--format-version", "1"])
        .args(["--filter-platform", &target])
        .arg("--manifest-path")
        .arg(probe.join("Cargo.toml"))
        .output()
        .expect("cargo metadata runs");
    assert!(
        output.status.success(),
        "cargo metadata could not resolve graviola {version}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = String::from_utf8(output.stdout).expect("cargo metadata's JSON is UTF-8");
    // Each package object lists its `manifest_path` after its name and version.
    let manifest_path = json
        .split(&format!("\"name\":\"graviola\",\"version\":\"{version}\""))
        .nth(1)
        .and_then(|rest| rest.split("\"manifest_path\":\"").nth(1))
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("cargo metadata names no graviola {version}"));
    Path::new(manifest_path)
        .parent()
        .expect("a manifest is in a directory")
        .to_path_buf()
}
