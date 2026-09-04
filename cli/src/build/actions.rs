//! `buri build` and `buri run`.
//!
//! Artifacts land in `.buri/out/<platform>/<package>/<artifact>`, where
//! `<artifact>` is the package's directory name unless the output overrides it
//! with `artifact_name`. Tags are not in the path, because they are not in the
//! cache key: a tag decides whether a build is permitted, never what it
//! produces.

use crate::build::buildfile::{Output, Platform};
use crate::build::cache::{hash_bytes, Action, ActionKey, Cache, KeyBuilder};
use crate::build::link;
use crate::build::session::Session;
use crate::build::workspace::{RuleKind, TargetId};
use crate::commands::arguments::Flags;
use crate::compiler::backend::runtime_native;
use crate::compiler::backend::{
    self, Emitted, LinkOptions, Linker, Options as BackendOptions, Profile, Target, Units,
};
use crate::compiler::middle;
use crate::compiler::middle::monomorphize;
use crate::compiler::middle::{ir, layout, lower};
use crate::compiler::modules::Unit;
use crate::compiler::semantics::types::Tables;
use crate::diagnostics::{Diagnostic, Diagnostics, Span};
use std::path::{Path, PathBuf};

pub struct Artifact {
    pub target: TargetId,
    pub path: PathBuf,
    pub bytes: usize,
    pub cached: bool,
}

/// Builds one target for one output, returning the artifact's path.
pub fn build_target(
    session: &mut Session,
    target: TargetId,
    output: &Output,
    flags: &Flags,
) -> Result<Artifact, Diagnostics> {
    let mut diagnostics = Diagnostics::new();
    let platform = output.platform();

    // Every check the graph can answer before a line is compiled.
    check_policy(session, target, platform, &mut diagnostics);
    if diagnostics.has_errors() {
        return Err(diagnostics);
    }

    // "Is there a linker and an object file at the end of this?" — not "is
    // this the JavaScript platform". A WEB output is JavaScript, so it takes
    // the branch below with `Js` rather than this one.
    if platform.is_native() {
        // The native path is reachable exactly when there is something to
        // reach: a backend compiled in for this target and profile, a runtime
        // archive for this host, and a host that can link the platform asked
        // for. Until all three hold, [`native_gap`] is the refusal — and it
        // names *which* of the three, because the one sentence the three used
        // to share was false on two of them.
        match native_gap(target_of(output), profile_of(flags)) {
            None => return build_native(session, target, output, flags, diagnostics),
            Some(gap) => {
                diagnostics.push(no_native_artifact(&gap, output.span));
                return Err(diagnostics);
            }
        }
    }

    // The key covers everything that can affect the artifact, so a hit means
    // the compiler has nothing to do.
    let key = action_key(session, target, output, flags, Action::Link);
    let path = artifact_path(session, target, output);
    let cache = Cache::open(&session.root);
    explain_closure(session, target, output, flags);
    let explain_link = |status: crate::build::cache::Status| {
        crate::build::cache::explain(
            flags.explain,
            status,
            Action::Link,
            &session.workspace.label(target),
            platform,
            &key,
        )
    };
    // A WEB output writes two more files beside its module, and both are in the
    // cache under keys derived from this one — so a hit either reproduces the
    // whole page or is not a hit. Reconstructing them from the module's bytes
    // instead would mean parsing generated JavaScript back into a string, and
    // a stale `.css` beside a fresh `.mjs` is exactly the failure a cache is
    // supposed to be incapable of.
    let sheet_key = key.companion("stylesheet");
    if !flags.force {
        if let Some(bytes) = cache.get(&key) {
            let stylesheet = if platform == Platform::Web {
                cache.get(&sheet_key).and_then(|b| String::from_utf8(b).ok())
            } else {
                Some(String::new())
            };
            if let Some(stylesheet) = stylesheet {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if std::fs::write(&path, &bytes).is_ok()
                    && write_companions(&path, output, &stylesheet, &mut diagnostics)
                {
                    explain_link(crate::build::cache::Status::Cached);
                    link_out_symlink(session, output);
                    return Ok(Artifact { target, path, bytes: bytes.len(), cached: true });
                }
            }
        }
    }
    explain_link(crate::build::cache::Status::Run);

    let compiled = compile_artifact(session, target, platform, flags, &mut diagnostics)?;
    cache.put(&key, compiled.module.as_bytes());
    if platform == Platform::Web {
        cache.put(&sheet_key, compiled.stylesheet.as_bytes());
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, &compiled.module) {
        diagnostics.push(
            Diagnostic::error(Span::NONE, format!("cannot write {}: {e}", path.display()))
                .with_fix("check the directory exists and is writable"),
        );
        return Err(diagnostics);
    }
    if !write_companions(&path, output, &compiled.stylesheet, &mut diagnostics) {
        return Err(diagnostics);
    }
    link_out_symlink(session, output);
    Ok(Artifact { target, path, bytes: compiled.module.len(), cached: false })
}

/// One compiled JavaScript artifact: the module, and the stylesheet its static
/// styles extracted to.
///
/// Two fields rather than one string because a WEB output writes both, and the
/// sheet cannot be recovered from the module afterwards without parsing
/// generated JavaScript back into a string literal. `stylesheet` is empty for
/// every program that is not a user interface, which is nearly all of them.
pub struct Compiled {
    /// The `.mjs`. This is the byte string the cache stores, the one
    /// `--check-reproducible` compares, and the whole of what a JS output
    /// writes — the sheet is inside it either way, as the constant `mount`
    /// installs.
    pub module: String,
    /// The same rules, as CSS, for the companion `.css` a WEB output writes.
    pub stylesheet: String,
}

/// The `link` action itself: sources in, artifact bytes out, and nothing on
/// disk touched either way.
///
/// Split out of `build_target` so that it can be run twice without the cache
/// and without an output directory, which is what `--check-reproducible` needs
/// and what makes "two builds of the same commit produce identical bytes"
/// something the toolchain can be asked rather than something a comment claims.
/// Analyse, insist on a `main`, and monomorphize from it.
///
/// The front end `compile_artifact` and `compile_objects` both run before they
/// part company over a backend. Two copies is two places for "a program is its
/// `main`" to be spelled, and they had already drifted over how the missing-
/// `main` diagnostic was laid out.
fn monomorphized_main(
    session: &mut Session,
    target: TargetId,
    platform: Platform,
    diagnostics: &mut Diagnostics,
) -> Result<(crate::compiler::driver::Analysis, monomorphize::Program), Diagnostics> {
    // `Some`: a build is the per-output check. See `Unit::platform`.
    let unit = Unit { target: Some(target), platform: Some(platform), with_tests: false };
    let mut analysis = crate::compiler::driver::analyze(
        Some(&session.workspace),
        &mut session.map,
        &mut session.parsed,
        &unit,
    );
    if analysis.diagnostics.has_errors() {
        return Err(analysis.diagnostics);
    }
    diagnostics.extend(std::mem::take(&mut analysis.diagnostics.items));

    let Some(entry) = analysis.checked.entry else {
        diagnostics.push(
            Diagnostic::templated("no-main", Span::NONE)
                .with_bind("package", session.workspace.package(target.package).label()),
        );
        return Err(std::mem::take(diagnostics));
    };

    let module_paths: Vec<String> =
        analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let program = monomorphize::run(
        &analysis.checked,
        module_paths,
        diagnostics,
        monomorphize::Roots::Main(entry),
    );
    if diagnostics.has_errors() {
        return Err(std::mem::take(diagnostics));
    }
    Ok((analysis, program))
}

pub fn compile_artifact(
    session: &mut Session,
    target: TargetId,
    platform: Platform,
    flags: &Flags,
    diagnostics: &mut Diagnostics,
) -> Result<Compiled, Diagnostics> {
    let (analysis, mut program) = monomorphized_main(session, target, platform, diagnostics)?;
    // The arch is `None` until a native backend has one to vary on: every
    // `Output` carries it and it is already in every key, but nothing below
    // here reads it while the only backend is JavaScript.
    let target = Target { platform, arch: None };
    // Read before `emit`, which takes the program by `&mut`. It is the same
    // text the backend is about to write into the module as `$ui_sheet`, so
    // the `.css` a WEB output writes and the `<style>` `mount` installs are
    // one string produced once.
    let stylesheet = program.stylesheet.clone();
    let module = emit(&mut program, &analysis.checked.tables, target, flags, diagnostics)?;
    Ok(Compiled { module, stylesheet })
}

/// The first byte at which two artifacts differ, or `None` when they are the
/// same bytes.
///
/// A byte offset rather than a diff: the artifacts are machine output, so what
/// a reader needs is somewhere to look and the fact that there is somewhere to
/// look. A length difference reports the first byte past the shorter one, which
/// is where the two stop agreeing.
pub fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    let common = a.len().min(b.len());
    if let Some((i, _)) = a.iter().zip(b).enumerate().find(|(_, (x, y))| x != y) {
        return Some(i);
    }
    (a.len() != b.len()).then_some(common)
}

/// The key for one action on one target. Paths are repository-relative, so two
/// checkouts in different directories produce identical keys.
pub fn action_key(
    session: &Session,
    target: TargetId,
    output: &Output,
    flags: &Flags,
    action: Action,
) -> ActionKey {
    let mut k = KeyBuilder::new(action, flags.mode);
    k.platform(output.platform(), output.arch());
    // Which backend will produce the bytes, and the identity of everything
    // outside the program that they depend on. The toolchain version does not
    // catch the second: `llvm-sys` links against whatever `llvm-config` found
    // at build time, so `--release` on LLVM 20 and `--release` on LLVM 21 are
    // two `buri` binaries with identical Rust source and different output, and
    // they must not share a cache entry.
    //
    // A platform with no backend keys as `none`: it has no bytes, and the
    // refusal happens before anything is emitted.
    match backend::select(target_of(output), profile_of(flags)) {
        Ok(b) => k.backend(b.name(), &b.identity()),
        Err(_) => k.backend("none", ""),
    }
    // Every target in the closure contributes its identity and its sources,
    // in a deterministic order.
    for member in session.workspace.closure(target) {
        contribute(session, member, &mut k);
    }
    k.finish()
}

