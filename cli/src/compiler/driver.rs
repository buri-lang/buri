//! Putting the front end together: load a unit, check it, report.

use crate::build::actions;
use crate::build::buildfile::Platform;
use crate::build::workspace::Workspace;
use crate::compiler::modules::{Loaded, Loader, Unit};
use crate::compiler::semantics::resolve::{Checked, Checker};
use crate::diagnostics::{Diagnostic, Diagnostics, SourceMap, Span};

pub struct Analysis {
    pub loaded: Loaded,
    pub checked: Checked,
    pub diags: Diagnostics,
}

/// Loads and checks one unit. The two halves are separate so that `lint` and
/// `query` can stop after loading.
pub fn analyze(ws: Option<&Workspace>, map: &mut SourceMap, unit: &Unit) -> Analysis {
    let mut diags = Diagnostics::new();
    let loaded = {
        let mut loader = Loader::new(ws, map, &mut diags);
        loader.load_unit(unit);
        loader.finish()
    };
    let checked = Checker::new(&loaded, ws, &mut diags).run();
    diags.sort(map);
    Analysis { loaded, checked, diags }
}

/// Loads and checks every module of the standard library, with no repository.
/// This is what `buri version --self-check` runs, and what the toolchain's own
/// tests use.
pub fn analyze_stdlib(map: &mut SourceMap) -> Analysis {
    let mut diags = Diagnostics::new();
    let loaded = {
        let mut loader = Loader::new(None, map, &mut diags);
        loader.load_all_std();
        loader.finish()
    };
    let checked = Checker::new(&loaded, None, &mut diags).run();
    diags.sort(map);
    Analysis { loaded, checked, diags }
}

/// Loads and checks one standard library module the way a *program* would
/// reach it: on top of the built-in types and whatever it imports for itself,
/// and nothing else.
///
/// `analyze_stdlib` loads every module together, so it cannot notice one that
/// only checks because something else happened to be present. Since the
/// standard library loads lazily, that is exactly the mistake worth catching:
/// a module is first seen in a compilation holding the eager set and its own
/// imports.
pub fn analyze_std_module(map: &mut SourceMap, path: &str) -> Analysis {
    let mut diags = Diagnostics::new();
    let loaded = {
        let mut loader = Loader::new(None, map, &mut diags);
        loader.load_builtin_modules();
        loader.load_std_module(path);
        loader.finish()
    };
    let checked = Checker::new(&loaded, None, &mut diags).run();
    diags.sort(map);
    Analysis { loaded, checked, diags }
}

/// Loads and checks one module given as text, on top of the whole standard
/// library and with no repository.
///
/// This is the documentation harness's entry point. It is deliberately the
/// same `Loader` and the same `Checker` the compiler runs, because a doctest
/// that passed against a simplified pipeline would prove nothing about the
/// example a reader is about to copy.
pub fn analyze_snippet(
    map: &mut SourceMap,
    name: &str,
    text: &str,
    role: crate::compiler::modules::Role,
) -> Analysis {
    analyze_snippet_in(None, map, name, text, role)
}

/// The same, against a repository, so a snippet that imports `//lib/money`
/// resolves it.
///
/// The build system's documentation is mostly *about* a monorepo, so most of
/// its examples name `//...` paths. Compiling them against the worked example
/// repository is what makes those examples testable instead of illustrative —
/// and it is the same path a third-party repository's own documentation takes.
pub fn analyze_snippet_in(
    ws: Option<&Workspace>,
    map: &mut SourceMap,
    name: &str,
    text: &str,
    role: crate::compiler::modules::Role,
) -> Analysis {
    analyze_snippet_as(ws, None, map, name, text, role)
}

/// The same, with the snippet standing in for a file of `pkg` — which is what
/// makes a document about a library's internals compilable.
pub fn analyze_snippet_as(
    ws: Option<&Workspace>,
    pkg: Option<crate::build::workspace::PkgId>,
    map: &mut SourceMap,
    name: &str,
    text: &str,
    role: crate::compiler::modules::Role,
) -> Analysis {
    let mut diags = Diagnostics::new();
    let loaded = {
        let mut loader = Loader::new(ws, map, &mut diags);
        loader.load_all_std();
        loader.load_source_in(name, role, text.to_string(), pkg);
        loader.finish()
    };
    let checked = Checker::new(&loaded, ws, &mut diags).run();
    diags.sort(map);
    Analysis { loaded, checked, diags }
}

