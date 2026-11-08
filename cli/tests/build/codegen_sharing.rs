//! Codegen shared across programs: a unit two programs compile alike is
//! compiled by the first and restored by the second, closures included.
use crate::harness::*;

/// This machine's `variant` for a native output.
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

/// A library of closures, each capturing its parent's parameter. Each program
/// calls `scaled` twice, so `inline` leaves it in the library's unit.
const SHARED: &str = "export fn apply(f: fn(Int) => Int, x: Int): Int {\n    f(x)\n}\n\n\
     export fn scaled(k: Int, x: Int): Int {\n    apply(fn(y) => y * k, x)\n}\n\n\
     export fn shifted(k: Int, x: Int): Int {\n    apply(fn(y) => y + k, x)\n}\n";

/// A native program over `//lib/shared` whose `main` prints `body`, an `Int`,
/// after defining `items`.
fn program(items: &str, body: &str) -> String {
    format!(
        "from \"platform/effect\" import {{ Allocator, Stdout }};\n\
         from \"native\" import {{ NativeHost }};\n\
         from \"core/io\" import * as io;\n\
         from \"//lib/shared\" import {{ apply, scaled, shifted }};\n\n{items}\n\
         export fn main(host: NativeHost): Result<(), Str> {{\n    \
         let ctx = context {{ Allocator: host.alloc, Stdout: host.stdout }};\n    \
         let _ = io.println(ctx, \"${{{body}}}\").ignore();\n    \
         .Ok(())\n}}\n"
    )
}

/// `//cmd/a` with one closure of its own, `//cmd/b` with three, and both over
/// `//lib/shared`.
fn repo(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    let binary = format!(
        "binary {{\n  outputs: [{{ platform: \"native\", variant: \"{}\" }}]\n}}\n",
        host_variant()
    );
    scratch.write(
        "lib/shared/BUILD.buri",
        "library {\n  visibility: [\"//visibility:public\"]\n}\n",
    );
    scratch.write("lib/shared/lib.buri", SHARED);
    scratch.write("cmd/a/BUILD.buri", &binary);
    scratch.write(
        "cmd/a/main.buri",
        &program(
            "",
            "apply(fn(y) => y - 1, scaled(3, scaled(1, shifted(2, 5))))",
        ),
    );
    scratch.write("cmd/b/BUILD.buri", &binary);
    scratch.write("cmd/b/main.buri", &b_main(10));
    scratch
}

/// `//cmd/b`'s source, its first closure adding `n`.
fn b_main(n: i64) -> String {
    program(
        "fn twice(k: Int, x: Int): Int {\n    apply(fn(y) => y * 2 + k, x)\n}\n",
        &format!("apply(fn(y) => y + {n}, twice(1, apply(fn(y) => y * y, scaled(1, scaled(1, shifted(1, 2))))))"),
    )
}

/// A profiled build of `label`, `release` or not: its objects compiled and
/// restored.
fn build(scratch: &Scratch, label: &str, release: bool) -> (u64, u64) {
    let mut args = vec!["build"];
    if release {
        args.push("--release");
    }
    args.push(label);
    let run = scratch.run_with_env(&args, &[("BURI_PROFILE", "1")]);
    run.ok();
    let all = run.all();
    let count = |name: &str| -> u64 {
        all.lines()
            .find_map(|l| l.strip_prefix(&format!("{name} ")))
            .unwrap_or_else(|| panic!("no `{name}` count in\n{}", indent(&all)))
            .parse()
            .unwrap()
    };
    (count("objects compiled"), count("objects restored"))
}

/// What the program at `package` prints, as its last build left it.
fn printed(scratch: &Scratch, package: &str) -> String {
    let name = package.rsplit('/').next().unwrap();
    let binary = scratch.path(&format!(
        ".buri/out/native/{}/{package}/{name}",
        host_variant()
    ));
    let ran = std::process::Command::new(&binary)
        .output()
        .unwrap_or_else(|e| panic!("{}: {e}", binary.display()));
    String::from_utf8_lossy(&ran.stdout).into_owned()
}