/// One target's own contribution to a key: its rule identity, and the contents
/// of the sources that rule names. Factored out of `action_key` so it can also
/// be taken alone — which is what `--explain` reports per closure member, and
/// what makes "editing this file changed this target's key and not that one"
/// something a test can watch rather than something a comment asserts.
fn contribute(session: &Session, member: TargetId, k: &mut KeyBuilder) {
    let package = session.workspace.package(member.package);
    let kind = match member.kind {
        RuleKind::Library => "library",
        RuleKind::Binary => "binary",
    };
    let mut sources: Vec<String> = Vec::new();
    let entry = if member.kind == RuleKind::Library { "lib.buri" } else { "main.buri" };
    sources.push(entry.to_string());
    match member.kind {
        RuleKind::Library => {
            if let Some(lib) = &package.build.library {
                sources.extend(lib.sources.iter().map(|x| x.value.clone()));
                // A `.proto` is an input like any other: the module it becomes
                // is a pure function of its bytes, so editing a schema changes
                // this key exactly as editing a source does.
                sources.extend(lib.proto_sources.iter().map(|x| x.value.clone()));
                if let Some(testing) = &lib.testing {
                    sources.push("testing/lib.buri".into());
                    sources.extend(testing.sources.iter().map(|x| x.value.clone()));
                }
            }
        }
        RuleKind::Binary => {
            if let Some(bin) = &package.build.binary {
                sources.extend(bin.sources.iter().map(|x| x.value.clone()));
                sources.extend(bin.proto_sources.iter().map(|x| x.value.clone()));
            }
        }
    }
    sources.sort();
    k.rule_identity(&package.label(), kind, &sources);
    // Read in parallel, hashed in order. A key is a fold over the sources in
    // sorted order and that fold stays exactly where it was, on this thread; a
    // library of three hundred and sixty files is three hundred and sixty
    // `open`/`read`/`close` round trips, and those are what the cores are idle
    // for. `parallel::map` returns in index order, so the bytes reach the
    // builder in the order `sources` is in.
    let contents: Vec<Vec<u8>> = crate::parallel::map(sources.len(), |i| {
        sources
            .get(i)
            .map(|rel| std::fs::read(package.dir.join(rel)).unwrap_or_default())
            .unwrap_or_default()
    });
    for (rel, contents) in sources.iter().zip(&contents) {
        k.input(&session.workspace.rel_of(&package.dir.join(rel)), contents);
    }
}

/// The key for one target's own compilation: its identity and its own sources'
/// contents, and nothing from its dependencies.
///
/// This is not (yet) a cache key — no `compile` action is stored separately —
/// but it is the quantity the incrementality table in
/// `buri docs build/hermeticity` is written in terms of, so `--explain` reports
/// it and the tests compare it between two states of one tree.
fn compile_key(session: &Session, target: TargetId, output: &Output, flags: &Flags) -> ActionKey {
    let mut k = KeyBuilder::new(Action::Compile, flags.mode);
    k.platform(output.platform(), output.arch());
    contribute(session, target, &mut k);
    k.finish()
}

/// The schemas a rule declares, package-relative and sorted.
pub fn proto_sources(session: &Session, target: TargetId) -> Vec<String> {
    let package = session.workspace.package(target.package);
    let mut out: Vec<String> = match target.kind {
        RuleKind::Library => package
            .build
            .library
            .as_ref()
            .map(|l| l.proto_sources.iter().map(|x| x.value.clone()).collect())
            .unwrap_or_default(),
        RuleKind::Binary => package
            .build
            .binary
            .as_ref()
            .map(|b| b.proto_sources.iter().map(|x| x.value.clone()).collect())
            .unwrap_or_default(),
    };
    out.sort();
    out
}

/// The key for turning one rule's schemas into modules.
///
/// Content, not paths and not timestamps — the generated module is a pure
/// function of the schema text, so this is the whole of what it depends on.
/// The platform is in it for the same reason it is in every other key, even
/// though generation does not vary along it today: a key that leaves out
/// something a future action varies on is the shape of a stale-cache bug.
fn proto_key(session: &Session, target: TargetId, output: &Output, flags: &Flags) -> ActionKey {
    let mut k = KeyBuilder::new(Action::Proto, flags.mode);
    k.platform(output.platform(), output.arch());
    let package = session.workspace.package(target.package);
    let schemas = proto_sources(session, target);
    k.rule_identity(&package.label(), "proto", &schemas);
    for rel in &schemas {
        let full = package.dir.join(rel);
        k.input(&session.workspace.rel_of(&full), &std::fs::read(&full).unwrap_or_default());
    }
    k.finish()
}

/// Reports every action a build of `target` involves, deepest first: one
/// `proto` line per rule that declares a schema, one `compile` line per closure
/// member, then the `link` that consumed them.
fn explain_closure(session: &Session, target: TargetId, output: &Output, flags: &Flags) {
    if !flags.explain {
        return;
    }
    let platform = output.platform();
    for member in session.workspace.closure(target) {
        if !proto_sources(session, member).is_empty() {
            crate::build::cache::explain(
                true,
                crate::build::cache::Status::Keyed,
                Action::Proto,
                &session.workspace.label(member),
                platform,
                &proto_key(session, member, output, flags),
            );
        }
        let key = compile_key(session, member, output, flags);
        crate::build::cache::explain(
            true,
            crate::build::cache::Status::Keyed,
            Action::Compile,
            &session.workspace.label(member),
            platform,
            &key,
        );
    }
}

/// The key for a test suite: its own sources and data on top of the target's,
/// and the closure of every library its *test* code depends on.
///
/// The last of those was missing, and its absence was a stale-verdict bug
/// rather than a gap in coverage. `test { dependencies }` and a library's
/// `testing { dependencies }` are compiled *into* the suite — `Unit::with_tests`
/// loads the suite's sources, and their imports pull the helper's modules in —
/// but they are deliberately not in [`Workspace::closure`](crate::build::workspace::Workspace::closure),
/// because a test dependency is not a dependency of the thing being shipped. So
/// the base key, which walks the production closure, could not see them: editing
/// a test-only helper left every key unchanged and `buri test` served the
/// previous verdict for a suite whose code had changed.
///
/// Each test dependency contributes its own production closure, because that is
/// what compiling it involves — the helper's own `dependencies` are as much part
/// of the suite as the helper is.
pub fn test_key(session: &Session, target: TargetId, output: &Output, flags: &Flags) -> ActionKey {
    let base = action_key(session, target, output, flags, Action::Test);
    let mut k = KeyBuilder::new(Action::Test, flags.mode);
    k.dependency(&base);
    // Sorted and deduplicated: `test_dep_edges` yields declaration order, and a
    // key must not depend on the order two `dependencies` entries were written
    // in — nor count a helper twice because two of them reach it.
    let production = session.workspace.closure(target);
    let mut test_closure: Vec<TargetId> = Vec::new();
    for (dep, _) in session.workspace.test_dep_edges(target) {
        test_closure.extend(session.workspace.closure(dep));
    }
    test_closure.sort();
    test_closure.dedup();
    for member in test_closure {
        // A helper that is also a production dependency is already in the base
        // key. Contributing it again would be harmless but would make the key
        // depend on how a target reached it, which is not a fact about the
        // suite.
        if production.contains(&member) {
            continue;
        }
        contribute(session, member, &mut k);
    }
    let package = session.workspace.package(target.package);
    let suite = match target.kind {
        RuleKind::Library => package.build.library.as_ref().and_then(|l| l.test.as_ref()),
        RuleKind::Binary => package.build.binary.as_ref().and_then(|b| b.test.as_ref()),
    };
    if let Some(suite) = suite {
        let mut files: Vec<String> =
            suite.sources.iter().map(|x| x.value.clone()).collect();
        files.sort();
        k.rule_identity(&package.label(), "test", &files);
        for rel in &files {
            let full = package.dir.join(rel);
            k.input(rel, &std::fs::read(&full).unwrap_or_default());
        }
    }
    k.finish()
}

/// The middle end and then a backend, over one monomorphized program.
///
/// This is the seam the native backends arrive at: everything above it is
/// shared, everything below it is [`backend::select`]'s answer. The one thing
/// that is still JavaScript-shaped is the return type — a `String`, because a
/// JavaScript artifact is text and `buri test` appends to it before running it.
/// A native artifact is bytes, and it is the `link` step rather than this
/// function that will produce it.
pub fn emit(
    program: &mut monomorphize::Program,
    tables: &crate::compiler::semantics::types::Tables,
    target: Target,
    flags: &Flags,
    diagnostics: &mut Diagnostics,
) -> Result<String, Diagnostics> {
    let profile = profile_of(flags);
    prepare(program, target);

    let mut backend = match backend::select(target, profile) {
        Ok(b) => b,
        Err(_) => {
            // The same sentence `prepare_artifact` refuses with, from the same
            // function: `select`'s own message is one of the three [`native_gap`]
            // chooses between, and reporting it here without the other two would
            // be this site disagreeing with that one about what is wrong.
            let gap = native_gap(target, profile).unwrap_or_else(|| NativeGap {
                output: target.platform.slug().to_string(),
                reason: "this toolchain has no backend for it".to_string(),
                fix: "add `{ platform: JS }` to `outputs`".to_string(),
            });
            diagnostics.push(no_native_artifact(&gap, Span::NONE));
            return Err(std::mem::take(diagnostics));
        }
    };
    let opts = BackendOptions { profile, target, unit_prefix: "" };
    let units = match backend.emit(program, tables, &opts) {
        Ok(units) => units,
        Err(errors) => {
            diagnostics.extend(errors.items);
            return Err(std::mem::take(diagnostics));
        }
    };
    // One unit, and this is the backend for which that is always true. The
    // vector is the shape because a native build emits one object per codegen
    // unit; taking element zero here is what the JavaScript `Linker` does, and
    // it does it in one place rather than two.
    let Some(unit) = units.into_iter().next() else {
        diagnostics.push(Diagnostic::error(
            Span::NONE,
            String::from("internal error: the backend emitted no codegen unit"),
        ));
        return Err(std::mem::take(diagnostics));
    };
    match String::from_utf8(unit.bytes) {
        Ok(source) => Ok(source),
        Err(_) => {
            diagnostics.push(Diagnostic::error(
                Span::NONE,
                String::from("internal error: the backend emitted bytes that are not text"),
            ));
            Err(std::mem::take(diagnostics))
        }
    }
}

/// The middle end, composed for one target.
///
/// **This is the one place a pipeline is chosen.** `middle::run` is layer A, and
/// every backend consumes it; `middle::native` is the native branch —
/// `derives`, `closures`, `rc` — and JavaScript must not run it: closure
/// conversion is a pessimisation in a language with closures, a run-time
/// descriptor walk is what the JS runtime wants, and reference counting is
/// pointless in front of a garbage collector (`middle/mod.rs`, "Two layers").
///
/// It lives here rather than behind [`Backend::emit`](backend::Backend::emit)
/// because `middle::native` needs the program by `&mut` and a backend is handed
/// it by `&` — which is the type saying that a backend transforms nothing. So
/// the composition is the build system's, and there is exactly one of it:
/// [`emit`] and [`compile_objects`] both call this, and neither decides
/// anything else about the middle end.
///
/// Both profiles run the same passes, so that `release_and_debug_agree` keeps
/// covering the middle end rather than only the part of it release turns on.
///
/// The reference-counting plan comes back out, because the native branch's last
/// pass is the analysis [`lower::run`] would otherwise redo. `None` is the
/// JavaScript answer and means there is no plan rather than an empty one: a
/// garbage-collected target has no `incref` to place.
pub fn prepare(
    program: &mut monomorphize::Program,
    target: Target,
) -> Option<crate::compiler::middle::rc::Plan> {
    middle::run(program, &middle::Options::default());
    // The native branch is chosen by what the artifact *is*, and a WEB artifact
    // is JavaScript: closure conversion and reference counting are the same
    // pessimisation for a page that they are for a script.
    if target.platform.is_native() {
        return Some(middle::native(program));
    }
    None
}

