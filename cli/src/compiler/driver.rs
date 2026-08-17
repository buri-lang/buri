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
pub fn analyze(
    ws: Option<&Workspace>,
    map: &mut SourceMap,
    cache: &mut crate::parsing::parser::Cache,
    unit: &Unit,
) -> Analysis {
    let mut diags = Diagnostics::new();
    let loaded = {
        let mut loader = Loader::new(ws, map, &mut diags, cache);
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
    let mut cache = crate::parsing::parser::Cache::new();
    let loaded = {
        let mut loader = Loader::new(None, map, &mut diags, &mut cache);
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
    let mut cache = crate::parsing::parser::Cache::new();
    let loaded = {
        let mut loader = Loader::new(None, map, &mut diags, &mut cache);
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
    let mut cache = crate::parsing::parser::Cache::new();
    analyze_snippet_in(None, map, &mut cache, name, text, role)
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
    cache: &mut crate::parsing::parser::Cache,
    name: &str,
    text: &str,
    role: crate::compiler::modules::Role,
) -> Analysis {
    analyze_snippet_as(ws, None, map, cache, name, text, role)
}

/// The same, with the snippet standing in for a file of `pkg` — which is what
/// makes a document about a library's internals compilable.
pub fn analyze_snippet_as(
    ws: Option<&Workspace>,
    pkg: Option<crate::build::workspace::PkgId>,
    map: &mut SourceMap,
    cache: &mut crate::parsing::parser::Cache,
    name: &str,
    text: &str,
    role: crate::compiler::modules::Role,
) -> Analysis {
    let mut diags = Diagnostics::new();
    let loaded = {
        let mut loader = Loader::new(ws, map, &mut diags, cache);
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
    let mut cache = crate::parsing::parser::Cache::new();
    let analysis =
        analyze_snippet_in(ws, map, &mut cache, name, text, crate::compiler::modules::Role::Entry);
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
    let mut program = crate::compiler::middle::monomorphize::run(
        &analysis.checked,
        module_paths,
        &mut diags,
        crate::compiler::middle::monomorphize::Roots::Main(entry),
    );
    if diags.has_errors() {
        return Err(diags);
    }
    let flags = crate::commands::arguments::Flags::default();
    let source = actions::emit(
        &mut program,
        &analysis.checked.tables,
        crate::compiler::backend::Target { platform: crate::build::buildfile::Platform::Js, arch: None },
        &flags,
        &mut diags,
    )?;
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

/// The platform this toolchain compiles *for* when nothing selects one: the
/// machine it is running on, where it can produce something for that machine,
/// and JavaScript where it cannot.
///
/// This is the switch `design/native/ARCHITECTURE.md` §4 calls "one line and a
/// large amount of churn", and what it turned out to be is one line and a
/// condition. `Platform::Js` unconditionally was right while there was no other
/// backend; it is wrong now, because a toolchain that can emit, link and run a
/// macOS executable should not be describing itself as a JavaScript compiler to
/// the language server and the documentation harness.
///
/// The condition is [`actions::native_ready`] rather than `cfg!(target_os)`,
/// because "this is a mac" and "this toolchain can build for a mac" are
/// different claims: a build with `--no-default-features` has no native backend,
/// a host outside macOS and Linux has no runtime archive, and either way the
/// honest answer is the one that has always been given. That is what keeps a
/// toolchain without a backend byte-identical to the one before this wave.
pub fn host_platform() -> Platform {
    let native = host_native_platform();
    if actions::native_ready(
        crate::compiler::backend::Target { platform: native, arch: None },
        crate::compiler::backend::Profile::Debug,
    ) {
        native
    } else {
        Platform::Js
    }
}

/// The platform a *test suite* is checked against when it names none. A suite
/// runs once, on the host platform (TAGS.md, "Tags and tests"), and the host
/// is this machine — code tagged for the machine it was written for is not
/// being asked to run in a browser just because the backend emits JavaScript.
///
/// Unconditional, unlike [`host_platform`], and the difference is the point:
/// this one answers "which machine is this", which is a fact about the machine,
/// and the other answers "what can this toolchain produce for it", which is a
/// fact about the toolchain. A suite tagged `macos` is macOS code on a macOS
/// host whether or not a native backend is compiled in.
pub fn host_native_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Linux
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole of what the switch promises, in both directions: a toolchain
    /// that cannot build for its own host answers exactly what it answered
    /// before this wave, and one that can answers the host.
    ///
    /// Written as an equivalence rather than as a constant, because the answer
    /// depends on how this toolchain was built — `--no-default-features` has no
    /// backend, a host outside macOS and Linux has no runtime archive — and a
    /// test asserting `Js` would pass for the wrong reason on the machine where
    /// it matters most.
    #[test]
    fn the_host_platform_is_js_exactly_when_this_toolchain_cannot_build_for_the_host() {
        let native = host_native_platform();
        let ready = actions::native_ready(
            crate::compiler::backend::Target { platform: native, arch: None },
            crate::compiler::backend::Profile::Debug,
        );
        assert_eq!(host_platform(), if ready { native } else { Platform::Js });
        // And the machine's own platform is a fact about the machine, which no
        // feature flag moves.
        assert!(matches!(native, Platform::Macos | Platform::Linux));
    }
}
