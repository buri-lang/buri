//! **One function, one symbol.**
//!
//! `monomorphize::name_of` mangles every function in a program to a symbol, and
//! the whole of what a symbol is for is that it names exactly one body. Two
//! functions wearing one symbol is a miscompile on every backend: the
//! JavaScript emitter writes two `function` declarations and the second
//! shadows the first, and a native link binds every cross-unit call to
//! whichever definition the linker kept.
//!
//! It is not a theoretical hazard. A context constructor was mangled as
//! `ctx$<name>` with nothing in front of it, so two modules that each declared
//! `context Fixture { ... }` — a fixture in one library's `testing` surface and
//! a fixture in another's, or one test source per package in a batched test
//! binary — produced two constructors called `ctx$Fixture`. What that looked
//! like from the outside was not a wrong answer: the inliner empties the
//! smaller of the two constructors into its callers, `dce` marks the emptied
//! one unbuilt, the backend emits an *abort* under the shared symbol, and the
//! other module's every test dies on "this function was never built".
//! `repositories/testing/same_named_contexts` is that repository, driven
//! through the CLI; this file is the invariant underneath it.
//!
//! The invariant is asserted **before** `middle::run`, on the program
//! monomorphization produced, so it does not depend on what the inliner
//! happened to do with either body. A collision that today only shows up
//! because one of the two was inlined away would still be a collision on a
//! toolchain that inlined neither.
//!
//! Both ways a program comes to hold two modules are covered: one target at a
//! time, and every target that has a suite compiled together, which is what
//! `commands/test.rs`'s `run_batch` builds for a `buri test` naming more than
//! one package.
//!
//! ```text
//! cargo test -p buri --test language symbols::
//! ```
use buri::build::session;
use buri::commands::arguments::Flags;
use buri::compiler::driver;
use buri::compiler::middle::monomorphize::{self, FuncKind, Program};
use buri::compiler::modules::Unit;
use buri::diagnostics::Diagnostics;

use std::path::{Path, PathBuf};

fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

/// The worked monorepo and every repository the test-runner corpus is written
/// against — the small ones are where a shape like two same-named contexts is
/// written down on purpose, and the large one is where an accident would be.
fn repositories() -> Vec<PathBuf> {
    let mut out = vec![tests_dir().join("example")];
    let testing = tests_dir().join("repositories/testing");
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&testing)
        .expect("the test-runner corpus")
        .filter_map(Result::ok)
        .map(|e| e.path().join("repo"))
        .filter(|p| p.join("REPO.buri").is_file())
        .collect();
    cases.sort();
    out.extend(cases);
    out
}

#[test]
fn no_two_functions_in_one_program_share_a_symbol() {
    let mut programs = 0usize;
    for root in repositories() {
        let Ok(mut open) = session::open_at(&root, &Flags::default()) else { continue };
        // A repository whose own build files do not read compiles nothing.
        if open.diagnostics.has_errors() {
            continue;
        }
        let every: Vec<Unit> = open
            .workspace
            .targets()
            .into_iter()
            .map(|target| Unit { target: Some(target), platform: None, with_tests: true })
            .collect();

        // One target at a time, and then all of them at once — the batched
        // test binary, which is where two packages' declarations first meet.
        let mut sets: Vec<(String, Vec<Unit>)> =
            every.iter().map(|u| (root.display().to_string(), vec![u.clone()])).collect();
        sets.push((format!("{} (batched)", root.display()), every));

        for (label, units) in sets {
            let Some(program) = program_of(&mut open, &units) else { continue };
            unique_symbols(&label, &program);
            programs = programs.saturating_add(1);
        }
    }
    assert!(programs > 20, "only {programs} programs were monomorphized");
}

/// The two `Fixture` declarations of
/// `repositories/testing/same_named_contexts` get two symbols, and each is
/// qualified by the module that declares it.
///
/// [`no_two_functions_in_one_program_share_a_symbol`] would catch the
/// collision; this says what the answer *is*, so that a repair which made the
/// two symbols unique by some means that does not survive a rename — an
/// instantiation counter, say — is a failure here rather than a green suite.
///
/// Four, not five: `//lib/app`'s suite declares no `context` of its own and
/// reaches two of the four through the `testing` surfaces it depends on, which
/// is why every target of the repository is compiled together here.
#[test]
fn a_context_constructor_is_named_by_the_module_that_declares_it() {
    let root = tests_dir().join("repositories/testing/same_named_contexts/repo");
    let mut open = session::open_at(&root, &Flags::default()).expect("the case repository");
    assert!(!open.diagnostics.has_errors(), "the case repository does not open");
    let units: Vec<Unit> = open
        .workspace
        .targets()
        .into_iter()
        .map(|target| Unit { target: Some(target), platform: None, with_tests: true })
        .collect();
    let program = program_of(&mut open, &units).expect("the case repository does not compile");

    let mut ctors: Vec<String> = program
        .funcs
        .iter()
        .filter(|f| f.symbol.contains("$ctx$"))
        .map(|f| f.symbol.clone())
        .collect();
    ctors.sort();
    ctors.dedup();

    let expected = [
        "__lib_one_testing_fixture_buri$ctx$Fixture",
        "__lib_reader_test_reader_buri$ctx$Fixture",
        "__lib_two_testing_fixture_buri$ctx$Fixture",
        "__lib_writer_test_writer_buri$ctx$Fixture",
    ];
    let missing: Vec<&str> =
        expected.iter().copied().filter(|w| !ctors.iter().any(|s| s == w)).collect();
    assert!(missing.is_empty(), "the constructors {missing:?} are not in {ctors:?}");
}

/// The monomorphized program for one set of units, or `None` where the
/// analysis or the monomorphizer reported an error — a repository that does
/// not compile has nothing to say about symbols.
fn program_of(open: &mut session::Session, units: &[Unit]) -> Option<Program> {
    let analysis = driver::analyze_all(
        Some(&open.workspace),
        &mut open.map,
        &mut open.parsed,
        units,
    );
    if analysis.diagnostics.has_errors() {
        return None;
    }
    let module_paths: Vec<String> =
        analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diagnostics = Diagnostics::new();
    let program = monomorphize::run(
        &analysis.checked,
        module_paths,
        &mut diagnostics,
        monomorphize::Roots::Tests,
    );
    if diagnostics.has_errors() {
        return None;
    }
    Some(program)
}

/// Every function that will be *defined* has a symbol of its own.
///
/// An intrinsic is excluded and only an intrinsic is: it defines nothing —
/// "the backend declares the runtime import and defines nothing"
/// (`backend/llvm/emit.rs`) — so two of them under one key are two names for
/// one runtime entry rather than two bodies fighting over a symbol. `Str`'s
/// `compare` is reached both inherently and through `Ord` and is a live
/// example.
fn unique_symbols(label: &str, program: &Program) {
    let mut seen: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for func in &program.funcs {
        if matches!(func.kind, FuncKind::Intrinsic(_)) {
            continue;
        }
        if let Some(first) = seen.insert(&func.symbol, &func.debug_name) {
            panic!(
                "{label}: `{}` names two functions — `{first}` and `{}`",
                func.symbol, func.debug_name
            );
        }
    }
}