/// The profile a set of flags names. One place, because `--release` decides
/// three things — inlining budget, defensive aborts, and name mangling — and
/// they must not be able to disagree about which build this is.
pub fn profile_of(flags: &Flags) -> Profile {
    if flags.mode.is_release() { Profile::Release } else { Profile::Debug }
}

/// What an `Output` names, in the form a backend wants it.
pub fn target_of(output: &Output) -> Target {
    Target { platform: output.platform(), arch: output.arch() }
}

/// The key for one codegen unit.
///
/// ```text
/// codegen_key(unit) = H(Codegen, toolchain_version, mode, platform, arch,
///                       backend.name(), backend.identity(),
///                       unit_prefix,
///                       H(the unit's lowered IR),
///                       H(the layout of every type the unit names))
/// ```
///
/// Content-addressed **on the IR**, not on source files, and that is the
/// decision the whole incremental story rests on. Keying a unit on the sources
/// of the module it came from — the way [`contribute`] keys a target — is wrong
/// in both directions. It is *unsound*, because a monomorphized unit contains
/// instantiations requested by other modules, so `core/list`'s object for a
/// program depends on which types that program maps over; and it is
/// *imprecise*, because reformatting a comment changes a file's bytes and not
/// one instruction of its IR.
///
/// The cost is that computing the key requires running the front end and the
/// whole middle end, so a `codegen` action can never be skipped without doing
/// the analysis. That is acceptable and nearly free here: the expensive half of
/// a native build is the half the key is protecting.
///
/// # Why the prefix is in it
///
/// The IR is not the whole of what a backend reads: `BackendOptions` is
/// `profile`, `target` and `unit_prefix`, and the first two are in this key
/// already. The third was not, and it is **observable in the emitted object**.
/// The LLVM backend builds a unit's module name from it
/// (`llvm/mod.rs`, `emit_selected`), and LLVM's `AsmPrinter` emits the module's
/// source-file name as a `.file` directive wherever the target's assembly
/// syntax has one — which is every ELF target and no Mach-O one. Two objects
/// from one IR with prefixes `""` and `lib/money`, `llc -filetype=obj` on
/// LLVM 21.1.2:
///
/// ```text
/// x86_64-unknown-linux-gnu    differ   .symtab STT_FILE `core_list` vs `moneycore_list`
/// aarch64-unknown-linux-gnu   differ   the same symbol
/// aarch64-apple-darwin        identical
/// x86_64-apple-darwin         identical
/// ```
///
/// So on a Linux host `//cmd/a` and `//cmd/b` sharing one `core/list` object
/// under one key is a hit that serves bytes codegen would not have produced —
/// and ARCHITECTURE.md §7 makes the prefix reach *more* of the object the day
/// debug info is emitted, since `DW_AT_comp_dir` and the Mach-O `N_OSO` stabs
/// are to be set from it. A key that omits an input to codegen is unsound on
/// whichever host makes the input visible, so the term is unconditional rather
/// than per-backend or per-platform.
///
/// What it costs is cross-target reuse, and that was measured before it was
/// spent: on a 118k-line repository with two native binaries over one library,
/// **2 of 369** codegen units were shared between the pair, and the cold
/// `buri build //...` cell does not move. Monomorphization is why — a unit's IR
/// is a function of the whole program it is in, so two binaries agree on a unit
/// only where neither instantiated anything the other did not. Reuse *within* a
/// target is untouched, and so is a batch's: `native_test_batch` gives every
/// member the same empty prefix. Adding the term does invalidate every existing
/// `codegen` and therefore every `link` entry, once.
///
/// The native object path is what calls this: it runs the front end and the
/// middle end, asks `unit_hashes` for a unit's IR and layout hashes, and gets
/// one key per unit back.
pub fn codegen_key(
    output: &Output,
    flags: &Flags,
    backend_name: &str,
    backend_identity: &str,
    unit_prefix: &str,
    ir_hash: &str,
    layout_hash: &str,
) -> ActionKey {
    let mut k = KeyBuilder::new(Action::Codegen, flags.mode);
    k.platform(output.platform(), output.arch());
    k.backend(backend_name, backend_identity);
    k.input("prefix", unit_prefix.as_bytes());
    k.input("ir", ir_hash.as_bytes());
    k.input("layout", layout_hash.as_bytes());
    k.finish()
}

/// The `link` key for a native artifact.
///
/// ```text
/// link_key = H(Link, toolchain_version, mode, platform, arch,
///              linker.name(), linker.version(), linker.link_identity(),
///              [codegen_key(u) for u in units],   // ordered
///              runtime_archive_hash | "omitted")
/// ```
///
/// Ordered, because link order determines symbol resolution order and therefore
/// determines the bytes. The runtime archive is in it because editing the
/// runtime relinks every artifact and recompiles none (BUILD-AND-WATCH.md
/// §2.2), and nothing else in this key would notice.
///
/// The last term is the archive's *decision* and not merely its digest
/// ([`link::RuntimeArchive`]). A link that does not name the archive does not
/// depend on it, so folding the digest in would relink an artifact whose bytes
/// could not have moved; and the two decisions have to be two keys, because
/// they are two command lines and therefore two artifacts. It stays **one**
/// term either way rather than becoming a digest plus a flag: an omitted
/// archive has no digest to state, and a term whose value is sometimes
/// meaningless is a term a reader has to be told to ignore.
///
/// This is a *different function* from `action_key(.., Action::Link)`, which
/// stays exactly what it was and is what a JavaScript artifact is keyed on. The
/// two do not meet: a native artifact is the ordered product of its objects,
/// and a JavaScript one is the product of its closure's sources.
pub fn link_key(
    output: &Output,
    flags: &Flags,
    linker: &dyn Linker,
    unit_keys: &[ActionKey],
    runtime: link::RuntimeArchive,
) -> ActionKey {
    link_key_of(flags.mode, target_of(output), linker, unit_keys, runtime)
}

/// [`link_key`] without a repository, which is what makes the three claims it
/// rests on testable as claims rather than as the shadow of a build: that the
/// unit keys enter **in order**, that the linker's identity enters at all, and
/// that the archive decision moves the key exactly when it moves.
pub fn link_key_of(
    mode: crate::commands::arguments::BuildMode,
    target: Target,
    linker: &dyn Linker,
    unit_keys: &[ActionKey],
    runtime: link::RuntimeArchive,
) -> ActionKey {
    let mut k = KeyBuilder::new(Action::Link, mode);
    k.platform(target.platform, target.arch);
    k.linker(linker.name(), &linker.version());
    // *How* the link runs, beside *who* runs it. The linker's banner does not
    // move when a toolchain gains a musl sysroot and starts linking
    // `-static-pie` against it, and the artifact is a different file — so
    // without this term the rebuilt toolchain is served the old one's
    // executable. See [`Linker::link_identity`].
    k.input("libc", linker.link_identity().as_bytes());
    for key in unit_keys {
        k.dependency(key);
    }
    match runtime {
        link::RuntimeArchive::Linked => k.input("runtime", runtime_archive_hash().as_bytes()),
        // Not the empty string: "this link named no archive" and "this link
        // named an archive whose digest is of no bytes" would otherwise be one
        // key, and on a host with no runtime the second is what the digest is.
        link::RuntimeArchive::Omitted => k.input("runtime", b"omitted"),
    }
    k.finish()
}

/// The runtime archive's hash, computed once for this process.
///
/// SHA-256 over six megabytes of embedded archive, and the archive is a
/// *constant of this binary* — `include_bytes!` at `runtime_native::ARCHIVE`. A
/// `buri test //...` builds a `link` key per suite, so a five-suite repository
/// hashed the same six megabytes five times and spent longer on it than on its
/// own front end. The term in the key is unchanged; only the number of times it
/// is computed is.
fn runtime_archive_hash() -> &'static str {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(runtime_native::archive_hash)
}

/// Whether this toolchain can produce and link a native artifact for `target`.
///
/// Three questions, and a `false` from any of them means the build refuses with
/// the diagnostic it refused with before any of this existed. That is the gate
/// every native build passes through: nothing here changes what a
/// `--output=linux/x86_64` build does on a toolchain with no native backend
/// compiled in.
///
/// `backend::select` answers the *target* question and not merely the platform
/// one, which is what this relies on: the development backend has a triple it
/// has no stencil library for, and without that a host of that kind would be
/// told the build is ready and then refused inside the emission.
pub fn native_ready(target: Target, profile: Profile) -> bool {
    target.platform.is_native() && native_gap(target, profile).is_none()
}

/// One reason this toolchain cannot produce a native artifact, in the three
/// pieces every refusal site prints: what was asked for, what is missing, and
/// what to do. [`native_gap`] is what fills it in.
pub struct NativeGap {
    /// The output, spelled the way a build file and `--output` spell it —
    /// `linux/x86_64`, or `macos` where the output named no architecture.
    ///
    /// **Not the target triple**, which is what this said first and what the
    /// goldens caught: a triple is `aarch64-unknown-linux-musl` on one host and
    /// `x86_64-apple-darwin` on another for the same *declared* output, so a
    /// recorded diagnostic holding one pins the runner. This is the spelling
    /// the reader wrote, `tests/harness/case.rs` already has a placeholder for
    /// each half of it, and the triple is `backend::triple_text`'s business
    /// rather than a diagnostic's.
    pub output: String,
    /// Which of the three is missing.
    pub reason: String,
    /// What the reader can do about it.
    pub fix: String,
}