/// Compiles a snippet that exports `main`, runs it, and returns its standard
/// output.
///
/// The tail of `actions::build_target` minus the workspace, the cache, and the
/// artifact directory — so a documented program is executed exactly the way
/// `buri run` would execute it.
pub fn run_snippet(map: &mut SourceMap, name: &str, text: &str) -> Result<String, Diagnostics> {
    run_snippet_in(None, map, name, text)
}

/// The same, against a repository, so a documented program may import the
/// packages the document is about.
pub fn run_snippet_in(
    ws: Option<&Workspace>,
    map: &mut SourceMap,
    name: &str,
    text: &str,
) -> Result<String, Diagnostics> {
    let analysis = analyze_snippet_in(ws, map, name, text, crate::compiler::modules::Role::Entry);
    if analysis.diags.has_errors() {
        return Err(analysis.diags);
    }
    let mut diags = Diagnostics::new();
    let Some(entry) = analysis.checked.entry else {
        diags.push(
            Diagnostic::error(Span::NONE, "this example exports no `main`")
                .with_fix(
                    "give the fence `wrap=body`, which supplies one, or write \
                     `export fn main(): Result<(), Str> { ... }`",
                ),
        );
        return Err(diags);
    };
    let module_paths: Vec<String> =
        analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut program = crate::compiler::transform::monomorphize::run(
        &analysis.checked,
        module_paths,
        &mut diags,
        crate::compiler::transform::monomorphize::Roots::Main(entry),
    );
    if diags.has_errors() {
        return Err(diags);
    }
    let flags = crate::commands::arguments::Flags::default();
    let source = actions::emit(&mut program, &analysis.checked.tables, &flags, &mut diags)?;
    execute(name, &source)
}

/// Writes the emitted module to a scratch file and runs it under the JS
/// runtime, because an ES module has to come from a file to be imported.
fn execute(name: &str, source: &str) -> Result<String, Diagnostics> {
    use std::process::Command;
    let fail = |msg: String, fix: &str| {
        let mut d = Diagnostics::new();
        d.push(Diagnostic::error(Span::NONE, msg).with_fix(fix.to_string()));
        d
    };
    let dir = std::env::temp_dir().join("buri-doctest");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return Err(fail(format!("cannot create {}: {e}", dir.display()), "check TMPDIR"));
    }
    // The file name has to be unique across concurrently running tests, and
    // derived from the snippet so a rerun overwrites rather than accumulates.
    let stem: String =
        name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let path = dir.join(format!("{stem}.mjs"));
    if let Err(e) = std::fs::write(&path, source) {
        return Err(fail(format!("cannot write {}: {e}", path.display()), "check TMPDIR"));
    }
    let out = match Command::new(crate::commands::test::js_runtime()).arg(&path).output() {
        Ok(o) => o,
        Err(e) => {
            return Err(fail(
                format!("cannot run {}: {e}", crate::commands::test::js_runtime()),
                "install bun, or set BURI_JS to a JavaScript runtime",
            ))
        }
    };
    let _ = std::fs::remove_file(&path);
    if !out.status.success() {
        return Err(fail(
            format!(
                "the example exited {}: {}",
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            "fix the example, or change the expected output beneath it",
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// The platform a build defaults to when nothing selects one.
pub fn host_platform() -> Platform {
    // Only the JavaScript backend exists, so it is what a build produces.
    Platform::Js
}

/// The platform a *test suite* is checked against when it names none. A suite
/// runs once, on the host platform (TAGS.md, "Tags and tests"), and the host
/// is this machine — code tagged for the machine it was written for is not
/// being asked to run in a browser just because the backend emits JavaScript.
pub fn host_native_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Linux
    }
}