/// Whether this toolchain can build `--release` natively.
fn has_release(scratch: &Scratch) -> bool {
    let run = scratch.run(&["build", "--release", "//cmd/a"]);
    if run.stderr.contains("built without `backend-llvm`") {
        run.exits(1);
        return false;
    }
    run.ok();
    true
}

/// **A second program restores the units it shares with the first.** Both
/// reach `//lib/shared`, its closures and the same standard library, and
/// differ in `main` alone, so `//cmd/b` compiles one object and restores the
/// rest, however many closures each program holds. Release too, when the
/// toolchain has the optimizing backend.
#[test]
fn a_second_program_restores_the_units_it_shares_with_the_first() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::codegen_sharing", &why);
        return;
    }
    let scratch = repo("codegen-sharing");
    let (a_compiled, a_restored) = build(&scratch, "//cmd/a", false);
    assert!(a_compiled > 1, "//cmd/a compiled {a_compiled} objects");
    assert_eq!(a_restored, 0);
    let (b_compiled, b_restored) = build(&scratch, "//cmd/b", false);
    assert_eq!(
        (b_compiled, b_restored),
        (1, a_compiled - 1),
        "//cmd/b compiled its own main and restored the rest"
    );
    assert_eq!(printed(&scratch, "cmd/a"), "20\n");
    assert_eq!(printed(&scratch, "cmd/b"), "29\n");

    if !has_release(&scratch) {
        return;
    }
    let (b_compiled, b_restored) = build(&scratch, "//cmd/b", true);
    // `//cmd/b`'s main, and the glue every unit's asks decide.
    assert_eq!(b_compiled, 2, "{b_restored} restored");
    assert!(b_restored > 0);
    assert_eq!(printed(&scratch, "cmd/a"), "20\n");
    assert_eq!(printed(&scratch, "cmd/b"), "29\n");
}

/// **A closure edited in one unit recompiles that unit alone.** Adding a
/// closure to `//lib/shared` renames no closure in any other unit, so
/// `//cmd/b` compiles the library's unit and restores its `main`, and
/// `//cmd/a` then restores everything, the edited library included. Adding one
/// to `//cmd/b`'s `main` recompiles that `main` alone.
#[test]
fn editing_a_closure_recompiles_only_its_unit() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::codegen_sharing", &why);
        return;
    }
    let scratch = repo("codegen-sharing-edit");
    build(&scratch, "//cmd/a", false);
    build(&scratch, "//cmd/b", false);

    scratch.edit(
        "lib/shared/lib.buri",
        "apply(fn(y) => y * k, x)",
        "apply(fn(y) => y * k, apply(fn(z) => z, x))",
    );
    let (compiled, restored) = build(&scratch, "//cmd/b", false);
    assert_eq!(compiled, 1, "{restored} restored");
    assert_eq!(printed(&scratch, "cmd/b"), "29\n");
    let (compiled, _) = build(&scratch, "//cmd/a", false);
    assert_eq!(compiled, 0);
    assert_eq!(printed(&scratch, "cmd/a"), "20\n");

    scratch.write(
        "cmd/b/main.buri",
        &program(
            "fn twice(k: Int, x: Int): Int {\n    apply(fn(y) => y * 2 + k, x)\n}\n",
            "apply(fn(y) => y + 10, apply(fn(y) => y * 3, twice(1, apply(fn(y) => y * y, \
             scaled(1, scaled(1, shifted(1, 2)))))))",
        ),
    );
    let (compiled, restored) = build(&scratch, "//cmd/b", false);
    assert_eq!(compiled, 1, "{restored} restored");
    assert_eq!(printed(&scratch, "cmd/b"), "67\n");
}