/// Why this toolchain cannot produce a native artifact for `target` under
/// `profile` — the same three questions [`native_ready`] asks, answered as a
/// sentence instead of as a `false` — or `None` where it can.
///
/// The `bool` came first and it threw the answer away, which is the whole of
/// what two reports were about. `backend::select` already computed a reason and
/// `native_ready` discarded it, so a `--release` build on a toolchain without
/// `backend-llvm` and a `linux/x86_64` output on a mac were refused with the
/// same sentence — *"the macos backend is not implemented"*, *"this toolchain
/// emits JavaScript"* — and neither half was true on a host whose debug native
/// build of the same output had just succeeded (buri-lang/buri#25,
/// buri-lang/buri#26).
///
/// The order is structural rather than arbitrary: the **host** questions come
/// first, because a cross target is refused whatever the profile and saying
/// "install `backend-llvm`" to somebody on a mac asking for Linux would be
/// advice that does not help. The profile question is last, and it is the only
/// one whose fix is about the *invocation* rather than about the machine.
pub fn native_gap(target: Target, profile: Profile) -> Option<NativeGap> {
    let output = match target.arch {
        Some(arch) => format!("{}/{}", target.platform.slug(), arch.slug()),
        None => target.platform.slug().to_string(),
    };
    let js = "add `{ platform: JS }` to `outputs`";
    // The host, first: a cross target is refused on every profile and by a
    // constant this file cannot change.
    if !link::can_link(target) {
        return Some(NativeGap {
            // **Neither the host nor the target is named in this sentence**,
            // and that is the harness's constraint rather than a shortage of
            // words: `tests/harness/case.rs` has no `HOST_ARCH` placeholder,
            // because a runner's architecture is a property of the machine
            // rather than of the toolchain, and a golden holding one would pin
            // the machine. The target's own triple carries the host's
            // architecture whenever the output declared none, so naming either
            // makes this text move from runner to runner. Both are in the
            // diagnostic anyway — the target in the headline, the machine that
            // could build it in the fix — and `commands/test.rs` reuses this
            // reason under a headline of its own.
            reason: "this toolchain builds a native artifact for its own host only: the \
                     runtime archive, the C library and the linker are all the host's"
                .to_string(),
            fix: format!("build this output on a {output} host, or {js}"),
            output,
        });
    }
    if !runtime_native::AVAILABLE {
        return Some(NativeGap {
            reason: "this toolchain carries no native runtime archive: `cli/build.rs` builds \
                     one for a macOS or Linux host, and writes nothing on any other"
                .to_string(),
            fix: format!("{js}, or install a toolchain built on macOS or Linux"),
            output,
        });
    }
    // The backend last, because it is the only one of the three whose answer
    // depends on the profile and the only one a different invocation can change.
    if let Err(reason) = backend::select(target, profile) {
        let release_only =
            profile == Profile::Release && backend::select(target, Profile::Debug).is_ok();
        let fix = if release_only {
            format!(
                "build without `--release` — the development backend emits {output} — or \
                 install a toolchain built with `backend-llvm`"
            )
        } else {
            format!("declare an output this toolchain can build, or {js}")
        };
        return Some(NativeGap { output, reason, fix });
    }
    None
}

/// [`NativeGap`] as the diagnostic every refusal site prints.
///
/// One function so that `buri build`, `buri test` and `--check-reproducible`
/// cannot describe the same gap three ways.
pub fn no_native_artifact(gap: &NativeGap, span: Span) -> Diagnostic {
    Diagnostic::templated("native-artifact-not-available", span)
        .with_bind("output", gap.output.clone())
        .with_bind("reason", gap.reason.clone())
        .with_bind("fix", gap.fix.clone())
}

/// One unit's two hashes: the lowered IR it is made of, and the layout of every
/// type it names.
///
/// Both are text, and both are rendered by code that already exists for a
/// reader — `ir::Program::render_func` and `Layout`'s `Debug` — because a hash
/// nobody can print is a hash nobody can debug when it moves and no one knows
/// why. `ir.rs`'s printing section states the property this relies on: the
/// rendering is total, deterministic and derived entirely from the program,
/// with no hash iteration order anywhere in it.
///
/// The types a unit "names" are the aggregates in its functions' signatures, in
/// their values' types, and in the structural operations that take one. That is
/// the whole set a backend can ask `Layouts::of` about, because an aggregate a
/// unit touches is the type of some value in it. The `Layout` that goes into
/// the hash is the *computed* one — sizes, alignments and every field offset —
/// so a change to a field's type deep inside a record is caught without this
/// having to walk to it.
///
/// The shape lines are sorted **as text**, not by `TypeId`. A `TypeId` is a
/// program-global interning index, so ordering by it makes an unrelated unit's
/// `shapes` string depend on which types some other module happened to name
/// first — the same defect the IR rendering had when it named callees by
/// `FuncIdx`. Sorting the rendered lines gives a total order derived from the
/// content, so the string moves only when a layout this unit names moves.
fn unit_hashes(program: &ir::Program, tables: &Tables) -> Vec<(String, String, String)> {
    // Grouped in one pass rather than scanned once per unit: the filter was
    // `units * funcs`, which is the whole program re-walked for every unit.
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); program.units.len()];
    for (i, func) in program.funcs.iter().enumerate() {
        if let Some(slot) = members.get_mut(func.unit as usize) {
            slot.push(i);
        }
    }
    // A unit at a time, over the cores this machine has. Each unit's two hashes
    // are a pure function of the unit's members and of `tables`, and nothing
    // here writes to the program — so the only per-worker state is the `Layouts`
    // memo, which is a cache of an answer rather than an answer that
    // accumulates. `parallel::map_with` returns in index order, which is what
    // keeps `keys` in unit order for `codegen_units_for` and in link order for
    // `link_key`.
    crate::parallel::map_with(
        program.units.len(),
        || layout::Layouts::new(tables),
        |layouts, u| {
            let name = program.units.get(u).cloned().unwrap_or_default();
            let mut text = String::new();
            let mut types: Vec<usize> = Vec::new();
            for func in members.get(u).map(Vec::as_slice).unwrap_or_default() {
                let Some(func) = program.funcs.get(*func) else { continue };
                program.render_func_into(func, &mut text);
                collect_types(func, &mut types);
            }
            types.sort_unstable();
            types.dedup();
            let mut lines: Vec<String> = Vec::with_capacity(types.len());
            for id in types {
                let Some(info) = program.types.get(id) else { continue };
                lines.push(format!("{} {:?}\n", info.name, layouts.of(info.ty.clone())));
            }
            lines.sort();
            let shapes: String = lines.concat();
            (name, hash_bytes(text.as_bytes()), hash_bytes(shapes.as_bytes()))
        },
    )
}

/// Every aggregate type one function names, as indices into `Program::types`.
fn collect_types(func: &ir::Func, out: &mut Vec<usize>) {
    for t in func.sig.params.iter().chain(&func.sig.rets) {
        if let ir::Type::Agg(id) = t {
            out.push(id.index());
        }
    }
    let Some(code) = func.code() else { return };
    for i in 0..code.values() {
        if let ir::Type::Agg(id) = code.ty_of(ir::ValueId(i as u32)) {
            out.push(id.index());
        }
    }
    for block in &code.blocks {
        for inst in &block.insts {
            if let ir::Inst::Structural { ty, .. } = inst {
                out.push(ty.index());
            }
        }
    }
}

/// The objects for one program, from the cache where the cache has them.
///
/// `emit` is a closure rather than a `&mut dyn Backend` for two reasons. It is
/// what makes "the backend was never asked" a *fact this function establishes*
/// rather than a claim about a call it happened not to make — a test can pass a
/// closure that panics and watch nothing happen. And it is what lets the whole
/// of this be tested before either native backend exists, which is what
/// `tests/native/link.rs` does.
///
/// What is honest about the result, and what is not yet:
///
/// - A unit whose key hits is served **from the cache**. Its bytes are the
///   previous build's bytes, not this one's, which is what makes an unchanged
///   object an unchanged object.
/// - When *every* unit hits, `emit` is never called at all. That is the case a
///   watch loop hits on every keystroke inside a comment, and it is where the
///   seconds are.
/// - When a unit misses, `emit` is called **once, with the units that missed**,
///   and the units that hit are served from the cache and report `cached`. That
///   parameter is [`Units`](crate::compiler::backend::Units): without it,
///   invalidating one unit of several hundred cost exactly what `--force`
///   costs, because the backend re-emitted every unit and every object but one
///   was thrown away.
/// - A backend may return more objects than were asked for — the default
///   `emit_units` does — because the selection here is by name. It may not
///   return fewer: a unit that missed and has no object is the internal error
///   below.
///
/// The `Emitted::key` a backend attaches is *replaced* by the one the build
/// system computed. That is not a slight: the cache is the build system's, and
/// an entry is only useful if its key can be computed **before** the work that
/// would fill it — which a key the emitter produces cannot be, because
/// producing it is the work. The backend's own key stays what its doc comment
/// says it is, a statement about which of the backend's inputs the bytes depend
/// on, and that statement enters here through `Backend::identity`, which is in
/// every `codegen` key.
fn codegen_units_for<F>(
    cache: &Cache,
    keys: &[(String, ActionKey)],
    force: bool,
    emit: F,
) -> Result<Vec<(Emitted, bool)>, Diagnostics>
where
    F: FnOnce(&[u32]) -> Result<Vec<Emitted>, Diagnostics>,
{
    let hits: Vec<Option<Vec<u8>>> =
        keys.iter().map(|(_, k)| if force { None } else { cache.get(k) }).collect();
    if hits.iter().all(Option::is_some) {
        let mut out = Vec::with_capacity(keys.len());
        for ((name, key), bytes) in keys.iter().zip(hits) {
            let bytes = bytes.unwrap_or_default();
            out.push((Emitted { name: object_name(name), key: key.clone(), bytes }, true));
        }
        return Ok(out);
    }

    // `keys` is in unit order, because `unit_hashes` walks `Program::units`, so
    // a position in it is the `Func::unit` the backend selects on.
    let wanted: Vec<u32> = hits
        .iter()
        .enumerate()
        .filter(|(_, hit)| hit.is_none())
        .filter_map(|(i, _)| u32::try_from(i).ok())
        .collect();
    let fresh = emit(&wanted)?;
    let mut out = Vec::with_capacity(keys.len());
    for ((name, key), hit) in keys.iter().zip(hits) {
        if let Some(bytes) = hit {
            out.push((Emitted { name: object_name(name), key: key.clone(), bytes }, true));
            continue;
        }
        let wanted = object_name(name);
        let Some(unit) = fresh.iter().find(|e| e.name == wanted || e.name == *name) else {
            let mut diagnostics = Diagnostics::new();
            diagnostics.push(Diagnostic::error(
                Span::NONE,
                format!("internal error: the backend emitted no object for unit `{name}`"),
            ));
            return Err(diagnostics);
        };
        cache.put(key, &unit.bytes);
        out.push((
            Emitted { name: wanted, key: key.clone(), bytes: unit.bytes.clone() },
            false,
        ));
    }
    Ok(out)
}

/// [`codegen_units_for`], for a caller with no per-unit emission path.
///
/// The units the emitter is told about are dropped rather than ignored: a
/// closure that produces the whole program produces every unit that missed, and
/// selecting by name is what this hands back.
pub fn codegen_units<F>(
    cache: &Cache,
    keys: &[(String, ActionKey)],
    force: bool,
    emit: F,
) -> Result<Vec<(Emitted, bool)>, Diagnostics>
where
    F: FnOnce() -> Result<Vec<Emitted>, Diagnostics>,
{
    codegen_units_for(cache, keys, force, |_| emit())
}

/// `core_list` -> `core_list.o`. One rule, because the manifest names the unit
/// and the linker names the file, and they have to agree.
pub fn object_name(unit: &str) -> String {
    format!("{unit}.o")
}

/// Everything a native build produces before the link: the objects, and the
/// record of where each came from.
pub struct Objects {
    pub units: Vec<Emitted>,
    pub rows: Vec<link::Row>,
    pub keys: Vec<ActionKey>,
}

