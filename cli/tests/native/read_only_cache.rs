//! A native build whose `.buri/cache` can't be written to, on every native
//! backend built in: the build still succeeds, and the program it writes is
//! the program the sources say.

use crate::hang::Capped;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::Command;

fn program(says: &str) -> String {
    format!(
        r#"from "platform/effect" import {{ Allocator, Stdout }};
from "native" import {{ NativeHost }};
from "core/io" import * as io;

export fn main(host: NativeHost): Result<(), Str> {{
    let ctx = context {{ Allocator: host.alloc, Stdout: host.stdout }};
    io.println(ctx, "{says}").mapErr(fn(_e) => "no stdout")
}}
"#
    )
}

fn host_variant() -> String {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86_64"
    };
    format!("{os}-{arch}")
}

/// Sets every directory and file under `dir` read-only, or writable again.
fn writable(dir: &Path, yes: bool) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            writable(&path, yes);
        }
        let mode = match (path.is_dir(), yes) {
            (true, true) => 0o755,
            (true, false) => 0o555,
            (false, true) => 0o644,
            (false, false) => 0o444,
        };
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    let mode = if yes { 0o755 } else { 0o555 };
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// A directory kept read-only until dropped, so a failing test still leaves a
/// tree the sweep can remove.
struct Locked(std::path::PathBuf);

impl Locked {
    fn new(dir: std::path::PathBuf) -> Locked {
        writable(&dir, false);
        Locked(dir)
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        writable(&self.0, true);
    }
}

/// Builds the repository at `repo` and answers what its program prints.
fn build_and_run(repo: &Path, mode: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_buri"))
        .current_dir(repo)
        .arg("build")
        .args(mode)
        .capped_output()
        .expect("run buri build");
    assert!(
        out.status.success(),
        "the build failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let artifact = repo
        .join(".buri/out/native")
        .join(host_variant())
        .join("cmd/app/app");
    let ran = crate::shared::run_artifact(&artifact);
    String::from_utf8_lossy(&ran.stdout).into_owned()
}

/// A changed program, built once its cache can no longer take new objects,
/// links the objects this build emitted and prints what the new source says.
#[test]
fn a_cache_that_cannot_be_written_still_builds_the_changed_program() {
    for (backend, mode) in crate::e2e::build_modes() {
        let repo = crate::sweep::owned("native-read-only-cache", backend);
        let pkg = repo.join("cmd/app");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(repo.join("REPO.buri"), "").unwrap();
        std::fs::write(
            pkg.join("BUILD.buri"),
            format!(
                "binary {{\n    outputs: [{{ platform: \"native\", variant: \"{}\" }}]\n}}\n",
                host_variant()
            ),
        )
        .unwrap();
        std::fs::write(pkg.join("main.buri"), program("before")).unwrap();
        assert_eq!(build_and_run(&repo, mode), "before\n", "{backend}");

        let locked = Locked::new(repo.join(".buri/cache"));
        std::fs::write(pkg.join("main.buri"), program("after")).unwrap();
        assert_eq!(build_and_run(&repo, mode), "after\n", "{backend}");
        drop(locked);
    }
}