/// **Closures written alike in different parents stay apart.** Two modules
/// each define `adder` around the same closure, one generic function holds a
/// closure for two types, and a closure sits inside another: the program links,
/// and each parent runs its own code, natively and in release.
#[test]
fn closures_with_the_same_body_in_different_parents_stay_apart() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::codegen_sharing", &why);
        return;
    }
    let scratch = Scratch::repo("codegen-sharing-alike");
    scratch.write(
        "cmd/c/BUILD.buri",
        &format!(
            "binary {{\n  outputs: [{{ platform: \"native\", variant: \"{}\" }}]\n}}\n",
            host_variant()
        ),
    );
    let adder =
        "export fn adder(k: Int, x: Int): Int {\n    let f = fn(y) => y + k;\n    f(x)\n}\n";
    scratch.write("cmd/c/one.buri", adder);
    scratch.write("cmd/c/two.buri", &adder.replace("f(x)", "f(x * 100)"));
    scratch.write(
        "cmd/c/main.buri",
        "from \"platform/effect\" import { Allocator, Stdout };\n\
         from \"native\" import { NativeHost };\n\
         from \"core/io\" import * as io;\n\
         from \"//cmd/c/one.buri\" import * as one;\n\
         from \"//cmd/c/two.buri\" import * as two;\n\n\
         fn pick<T: Equal>(a: T, b: T, first: Bool): T {\n    \
         let f = fn(p) => if (p) { a } else { b };\n    f(first)\n}\n\n\
         fn nested(k: Int): Int {\n    \
         let outer = fn(x) => {\n        let inner = fn(y) => y * k;\n        inner(x) + 1\n    };\n    \
         outer(3)\n}\n\n\
         export fn main(host: NativeHost): Result<(), Str> {\n    \
         let ctx = context { Allocator: host.alloc, Stdout: host.stdout };\n    \
         let word = pick(\"x\", \"y\", true);\n    \
         let _ = io.println(ctx, \"${one.adder(1, 2)} ${two.adder(1, 2)} ${pick(4, 5, false)} \
         ${word} ${nested(7)}\").ignore();\n    \
         .Ok(())\n}\n",
    );
    let want = "3 201 5 x 22\n";
    scratch.run(&["build", "//cmd/c"]).ok();
    let ran = std::process::Command::new(
        scratch.path(&format!(".buri/out/native/{}/cmd/c/c", host_variant())),
    )
    .output()
    .unwrap();
    assert_eq!(String::from_utf8_lossy(&ran.stdout), want);

    let release = scratch.run(&["build", "--release", "//cmd/c"]);
    if release.stderr.contains("built without `backend-llvm`") {
        release.exits(1);
        return;
    }
    release.ok();
    let ran = std::process::Command::new(
        scratch.path(&format!(".buri/out/native/{}/cmd/c/c", host_variant())),
    )
    .output()
    .unwrap();
    assert_eq!(String::from_utf8_lossy(&ran.stdout), want);
}

/// The `codegen` lines of an `--explain` build's output.
fn codegen_lines(out: &str) -> Vec<&str> {
    out.lines().filter(|l| l.contains(" codegen //")).collect()
}

/// **One build says the same of a shared unit, whichever program got to it
/// first.** Built together, `//cmd/a` and `//cmd/b` emit each unit they share
/// once, and both report it `run`, because this build ran it. The profile
/// counts each emitted unit once, and the next build restores every unit.
#[test]
fn programs_built_together_report_a_shared_unit_alike() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::codegen_sharing", &why);
        return;
    }
    let scratch = repo("codegen-sharing-together");
    let cold = scratch.run_with_env(&["build", "--explain", "//..."], &[("BURI_PROFILE", "1")]);
    cold.ok();
    let all = cold.all();
    let codegen = codegen_lines(&all);
    assert!(
        codegen.iter().all(|l| l.starts_with("run ")),
        "{}",
        indent(&all)
    );
    let mut keys: Vec<&str> = codegen
        .iter()
        .filter_map(|l| l.rsplit(' ').next())
        .collect();
    keys.sort_unstable();
    keys.dedup();
    assert!(
        keys.len() < codegen.len(),
        "nothing shared:\n{}",
        indent(&all)
    );
    let compiled = all
        .lines()
        .find_map(|l| l.strip_prefix("objects compiled "))
        .unwrap_or_else(|| panic!("no count in\n{}", indent(&all)));
    assert_eq!(compiled, keys.len().to_string(), "{}", indent(&all));

    let warm = scratch.run(&["build", "--explain", "//..."]);
    warm.ok();
    let again = codegen_lines(&warm.stdout);
    assert_eq!(again.len(), codegen.len(), "{}", indent(&warm.stdout));
    assert!(
        again.iter().all(|l| l.starts_with("cached ")),
        "{}",
        indent(&warm.stdout)
    );
}