/// The front end, the middle end, the codegen keys, and the objects.
///
/// The native twin of [`compile_artifact`], and split out for the same reason:
/// `--check-reproducible` needs to run it twice with the cache off and compare
/// the objects it produced, and a function that also wrote an executable could
/// not be asked that.
/// The linker for an output, or the diagnostic that says why there is none.
///
/// One function because the refusal is one refusal: `cc` is how a native link
/// is driven, so a host without one cannot link, and saying so twice in two
/// wordings would be two answers to one question.
fn linker_for(output: &Output, diagnostics: &mut Diagnostics) -> Option<link::CDriver> {
    match link::select(target_of(output)) {
        Ok(l) => Some(l),
        Err(refusal) => {
            // The wording is `link::select`'s and not this function's. There
            // are three refusals now — wrong platform, no `cc`, and a
            // toolchain that cannot link hermetically — and only the module
            // that told them apart can say which remedy belongs to which. This
            // site's job is the span.
            let mut d = Diagnostic::error(output.span, refusal.message);
            for note in refusal.notes {
                d.note(note);
            }
            diagnostics.push(d.with_fix(refusal.fix));
            None
        }
    }
}

pub fn compile_objects(
    session: &mut Session,
    target: TargetId,
    output: &Output,
    flags: &Flags,
    diagnostics: &mut Diagnostics,
) -> Result<Objects, Diagnostics> {
    let platform = output.platform();
    let (analysis, mut program) = monomorphized_main(session, target, platform, diagnostics)?;
    objects_of(session, target, output, flags, &mut program, &analysis.checked.tables, diagnostics)
}

/// The middle end, the codegen keys and the objects, over a program somebody
/// else monomorphized.
///
/// The half of [`compile_objects`] below the front end, and it is split out for
/// the same reason that function was split out of `build_native`: a **test**
/// binary is a program with `ProgramRoots::Tests` rather than a `main`, and
/// everything from here down is the same. Two callers, one composition — which
/// is what stops `buri test --platform=macos` from being a second pipeline that
/// drifts from the one `buri build` uses.
pub fn objects_of(
    session: &mut Session,
    target: TargetId,
    output: &Output,
    flags: &Flags,
    program: &mut monomorphize::Program,
    tables: &Tables,
    diagnostics: &mut Diagnostics,
) -> Result<Objects, Diagnostics> {
    // Repository-relative, so that two checkouts in different directories put
    // the same string in a debug section. This is the same rule `action_key`
    // follows for input paths, and it is the source of nondeterminism
    // ARCHITECTURE.md §7 calls out by name.
    let prefix = session.workspace.package(target.package).path.clone();
    let label = session.workspace.label(target);
    objects_named(session, &prefix, &label, output, flags, program, tables, diagnostics)
}

/// [`objects_of`] with the two things it takes a target for named directly: the
/// repository-relative prefix a debug section records, and the label
/// `--explain` reports each unit under.
///
/// Split out because a **batched** test binary is one program built from several
/// suites, so neither of the two has a single target to come from. Everything
/// else — the middle end, the keys, the per-unit cache — is a function of the
/// program, and this is the seam that says so.
#[allow(
    clippy::too_many_arguments,
    reason = "the two strings a target used to stand in for are now named, and \
              neither is derivable from the program, the output or the flags"
)]
fn objects_named(
    session: &mut Session,
    prefix: &str,
    label: &str,
    output: &Output,
    flags: &Flags,
    program: &mut monomorphize::Program,
    tables: &Tables,
    diagnostics: &mut Diagnostics,
) -> Result<Objects, Diagnostics> {
    let platform = output.platform();
    let profile = profile_of(flags);
    let back_target = target_of(output);
    // The same composition `emit` runs, from the same function: this path
    // reaches a native backend and that one reaches JavaScript, and which
    // passes a target gets must not be a fact stated twice.
    let plan = prepare(program, back_target);
    // Lowered here for the *keys* only. `Backend::emit` lowers again for the
    // bytes, because `middle::lower` is deterministic and a pure function of the
    // program, so the two agree by construction. Handing the `ir::Program`
    // straight to the backend would save the second lowering, and it would mean
    // a second entry point on the trait — the backends have one (`emit_lowered`)
    // and it is deliberately not on `Backend`, so that there is one seam rather
    // than one seam and a shortcut.
    // Against the plan `prepare` already produced. `lower::run` would compute
    // an identical one — it is a pure function of the program, and nothing has
    // taken the program by `&mut` since — so this is the same lowering with one
    // whole-program analysis in it instead of two.
    let lowered = match &plan {
        Some(plan) => lower::run_with(program, tables, plan),
        None => lower::run(program, tables),
    };

    let mut backend = match backend::select(back_target, profile) {
        Ok(b) => b,
        Err(message) => {
            diagnostics.push(Diagnostic::error(output.span, message));
            return Err(std::mem::take(diagnostics));
        }
    };
    let missing = backend.missing_intrinsics(program, tables);
    if !missing.is_empty() {
        // One diagnostic per cause rather than one per program: an operation a
        // toolchain built without the runtime's `net` feature cannot answer is
        // a different sentence, and asks for a different thing, from an
        // operation the backend has no body for.
        let (networking, rest) = backend::split_networking(&missing);
        let (cryptography, rest) = backend::split_cryptography(&rest);
        if !networking.is_empty() {
            diagnostics.push(backend::no_networking(&networking, Span::NONE));
        }
        if !cryptography.is_empty() {
            diagnostics.push(backend::no_cryptography(&cryptography, Span::NONE));
        }
        if !rest.is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    Span::NONE,
                    format!(
                        "the {} backend has no implementation of {}",
                        backend.name(),
                        rest.join(", ")
                    ),
                )
                .with_fix("report it: this is a toolchain bug, not a problem with your program"),
            );
        }
        return Err(std::mem::take(diagnostics));
    }

    let name = backend.name().to_string();
    let identity = backend.identity();
    let keys: Vec<(String, ActionKey)> = unit_hashes(&lowered, tables)
        .into_iter()
        .map(|(unit, ir_hash, layout_hash)| {
            let key = codegen_key(output, flags, &name, &identity, prefix, &ir_hash, &layout_hash);
            (unit, key)
        })
        .collect();

    let cache = Cache::open(&session.root);
    let emitted = codegen_units_for(&cache, &keys, flags.force, |wanted| {
        let opts = BackendOptions { profile, target: back_target, unit_prefix: prefix };
        // `Units::Only` is a membership test per unit, so a build that wants
        // every unit — a first build, or `--force` — says so rather than
        // scanning a list of every unit once per unit.
        let selection =
            if wanted.len() == keys.len() { Units::All } else { Units::Only(wanted) };
        backend.emit_units(program, tables, &opts, selection)
    })?;

    let mut units = Vec::with_capacity(emitted.len());
    let mut rows = Vec::with_capacity(emitted.len());
    for ((unit, key), (object, cached)) in keys.iter().zip(emitted) {
        crate::build::cache::explain(
            flags.explain,
            if cached { crate::build::cache::Status::Cached } else { crate::build::cache::Status::Run },
            Action::Codegen,
            &format!("{label}:{unit}"),
            platform,
            key,
        );
        rows.push(link::Row { unit: unit.clone(), key: key.as_str().to_string(), cached });
        units.push(object);
    }
    Ok(Objects { units, rows, keys: keys.into_iter().map(|(_, k)| k).collect() })
}

/// A native build: codegen per unit, then one full link.
fn build_native(
    session: &mut Session,
    target: TargetId,
    output: &Output,
    flags: &Flags,
    mut diagnostics: Diagnostics,
) -> Result<Artifact, Diagnostics> {
    let platform = output.platform();
    let path = artifact_path(session, target, output);
    let Some(linker) = linker_for(output, &mut diagnostics) else { return Err(diagnostics) };

    explain_closure(session, target, output, flags);
    let objects = compile_objects(session, target, output, flags, &mut diagnostics)?;
    // Asked here and again inside the linker, of the same objects, because it
    // is a pure function of them: the key has to name the command line the link
    // is about to run, and the linker has to build that command line.
    let runtime = link::runtime_archive_for(&objects.units);
    let key = link_key(output, flags, &linker, &objects.keys, runtime);
    let linker = linker.in_dir(link::dir(&session.root, key.as_str()));
    let label = session.workspace.label(target);
    let explain_link = |status: crate::build::cache::Status| {
        crate::build::cache::explain(flags.explain, status, Action::Link, &label, platform, &key);
    };

    let cache = Cache::open(&session.root);
    // "The fastest link is the one that does not run": every unit's key
    // unchanged means the ordered list in `key` is unchanged, so the executable
    // in the cache is the executable this link would produce.
    if !flags.force {
        if let Some(bytes) = cache.get(&key) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if write_executable(&path, &bytes).is_ok() {
                explain_link(crate::build::cache::Status::Cached);
                link_out_symlink(session, output);
                return Ok(Artifact { target, path, bytes: bytes.len(), cached: true });
            }
        }
    }
    explain_link(crate::build::cache::Status::Run);

    let prefix = session.workspace.package(target.package).path.clone();
    let opts = LinkOptions {
        profile: profile_of(flags),
        target: target_of(output),
        unit_prefix: &prefix,
    };
    if let Err(errors) = link::run(&objects.units, &objects.rows, &linker, &path, &opts) {
        diagnostics.extend(errors.items);
        return Err(diagnostics);
    }
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => {
            diagnostics.push(Diagnostic::error(
                Span::NONE,
                format!("the link produced no {}: {e}", path.display()),
            ));
            return Err(diagnostics);
        }
    };
    cache.put(&key, &bytes);
    link_out_symlink(session, output);
    Ok(Artifact { target, path, bytes: bytes.len(), cached: false })
}

/// Where a native test binary was put, for as long as it is the one to run.
///
/// A value rather than a path because the shared runner file below is *claimed*,
/// and the claim is released when the suite that took it has finished with it.
/// Holding this is what says "this file is mine until I drop it".
pub struct TestBinary {
    path: PathBuf,
    _claim: Option<Claim>,
}

impl TestBinary {
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

/// One process's hold on the shared runner file.
struct Claim {
    lock: PathBuf,
}

impl Drop for Claim {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock);
    }
}

/// How long a claim may be held before it is read as a crashed process's.
///
/// Generous, because what is under it is a suite running to completion and a
/// suite may declare a `timeout_seconds` of its own. Stealing early would cost
/// exactly the thing the claim exists to prevent, and stealing late costs a
/// slower run — so the asymmetry is resolved in the direction of the answer
/// being right.
const CLAIM_STALE: std::time::Duration = std::time::Duration::from_secs(900);

/// The file a suite's native test binary is written to and executed from.
///
/// **One file per platform for the whole repository, not one per package**, and
/// that is a measurement rather than a tidy-up. macOS charges about 200 ms the
/// first time a *newly created* file is executed; the charge is on the file's
/// identity, so a file that has been executed once costs about 4 ms however many
/// times it is rewritten with different bytes afterwards — measured, and the
/// same effect `link::place` is written against. A test binary per package
/// meant a cold `buri test //...`
/// created five files that had never been executed and paid the charge five
/// times — more than half of that run.
///
/// The file is shared, so it is claimed: a lock file beside it, taken with
/// `create_new`, held for as long as the caller holds the [`TestBinary`], and
/// **never waited on**. A suite that cannot take it writes to the per-package
/// path instead and pays the charge, which is what every suite used to do. That
/// is what keeps "all commands are safe to run concurrently" (CLI.md) true:
/// two `buri test` processes in one repository do not share a file, they take
/// turns at one and the loser is merely slower.
///
/// The shared file in `dir` where this process can take it, and `private` where
/// it cannot. `private` is the caller's because a batched run has no one package
/// to derive it from (see [`native_test_batch`]).
fn claim_runner(dir: &std::path::Path, private: PathBuf) -> TestBinary {
    claim_runner_after(dir, private, CLAIM_STALE)
}

