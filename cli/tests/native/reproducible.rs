//! Two cold builds of one program, in two repositories, write the same
//! executable on every native backend built in.
//!
//! The program catches scheduling: the stencil backend deals parts to workers
//! at random, and half the modules release `Doc` first while the rest release
//! `Entry` first, so any helper name that depends on a worker's history moves.

use std::path::{Path, PathBuf};
use std::process::Command;

/// One codegen unit each: several per worker on most machines.
const MODULES: usize = 24;

/// The two types that reach each other.
const DOC: &str = r#"export enum Doc {
    Null,
    Num(Int),
    Arr([Doc]),
    Obj([Entry]),
}

export struct Entry {
    export key: Str,
    export value: Doc,
}
"#;

/// Module `i`: even ones release a list of `Entry`, odd ones release a `Doc`.
/// Recursive, so that the inliner leaves each body in its own unit.
fn module(i: usize) -> String {
    let body = if i.is_multiple_of(2) {
        "let es = [Entry { key: \"k\", value: Doc.Num(n) }];\n        es.length()"
    } else {
        "let d = Doc.Arr([Doc.Null, Doc.Num(n)]);\n        match (d) { .Arr(items) => items.length(), _ => 0 }"
    };
    format!(
        "from \"//cmd/app/doc.buri\" import {{ Doc, Entry }};\n\n\
         export fn run{i}(n: Int): Int {{\n    \
             if (n <= 0) {{ 0 }} else {{\n        \
                 let here = {{\n        {body}\n        }};\n        \
                 here + run{i}(n - 1)\n    \
             }}\n\
         }}\n"
    )
}

fn main_module() -> String {
    let imports: String = (0..MODULES)
        .map(|i| format!("from \"//cmd/app/m{i}.buri\" import {{ run{i} }};\n"))
        .collect();
    let total: Vec<String> = (0..MODULES).map(|i| format!("run{i}(2)")).collect();
    format!(
        "from \"platform/effect\" import {{ Allocator, Stdout }};\n\
         from \"native\" import {{ NativeHost }};\n\
         from \"core/io\" import * as io;\n\
         {imports}\n\
         export fn main(host: NativeHost): Result<(), Str> {{\n    \
             let ctx = context {{ Allocator: host.alloc, Stdout: host.stdout }};\n    \
             let total = {};\n    \
             io.println(ctx, \"${{total}}\").mapErr(fn(_e) => \"no stdout\")\n\
         }}\n",
        total.join(" + ")
    )
}

/// The host's variant, as a build file spells it.
fn host_variant() -> String {
    let os = if cfg!(target_os = "macos") { "macos" } else { "linux" };
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" };
    format!("{os}-{arch}")
}

/// Lays the repository out under `dir`.
fn write_repo(dir: &Path) {
    let pkg = dir.join("cmd").join("app");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(dir.join("REPO.buri"), "").unwrap();
    let mut sources = vec![String::from("\"doc.buri\"")];
    sources.extend((0..MODULES).map(|i| format!("\"m{i}.buri\"")));
    std::fs::write(
        pkg.join("BUILD.buri"),
        format!(
            "binary {{\n    sources: [{}]\n    outputs: [{{ platform: \"native\", variant: \"{}\" }}]\n}}\n",
            sources.join(", "),
            host_variant()
        ),
    )
    .unwrap();
    std::fs::write(pkg.join("doc.buri"), DOC).unwrap();
    for i in 0..MODULES {
        std::fs::write(pkg.join(format!("m{i}.buri")), module(i)).unwrap();
    }
    std::fs::write(pkg.join("main.buri"), main_module()).unwrap();
}

/// Builds the program from cold in a repository of its own and answers the
/// artifact's bytes, having run it once to be sure it is the program.
fn built(round: &Path, mode: &[&str]) -> Vec<u8> {
    let _ = std::fs::remove_dir_all(round);
    write_repo(round);
    let out = Command::new(env!("CARGO_BIN_EXE_buri"))
        .current_dir(round)
        .arg("build")
        .args(mode)
        .output()
        .expect("run buri build");
    assert!(
        out.status.success(),
        "the build failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let artifact = round.join(".buri/out/native").join(host_variant()).join("cmd/app/app");
    let ran = Command::new(&artifact).output().expect("run the artifact");
    // `run(2)` is two calls: an even module counts one entry in each, an odd
    // one two items, so a pair of modules adds six.
    assert_eq!(String::from_utf8_lossy(&ran.stdout), format!("{}\n", 3 * MODULES));
    std::fs::read(&artifact).unwrap()
}

#[test]
fn two_cold_builds_agree_byte_for_byte_on_every_native_backend() {
    for (backend, mode) in crate::e2e::build_modes() {
        // Named for this process, so two test runs at once do not collide.
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("reproducible-{backend}-{}", std::process::id()));
        let a = built(&dir.join("a"), mode);
        let b = built(&dir.join("b"), mode);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            a == b,
            "two cold builds of one program on the {backend} backend wrote different \
             executables, first at byte {:?}",
            a.iter().zip(&b).position(|(x, y)| x != y).or(Some(a.len().min(b.len())))
        );
    }
}