/// The same, with the staleness bound named, so that "a claim this old is a
/// crashed process's" is a rule a test can state rather than one it has to wait
/// out.
fn claim_runner_after(
    dir: &std::path::Path,
    private: PathBuf,
    stale: std::time::Duration,
) -> TestBinary {
    let _ = std::fs::create_dir_all(dir);
    let lock = dir.join(".test-runner.lock");
    // A claim older than `stale` belongs to a process that is not running any
    // more, and a repository that one `^C` can slow down for good is a
    // repository nobody trusts. `cache::Lock` steals on the same argument.
    let abandoned = std::fs::metadata(&lock)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|m| m.elapsed().ok())
        .is_some_and(|age| age >= stale);
    if abandoned {
        let _ = std::fs::remove_file(&lock);
    }
    match std::fs::OpenOptions::new().create_new(true).write(true).open(&lock) {
        Ok(_) => TestBinary { path: dir.join("test-runner"), _claim: Some(Claim { lock }) },
        Err(_) => {
            if let Some(parent) = private.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            TestBinary { path: private, _claim: None }
        }
    }
}

/// A native **test** binary: the same codegen and the same link, over a program
/// rooted at the suite's tests rather than at a `main`.
///
/// Written to the file [`claim_runner`] names and left there, because unlike
/// an artifact it is not something the repository asked for — it is the shape a
/// test run takes on a native platform, and `buri test` executes it and forgets
/// it.
///
/// The link **is** cached, and the reason is that the verdict cache in front of
/// this and the `link` key answer two different questions. `test_key` is over
/// the suite's *source bytes*, so it misses on every edit a reader can make;
/// `link_key` is the ordered list of `codegen` keys, and those are over the
/// lowered IR. Everything the front and middle end erase lies between the two —
/// a comment, a reformatting, a renamed local, a function nothing calls, a
/// change in a sibling module the suite does not reach — and for all of it the
/// suite has to run again while the binary that runs is byte for byte the one
/// that ran last time. Relinking it is 150 ms of `cc` for an answer already on
/// disk.
///
/// The bytes are the artifact's, so a hit is the same executable rather than an
/// equivalent one, and `--force` skips the lookup as it does everywhere else.
#[allow(
    clippy::too_many_arguments,
    reason = "the session, the target, the output, the flags, the program, its \
              tables and where to put the diagnostics: seven things none of \
              which is derivable from the others, and a struct bundling them \
              would name each of them twice"
)]
pub fn native_test_binary(
    session: &mut Session,
    target: TargetId,
    output: &Output,
    flags: &Flags,
    program: &mut monomorphize::Program,
    tables: &Tables,
    diagnostics: &mut Diagnostics,
) -> Result<TestBinary, Diagnostics> {
    let prefix = session.workspace.package(target.package).path.clone();
    let label = session.workspace.label(target);
    let dir = session.root.join(".buri/out").join(output.dir());
    let private = dir.join(&session.workspace.package(target.package).path).join("test");
    test_binary_named(
        session, &prefix, &label, private, output, flags, program, tables, diagnostics,
    )
}

/// A native test binary for **several** suites at once: one program, one link,
/// one file.
///
/// The suites are already in `program` — `driver::analyze_all` loaded their test
/// sources into one compilation and `monomorphize::Roots::Tests` rooted it at
/// every `test` block it found — so nothing here is aware of how many there are.
/// What it takes instead of a target is the two things a target stood in for:
///
/// - **`unit_prefix` is empty.** A batch spans packages, so no package's path is
///   the program's; the repository root is. It is also the answer that does not
///   depend on *which* suites are in the batch, and the batch's membership is a
///   function of which verdicts were already cached — so any prefix taken from a
///   member would give one batch's objects a different key on every run whose
///   membership differed, and no two batched runs would reuse each other's.
///   (The prefix is a term of `codegen_key`, so this is a reuse question rather
///   than a correctness one; see the note there for why it is a term.)
/// - **The private path** is the caller's, for the same reason [`claim_runner`]
///   takes one: the shared runner file is claimed, and a batch that cannot take
///   the claim needs somewhere of its own to run from.
///
/// Everything else is [`native_test_binary`]'s, including the `link` cache: the
/// key is the ordered list of the batch's `codegen` keys, so two runs whose
/// batches hold the same suites with the same IR relink nothing, and two runs
/// whose batches differ are two different keys rather than one wrong one.
#[allow(
    clippy::too_many_arguments,
    reason = "one more than `native_test_binary`, and the one more is the file to \
              run from — which is what the target used to decide"
)]
pub fn native_test_batch(
    session: &mut Session,
    targets: &[TargetId],
    output: &Output,
    flags: &Flags,
    program: &mut monomorphize::Program,
    tables: &Tables,
    diagnostics: &mut Diagnostics,
) -> Result<TestBinary, Diagnostics> {
    let label = targets.iter().map(|t| session.workspace.label(*t)).collect::<Vec<_>>().join(",");
    // The first member's own path, so that a batch that cannot take the shared
    // claim runs from a file some previous run has already executed rather than
    // from a new one. Deterministic: `targets` is in the pass's order.
    let dir = session.root.join(".buri/out").join(output.dir());
    let private = match targets.first() {
        Some(t) => dir.join(&session.workspace.package(t.package).path).join("test"),
        None => dir.join("test"),
    };
    test_binary_named(session, "", &label, private, output, flags, program, tables, diagnostics)
}

/// The body of both: link this program, cache it, and put it where it can be
/// run from.
#[allow(
    clippy::too_many_arguments,
    reason = "the target is gone and the two things it decided — the debug prefix \
              and the file to fall back to — are arguments, which is one more \
              rather than a different shape"
)]
fn test_binary_named(
    session: &mut Session,
    prefix: &str,
    label: &str,
    private: PathBuf,
    output: &Output,
    flags: &Flags,
    program: &mut monomorphize::Program,
    tables: &Tables,
    diagnostics: &mut Diagnostics,
) -> Result<TestBinary, Diagnostics> {
    let Some(linker) = linker_for(output, diagnostics) else {
        return Err(std::mem::take(diagnostics));
    };
    let objects =
        objects_named(session, prefix, label, output, flags, program, tables, diagnostics)?;
    let runtime = link::runtime_archive_for(&objects.units);
    let key = link_key(output, flags, &linker, &objects.keys, runtime);
    let linker = linker.in_dir(link::dir(&session.root, key.as_str()));
    let explain_link = |status: crate::build::cache::Status| {
        crate::build::cache::explain(flags.explain, status, Action::Link, label, output.platform(), &key);
    };
    // Claimed after the objects exist and before anything is written, so a run
    // that fails to compile never takes the shared file at all.
    let binary = claim_runner(&session.root.join(".buri/out").join(output.dir()), private);
    let path = binary.path().to_path_buf();
    let cache = Cache::open(&session.root);
    if !flags.force {
        if let Some(bytes) = cache.get(&key) {
            if write_executable(&path, &bytes).is_ok() {
                explain_link(crate::build::cache::Status::Cached);
                return Ok(binary);
            }
        }
    }
    explain_link(crate::build::cache::Status::Run);
    let opts =
        LinkOptions { profile: profile_of(flags), target: target_of(output), unit_prefix: prefix };
    if let Err(errors) = link::run(&objects.units, &objects.rows, &linker, &path, &opts) {
        diagnostics.extend(errors.items);
        return Err(std::mem::take(diagnostics));
    }
    if let Ok(bytes) = std::fs::read(&path) {
        cache.put(&key, &bytes);
    }
    Ok(binary)
}

/// Writes an executable, and makes it one.
///
/// A cached artifact is bytes out of a content-addressed store, and bytes out
/// of a store have no mode. Restoring the execute bit is what makes a cache hit
/// and a fresh link produce the same thing rather than a file that differs from
/// it in the one way `ls` shows and `cmp` does not.
fn write_executable(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    // Through `link::place`, which is where the rule about *not* rewriting an
    // artifact whose bytes are already there is stated, and which is what a
    // fresh link now reaches its output through as well. Two ways of putting an
    // executable on disk would be two places for that rule to hold in one of
    // them.
    link::place(path, bytes)
}

pub fn artifact_path(session: &Session, target: TargetId, output: &Output) -> PathBuf {
    let package = session.workspace.package(target.package);
    let dir_name = if package.path.is_empty() {
        session.root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or("main".into())
    } else {
        // `rsplit` yields the whole string when there is no separator, so the
        // last segment is there for any non-empty path — and the branch above
        // is the empty one.
        package.path.rsplit('/').next().unwrap_or(&package.path).to_string()
    };
    let base = output.artifact_name.clone().unwrap_or(dir_name);
    // The catch-all this used to end in would have given a WEB artifact no
    // extension at all. Every JavaScript platform writes an `.mjs`, and a
    // native one writes the bare name, so the match is over the two answers
    // rather than over one platform and everything else.
    let name = if output.platform().is_javascript() { format!("{base}.mjs") } else { base };
    session.root.join(".buri/out").join(output.dir()).join(&package.path).join(name)
}

// ---------------------------------------------------------------------------
// The other two thirds of a page
// ---------------------------------------------------------------------------

/// What a WEB output writes beside its module: the stylesheet, and the entry
/// shell that loads both.
///
/// **A page is three artifacts, and this is where the other two are decided.**
/// Both are pure functions of the module's file name and of the stylesheet, so
/// a cache hit reproduces them byte for byte without recompiling, and
/// `--check-reproducible` comparing them adds no new source of drift.
///
/// The `.mjs` alone is still a complete program — `mount` installs the sheet
/// itself when the document does not already carry one — so what the shell adds
/// is the document, and a stylesheet the browser can fetch and cache on its own
/// rather than one that arrives inside a script. That is why the `<link>`
/// carries `id="buri-styles"`: the runtime's injection looks for exactly that
/// id and finds it, so the rules are in the page once, before the first paint,
/// and the module has nothing to do about them.
///
/// Returns an empty vector for every platform that is not WEB.
pub fn web_companions(module: &Path, output: &Output, stylesheet: &str) -> Vec<(PathBuf, String)> {
    if output.platform() != Platform::Web {
        return Vec::new();
    }
    // `artifact_path` built this name, so it ends in `.mjs` and has a parent.
    let base = module
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| String::from("main"));
    let dir = module.parent().unwrap_or(Path::new("."));
    let mut out = Vec::new();
    // A program with no static styles writes no stylesheet and links none. An
    // empty file would be a request a browser makes for nothing.
    let styled = !stylesheet.is_empty();
    if styled {
        out.push((dir.join(format!("{base}.css")), stylesheet.to_string()));
    }
    let link = if styled {
        format!("  <link id=\"buri-styles\" rel=\"stylesheet\" href=\"{}.css\">\n", escape(&base))
    } else {
        String::new()
    };
    // A module script is deferred by definition, so it runs after the body is
    // parsed and `mount` has somewhere to mount. That is the whole reason the
    // shell needs no load event and no inline code.
    let html = format!(
        "<!doctype html>\n\
         <html lang=\"en\">\n\
         <head>\n\
         \x20 <meta charset=\"utf-8\">\n\
         \x20 <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         \x20 <title>{title}</title>\n\
         {link}</head>\n\
         <body>\n\
         \x20 <script type=\"module\" src=\"./{src}.mjs\"></script>\n\
         </body>\n\
         </html>\n",
        title = escape(&base),
        link = link,
        src = escape(&base),
    );
    out.push((dir.join(format!("{base}.html")), html));
    out
}

/// The five characters that mean something else in markup. The base name comes
/// from a build file's `artifact_name`, which is a string a person writes, so
/// it is escaped rather than trusted.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Writes them, answering whether every one landed. A failure is reported the
/// same way a failure to write the module is, because from a reader's side it
/// is the same mistake about the same directory.
fn write_companions(
    module: &Path,
    output: &Output,
    stylesheet: &str,
    diagnostics: &mut Diagnostics,
) -> bool {
    for (path, text) in web_companions(module, output, stylesheet) {
        if let Err(e) = std::fs::write(&path, &text) {
            diagnostics.push(
                Diagnostic::error(Span::NONE, format!("cannot write {}: {e}", path.display()))
                    .with_fix("check the directory exists and is writable"),
            );
            return false;
        }
    }
    true
}

/// A convenience symlink pointing at the most recent output directory.
fn link_out_symlink(session: &Session, output: &Output) {
    let link = session.root.join("out");
    let target = PathBuf::from(".buri/out").join(output.dir());
    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(&link);
        let _ = std::os::unix::fs::symlink(&target, &link);
    }
    #[cfg(not(unix))]
    {
        let _ = (link, target);
    }
}

/// The build-graph rules that do not need the compiler: visibility, tags, and
/// platforms.
pub fn check_policy(
    session: &Session,
    target: TargetId,
    platform: Platform,
    diagnostics: &mut Diagnostics,
) {
    check_visibility(session, target, diagnostics);
    check_tags(session, target, diagnostics);
    check_platform(session, target, platform, diagnostics);
}

pub fn check_visibility(session: &Session, target: TargetId, diagnostics: &mut Diagnostics) {
    // Production edges are checked across the whole closure: a violation
    // anywhere in it is a reason this target may not be linked. Test edges are
    // checked on the target itself only — a suite is not linked into anything
    // downstream, so a consumer neither depends on that edge nor could fix it,
    // and `//...` reaches every target's own suite anyway.
    let edges = session
        .workspace
        .closure(target)
        .into_iter()
        .map(|m| (m, session.workspace.dep_edges(m)))
        .chain(std::iter::once((target, session.workspace.test_dep_edges(target))));
    for (member, member_edges) in edges {
        for (dep, span) in member_edges {
            let Some(span) = span else { continue };
            if session.workspace.visible(member.package, dep) {
                continue;
            }
            let from = session.workspace.package(member.package).label();
            let to = session.workspace.label(dep);
            let to_path = session.workspace.package(dep.package).path.clone();
            diagnostics.push(
                Diagnostic::templated("visibility-violation", span)
                    .with_bind("from_target", from)
                    .with_bind("to_target", to)
                    .with_bind("visible_to", session.workspace.visibility_list(dep))
                    .with_bind("to_package_path", to_path),
            );
        }
    }
}

/// Two tags that forbid each other may not appear anywhere in the same
/// dependency closure. The path is printed because in a repository of any size
/// the interesting question is never "which library is tagged `server`" but
/// "who dragged it in".
pub fn check_tags(session: &Session, target: TargetId, diagnostics: &mut Diagnostics) {
    // A tag `REPO.buri` does not declare is an error, not a no-op.
    for member in session.workspace.closure(target) {
        for tag in session.workspace.tags(member) {
            if session.workspace.repo.tag(&tag.value).is_none() {
                let known: Vec<&str> =
                    session.workspace.repo.tags.iter().map(|t| t.name.value.as_str()).collect();
                let mut d = Diagnostic::templated("unknown-tag", tag.span)
                    .with_bind("tag", tag.value.as_str());
                // A near miss is a guess about which of the two fixes is meant,
                // not a replacement for saying what to do. Both go in the one
                // `fix`, because a diagnostic carries only one — so the near
                // miss replaces the page's fix rather than joining it.
                if let Some(near) = crate::build::buildfile::nearest(&tag.value, &known) {
                    d = d.with_fix(format!(
                        "did you mean \"{near}\"? — or declare \"{}\" with a `tag` block in REPO.buri",
                        tag.value
                    ));
                }
                diagnostics.push(d);
            }
        }
    }

    let Some((a, a_by, b, b_by)) = session.workspace.forbidden_pair(target) else { return };
    let label = session.workspace.label(target);
    let a_label = session.workspace.label(a_by);
    let b_label = session.workspace.label(b_by);
    let span = session
        .workspace
        .tags(target)
        .iter()
        .map(|t| t.span)
        .next()
        .unwrap_or(Span::point(session.workspace.package(target.package).build_file_id, 0));

    let mut d = Diagnostic::templated("tag-violation", span)
        .with_bind("target", label.as_str())
        .with_bind("first_tag", a.as_str())
        .with_bind("second_tag", b.as_str());
    // Both tags get the same treatment. The introducing edge is what makes
    // this diagnostic useful (TAGS.md:191-203), and printing it for only one
    // of the two leaves the reader to go and find the other by hand — which is
    // exactly the work the note exists to save. A tag the target carries
    // itself has no path to print, and that is the only asymmetry.
    let note_for = |tag: &str, by: TargetId, by_label: &str| -> String {
        let mut note = if by == target {
            format!("\"{tag}\" is carried by {label} itself")
        } else {
            format!("\"{tag}\" is carried by {by_label}")
        };
        if let Some(path) = session.workspace.dep_path(target, by) {
            if path.len() > 1 {
                let names: Vec<String> =
                    path.iter().map(|(t, _)| session.workspace.label(*t)).collect();
                note.push_str(&format!("\n    reached by: {}", names.join(" -> ")));
            }
        }
        note
    };
    let first = note_for(&a, a_by, &a_label);
    let second = note_for(&b, b_by, &b_label);
    d = d.with_note(first);
    d = d.with_note(second);
    // The doc strings are printed because the tag is a policy, and the policy
    // should say why.
    for name in [&a, &b] {
        let doc = session.workspace.tag_doc(name);
        if !doc.is_empty() {
            d = d.with_note(format!("\"{name}\": {doc}"));
        }
    }
    diagnostics.push(d);
}

pub fn check_platform(
    session: &Session,
    target: TargetId,
    platform: Platform,
    diagnostics: &mut Diagnostics,
) {
    let allowed = session.workspace.platforms(target);
    if allowed.contains(&platform) {
        return;
    }
    let label = session.workspace.label(target);
    let span = session
        .workspace
        .package(target.package)
        .build
        .binary
        .as_ref()
        .and_then(|b| b.outputs.iter().find(|o| o.platform() == platform))
        .map(|o| o.span)
        .unwrap_or(Span::point(session.workspace.package(target.package).build_file_id, 0));

    let mut d = Diagnostic::templated("platform-violation", span)
        .with_bind("target", label.as_str())
        .with_bind("platform", platform.slug());
    if let Some((blocker, why)) = session.workspace.platform_blocker(target, platform) {
        d = d.with_note(why);
        if let Some(path) = session.workspace.dep_path(target, blocker) {
            if path.len() > 1 {
                let names: Vec<String> =
                    path.iter().map(|(t, _)| session.workspace.label(*t)).collect();
                d = d.with_note(format!("reached by: {}", names.join(" -> ")));
            }
        }
        for tag in session.workspace.tags(blocker) {
            let doc = session.workspace.tag_doc(&tag.value);
            if !doc.is_empty() {
                d = d.with_note(format!("\"{}\": {doc}", tag.value));
            }
        }
    } else if allowed.is_empty() {
        d = d.with_note("its dependency closure admits no platform at all");
    }
    diagnostics.push(d);
}

/// The outputs a `build` invocation should produce for one target.
pub fn selected_outputs(session: &Session, target: TargetId, flags: &Flags) -> Vec<Output> {
    if target.kind != RuleKind::Binary {
        return Vec::new();
    }
    let Some(bin) = &session.workspace.package(target.package).build.binary else {
        return Vec::new();
    };
    let mut outputs = bin.outputs.clone();
    if outputs.is_empty() {
        // A binary with no declared output still builds for the host, which is
        // what `buri run` needs.
        outputs.push(Output::js(Span::NONE));
    }
    match &flags.output {
        Some(selector) => outputs.into_iter().filter(|o| o.matches_selector(selector)).collect(),
        None => outputs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::buildfile::Arch;

    /// `--check-reproducible`'s red path. A build that is genuinely
    /// irreproducible is hard to arrange in a hermetic system — which is the
    /// point of the system — so what the flag reports when two artifacts
    /// disagree is asserted on the comparison rather than on a build.
    #[test]
    fn two_artifacts_that_disagree_report_where() {
        assert_eq!(first_difference(b"same", b"same"), None);
        assert_eq!(first_difference(b"", b""), None);
        // A byte that moved.
        assert_eq!(first_difference(b"const x=1;", b"const x=2;"), Some(8));
        assert_eq!(first_difference(b"abc", b"xbc"), Some(0));
        // A length that moved: the first byte past the shorter one is where
        // the two stop agreeing, whichever side is shorter.
        assert_eq!(first_difference(b"abc", b"abcd"), Some(3));
        assert_eq!(first_difference(b"abcd", b"abc"), Some(3));
        assert_eq!(first_difference(b"", b"a"), Some(0));
    }

    /// Every field of `BackendOptions` is a term of the `codegen` key.
    ///
    /// `profile` and `target` were always in it. `unit_prefix` is the one that
    /// was not, and it is observable in the object on every ELF target — see the
    /// note on [`codegen_key`] for the measurement. So the claim this states is
    /// the one that makes the key sound: identical IR under two prefixes is two
    /// keys, not one entry that might hold either's bytes.
    #[test]
    fn the_codegen_key_carries_the_unit_prefix() {
        let output = Output::for_platform(Platform::Macos, Span::NONE);
        let flags = Flags::default();
        let key = |prefix: &str| {
            codegen_key(&output, &flags, "llvm", "llvm 21.1.2", prefix, "ir", "layout")
        };
        assert_ne!(key(""), key("lib/money"));
        assert_ne!(key("lib/money"), key("cmd/server"));
        // And it is still a *function* of its inputs: the same prefix twice is
        // the same key, or a batch would relink on every pass.
        assert_eq!(key("lib/money"), key("lib/money"));
        // The term is length-prefixed like every other, so no two prefixes can
        // collide by running into the field beside them.
        assert_ne!(
            codegen_key(&output, &flags, "llvm", "id", "a", "b", "layout"),
            codegen_key(&output, &flags, "llvm", "id", "ab", "", "layout")
        );
    }

    /// A `.buri` filled by a toolchain whose debug backend was a different one
    /// is not reused: the backend's name is a term of the key, so the old
    /// entries are unreachable rather than stale.
    ///
    /// This is what replaces a scheme-version bump. A bump would have to be
    /// remembered on the next swap; the name is in the key on every build.
    #[test]
    fn a_codegen_key_names_the_backend_that_made_it() {
        let output = Output::for_platform(Platform::Macos, Span::NONE);
        let flags = Flags::default();
        let key = |name: &str, identity: &str| {
            codegen_key(&output, &flags, name, identity, "lib/money", "ir", "layout")
        };
        assert_ne!(key("stencil", "id"), key("cranelift", "id"));
        assert_ne!(key("stencil", "id"), key("llvm", "id"));
        assert_ne!(key("stencil", "id"), key("none", "id"));
        // And the identity beside it, so two toolchains with different stencil
        // libraries under one name do not share an entry either.
        assert_ne!(key("stencil", "one"), key("stencil", "two"));
    }

    /// The shared runner file is one file, so two holders of it at once would be
    /// two suites writing one executable and one of them running the other's.
    ///
    /// The claim is what stops that, and the fallback is what stops it from
    /// costing anything: a caller that cannot take the shared file gets the
    /// per-package path every suite used to have, which is correct and slower
    /// rather than refused. Both halves are asserted, and so is the release —
    /// a claim that outlived its holder would turn the fallback into the only
    /// path.
    #[test]
    fn the_shared_runner_is_held_by_one_suite_at_a_time() {
        let dir = std::env::temp_dir().join(format!("buri-runner-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let private = |n: &str| dir.join(n).join("test");

        let first = claim_runner(&dir, private("a"));
        assert_eq!(first.path(), dir.join("test-runner"));
        // A second claim while the first is held is the concurrent case, and it
        // gets its own file rather than the shared one.
        let second = claim_runner(&dir, private("b"));
        assert_eq!(second.path(), private("b"));
        // And a third, so that "the loser falls back" is not "the loser takes
        // the loser's file".
        let third = claim_runner(&dir, private("c"));
        assert_eq!(third.path(), private("c"));

        drop(second);
        drop(third);
        // Released only by the holder: dropping the two that fell back releases
        // nothing, because they took nothing.
        assert_eq!(claim_runner(&dir, private("d")).path(), private("d"));

        drop(first);
        let after = claim_runner(&dir, private("e"));
        assert_eq!(after.path(), dir.join("test-runner"));
        drop(after);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A claim left behind by a process that is not running any more is stolen
    /// rather than waited for, on `cache::Lock`'s argument: one `^C` must not
    /// slow a repository down for good.
    ///
    /// Stated as "how old is old enough" rather than by backdating a file,
    /// because the rule is the comparison and the comparison is what a wrong
    /// bound would get wrong. A bound of zero makes every claim abandoned, which
    /// is the same question asked with a clock this test controls.
    #[test]
    fn an_abandoned_claim_is_taken_back() {
        let dir = std::env::temp_dir().join(format!("buri-runner-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let held = claim_runner(&dir, dir.join("own/test"));
        assert_eq!(held.path(), dir.join("test-runner"));
        // Fresh, so it is somebody's, and this suite gets its own file.
        assert_eq!(claim_runner(&dir, dir.join("other/test")).path(), dir.join("other/test"));
        // Old enough, so it is nobody's and it is taken back.
        let taken = claim_runner_after(&dir, dir.join("other/test"), std::time::Duration::ZERO);
        assert_eq!(taken.path(), dir.join("test-runner"));
        drop(held);
        drop(taken);
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// What a WEB output writes beside its module, and what a JS one does not.
    ///
    /// The `.html` is the whole of what "loadable in a browser as it stands"
    /// means mechanically, so the three things that make it true are asserted
    /// rather than left to a reader of the format string: a module script that
    /// names the module beside it, a stylesheet link carrying the id the
    /// runtime's own injection looks for, and a `<body>` for `mount` to find.
    #[test]
    fn a_web_output_writes_a_stylesheet_and_a_shell() {
        let module = PathBuf::from("/out/web/cmd/counter/counter.mjs");
        let web = Output::for_platform(Platform::Web, Span::NONE);
        let files = web_companions(&module, &web, ".p-r1{padding:1rem}");
        let names: Vec<String> =
            files.iter().map(|(p, _)| p.display().to_string()).collect();
        assert_eq!(
            names,
            vec![
                "/out/web/cmd/counter/counter.css".to_string(),
                "/out/web/cmd/counter/counter.html".to_string(),
            ]
        );
        assert_eq!(files[0].1, ".p-r1{padding:1rem}");
        let html = &files[1].1;
        assert!(html.contains("<script type=\"module\" src=\"./counter.mjs\">"), "{html}");
        assert!(html.contains("<link id=\"buri-styles\" rel=\"stylesheet\" href=\"counter.css\">"), "{html}");
        assert!(html.contains("<body>"), "{html}");

        // No static styles: no file, and nothing linked. An empty stylesheet
        // would be a request a browser makes for nothing.
        let bare = web_companions(&module, &web, "");
        assert_eq!(bare.len(), 1);
        assert!(!bare[0].1.contains("stylesheet"), "{}", bare[0].1);

        // A JS output is one file, as it has always been.
        let js = Output::js(Span::NONE);
        assert!(web_companions(&module, &js, ".p-r1{padding:1rem}").is_empty());
    }

    /// The artifact name reaches the shell's `<title>` and its `src`, and it is
    /// a string a person writes in a build file, so it is escaped.
    #[test]
    fn the_shell_escapes_the_artifact_name() {
        let module = PathBuf::from("/out/web/cmd/x/a<b&c.mjs");
        let web = Output::for_platform(Platform::Web, Span::NONE);
        let files = web_companions(&module, &web, "");
        let html = &files[0].1;
        assert!(html.contains("<title>a&lt;b&amp;c</title>"), "{html}");
        assert!(!html.contains("<title>a<b"), "{html}");
    }

    // -- what a native refusal says -----------------------------------------
    //
    // buri-lang/buri#25 and buri-lang/buri#26 were one sentence serving three
    // causes: "the {platform} backend is not implemented", with a fix reading
    // "this toolchain emits JavaScript; build with `--output=js`". It was false
    // on the two that reach it most — a cross output on a host whose own native
    // build had just succeeded, and `--release` on a toolchain whose debug
    // build of the same output works. These rows are per cause, because the
    // point of the change is that the causes are told apart.

    /// A target that is not this host's, whatever this host is — the same
    /// choice `tests/harness/case.rs` makes, and for the same reason.
    fn cross_target() -> Target {
        let platform = match link::host_platform() {
            Some(Platform::Macos) => Platform::Linux,
            _ => Platform::Macos,
        };
        Target { platform, arch: Some(Arch::X86_64) }
    }

    /// A cross output is refused by the **host**, and the sentence says so —
    /// on every profile, and without claiming anything about backends or about
    /// JavaScript (buri-lang/buri#25).
    #[test]
    fn a_cross_output_is_refused_by_the_host_and_not_by_a_backend() {
        let target = cross_target();
        for profile in [Profile::Debug, Profile::Release] {
            let gap = native_gap(target, profile)
                .unwrap_or_else(|| panic!("{target:?} was not refused in {profile:?}"));
            assert_eq!(gap.output, format!("{}/x86_64", target.platform.slug()));
            assert!(gap.reason.contains("own host only"), "{}", gap.reason);
            assert!(
                gap.fix.contains(&format!("on a {} host", gap.output)),
                "the fix does not name the machine that could build it: {}",
                gap.fix
            );
            assert!(
                !gap.reason.contains("backend"),
                "a host gap blamed a backend: {}",
                gap.reason
            );
        }
    }

    /// The sentence a *recorded* diagnostic holds names no triple and no host,
    /// because both carry the runner's own architecture and a golden that held
    /// one would pin the machine rather than the product.
    ///
    /// `tests/harness/case.rs` says the same thing from the other side, in the
    /// paragraph explaining why there is no `HOST_ARCH` placeholder. This is
    /// the assertion that keeps a future edit from putting one back.
    #[test]
    fn a_recorded_refusal_names_no_triple_and_no_host() {
        for target in [cross_target(), Target { platform: Platform::Macos, arch: None }] {
            for profile in [Profile::Debug, Profile::Release] {
                let Some(gap) = native_gap(target, profile) else { continue };
                for text in [&gap.output, &gap.reason, &gap.fix] {
                    for spelling in ["aarch64", "unknown-linux", "apple-darwin", "-musl"] {
                        assert!(
                            !text.contains(spelling),
                            "a refusal names `{spelling}`, which moves from runner to \
                             runner: {text}"
                        );
                    }
                }
            }
        }
    }

    /// The refusal every site prints is the templated one, so the three sites
    /// cannot word the same gap three ways.
    #[test]
    fn the_refusal_is_one_page_with_the_gap_bound_into_it() {
        let gap = native_gap(cross_target(), Profile::Debug).expect("a cross target is refused");
        let d = no_native_artifact(&gap, Span::NONE);
        assert_eq!(d.code.as_deref(), Some("native-artifact-not-available"));
        assert!(d.message.contains(&gap.output), "{}", d.message);
        assert_eq!(d.notes.first().map(String::as_str), Some(gap.reason.as_str()));
        assert_eq!(d.fix.as_deref(), Some(gap.fix.as_str()));
    }

    /// The `bool` and the sentence are one answer, at every target and both
    /// profiles: a `native_ready` that disagreed with `native_gap` would let a
    /// build start and then refuse it, or refuse one that would have worked.
    #[test]
    fn readiness_is_the_absence_of_a_gap() {
        for platform in [Platform::Macos, Platform::Linux] {
            for arch in [Arch::Arm64, Arch::X86_64] {
                for profile in [Profile::Debug, Profile::Release] {
                    let target = Target { platform, arch: Some(arch) };
                    assert_eq!(
                        native_ready(target, profile),
                        native_gap(target, profile).is_none(),
                        "{platform:?}/{arch:?}/{profile:?}"
                    );
                }
            }
        }
    }

    /// Every JavaScript platform writes an `.mjs`. The catch-all this replaced
    /// would have given a WEB artifact no extension at all.
    #[test]
    fn every_javascript_platform_is_emitted_by_the_js_backend() {
        for platform in Platform::ALL {
            let chosen = backend::select(Target { platform, arch: None }, Profile::Debug);
            assert_eq!(
                chosen.is_ok() && chosen.map(|b| b.name() == "js").unwrap_or(false),
                platform.is_javascript(),
                "`{}` and the js backend disagree about each other",
                platform.proto()
            );
        }
    }
}
