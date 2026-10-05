//! `buri build` and `buri run`.
//!
//! Artifacts land in `.buri/out/<platform>/<package>/<artifact>`, where
//! `<artifact>` is the package's directory name unless the output overrides it
//! with `artifact_name`. Tags are not in the path, because they are not in the
//! cache key: a tag decides whether a build is permitted, never what it
//! produces.

use crate::build::buildfile::{Output, OutputPlatform, PlatformRule};
use crate::build::cache::{hash_bytes, Action, ActionKey, Cache, KeyBuilder};
use crate::build::link;
use crate::build::session::Session;
use crate::build::workspace::{RuleKind, TargetId};
use crate::commands::arguments::Flags;
use crate::compiler::backend::runtime_native;
use crate::compiler::backend::{
    self, Emitted, LinkOptions, Options as BackendOptions, Profile, Target, Units,
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
///
/// A repository platform's `assets` land beside the artifact on every build,
/// cached or not: they are files the platform ships, copied as they are.
pub fn build_target(
    session: &mut Session,
    target: TargetId,
    output: &Output,
    flags: &Flags,
) -> Result<Artifact, Diagnostics> {
    let built = build_artifact(session, target, output, flags)?;
    let mut diagnostics = Diagnostics::new();
    if !copy_assets(session, target, output, &mut diagnostics) {
        return Err(diagnostics);
    }
    Ok(built)
}

/// Every `assets` file of the platform an output names, copied into the
/// output's directory at its own path. Answers whether every one landed.
///
/// A bundled platform's assets are embedded in the toolchain, as its build
/// file is; a repository platform's are read from its package.
fn copy_assets(session: &Session, target: TargetId, output: &Output, diagnostics: &mut Diagnostics) -> bool {
    let Some(rule) = platform_rule(session, output) else { return true };
    let path = artifact_path(session, target, output);
    let Some(into) = path.parent() else { return true };
    for asset in &rule.assets {
        let destination = into.join(&asset.value);
        if let Some(parent) = destination.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let copied = match &output.custom {
            Some(custom) => match session.workspace.platform_rule(&custom.label.value) {
                Some((pid, _)) => std::fs::copy(session.workspace.package(pid).dir.join(&asset.value), &destination)
                    .map(|_| ()),
                None => Ok(()),
            },
            None => match crate::build::platforms::file(output.platform().slug(), &asset.value) {
                Some(text) => std::fs::write(&destination, text),
                None => Ok(()),
            },
        };
        if let Err(e) = copied {
            diagnostics.push(
                Diagnostic::templated("unknown-source", asset.span)
                    .with_bind("source", asset.value.as_str())
                    .with_bind("field", "assets")
                    .with_note(format!("copying it failed: {e}")),
            );
            return false;
        }
    }
    true
}

/// The rule of the platform an output names: a bundled one's, embedded in the
/// toolchain, or a repository platform's.
pub fn platform_rule<'s>(session: &'s Session, output: &Output) -> Option<&'s PlatformRule> {
    match &output.custom {
        Some(custom) => session.workspace.platform_rule(&custom.label.value).map(|(_, rule)| rule),
        None => crate::build::platforms::bundled(output.platform().slug()),
    }
}

/// Whether an output is a page: its platform ships an `index.html` beside
/// every entry, which `buri run` serves instead of starting the entry.
pub fn is_page(session: &Session, output: &Output) -> bool {
    platform_rule(session, output).is_some_and(|rule| rule.assets.iter().any(|a| a.value == PAGE))
}

/// The document `buri run` serves an output's directory by.
pub const PAGE: &str = "index.html";

fn build_artifact(
    session: &mut Session,
    target: TargetId,
    output: &Output,
    flags: &Flags,
) -> Result<Artifact, Diagnostics> {
    let mut diagnostics = Diagnostics::new();
    let platform = output.platform();

    // Every check the graph can answer before a line is compiled.
    check_policy(session, target, &output.output_platform(), &mut diagnostics);
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
            &output.platform_label(),
            &key,
        )
    };
    // One entry holds the module, its stylesheet and its `core/lazy` chunks, so
    // a hit reproduces all of them or is not a hit. A stale `.css` beside a
    // fresh `.mjs` is exactly the failure a cache is supposed to be incapable
    // of, and the module *fetches* its chunks by name at run time: a hit that
    // reproduced the module and not its chunks would be a program that loads
    // and then cannot find half of itself.
    if !flags.force {
        if let Some(parts) = cache.get(&key).and_then(|b| decode_parts(&b)) {
            if let [module, stylesheet, chunks @ ..] = parts.as_slice() {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if std::fs::write(&path, module).is_ok()
                    && write_companions(&path, stylesheet, chunks, &mut diagnostics)
                {
                    explain_link(crate::build::cache::Status::Cached);
                    link_out_symlink(session, output);
                    return Ok(Artifact { target, path, bytes: module.len(), cached: true });
                }
            }
        }
    }
    explain_link(crate::build::cache::Status::Run);

    let compiled = compile_artifact(session, target, output, flags, &mut diagnostics)?;
    let parts = [&compiled.module, &compiled.stylesheet].into_iter().chain(&compiled.chunks);
    cache.put(&key, &encode_parts(parts.map(String::as_str)));
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
    if !write_companions(&path, &compiled.stylesheet, &compiled.chunks, &mut diagnostics) {
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
    /// What `core/lazy`'s `load` split out, in `$lazy`'s own numbering: chunk
    /// `n` is written as `<artifact>.<n>.mjs` beside the module and is fetched
    /// by the module at run time.
    ///
    /// Empty for nearly every program, and always empty for a native output.
    pub chunks: Vec<String>,
}

/// The `link` action itself: sources in, artifact bytes out, and nothing on
/// disk touched either way.
///
/// Split out of `build_target` so that it can be run twice without the cache
/// and without an output directory, which is what `--check-reproducible` needs
/// and what makes "two builds of the same commit produce identical bytes"
/// something the toolchain can be asked rather than something a comment claims.
/// Analyse, find the entry this output names, and monomorphize from it.
///
/// The front end `compile_artifact` and `compile_objects` both run before they
/// part company over a backend. Two copies is two places for "a program is its
/// entry" to be spelled, and they had already drifted over how the missing-
/// entry diagnostic was laid out.
///
/// **Monomorphizing from the named entry is the whole of per-entry dead-code
/// elimination.** `Roots::Main` roots the program at one function, and
/// `middle::dce` walks from the root; so a binary whose page enters at `main`
/// and whose worker enters at `fetch` produces two artifacts, and neither holds
/// what only the other reaches.
fn monomorphized_entry(
    session: &mut Session,
    target: TargetId,
    output: &Output,
    diagnostics: &mut Diagnostics,
) -> Result<(crate::compiler::driver::Analysis, monomorphize::Program), Diagnostics> {
    let platform = output.platform();
    let name = output.entry_name().to_string();
    // `Some`: a build is the per-output check. See `Unit::platform`.
    let unit = Unit {
        target: Some(target),
        platform: Some(platform),
        entry: Some(name.clone()),
        with_tests: false,
    };
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

    let Some(entry) = analysis.checked.entries.get(&name).copied() else {
        diagnostics.push(missing_entry(session, target, output, &analysis.checked));
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

/// "This output enters through a function that is not there."
///
/// Two diagnostics rather than one, because they are two mistakes. A binary
/// with no `main` has not written its entry point yet; an output naming an
/// `entry` that `main.buri` does not export has written the name twice and
/// spelled it differently once, so its page offers what the module does export.
pub fn missing_entry(
    session: &Session,
    target: TargetId,
    output: &Output,
    checked: &crate::compiler::semantics::resolve::Checked,
) -> Diagnostic {
    let package = session.workspace.package(target.package).label();
    let name = output.entry_name();
    if output.entry.is_none() {
        return Diagnostic::templated("missing-main", Span::NONE)
            .with_bind("package", package)
            .with_bind("entry", name);
    }
    let mut exported: Vec<&str> = checked.entries.keys().map(String::as_str).collect();
    exported.sort_unstable();
    let mut d = Diagnostic::templated("unknown-entry-function", output.span)
        .with_bind("entry", name)
        .with_bind("package", package);
    if let Some(near) = crate::build::buildfile::nearest(name, &exported) {
        d = d.with_note(format!("did you mean `{near}`?"));
    }
    if exported.is_empty() {
        d = d.with_note("`main.buri` exports no function at all");
    } else {
        d = d.with_note(format!("`main.buri` exports: {}", exported.join(", ")));
    }
    d
}

pub fn compile_artifact(
    session: &mut Session,
    target: TargetId,
    output: &Output,
    flags: &Flags,
    diagnostics: &mut Diagnostics,
) -> Result<Compiled, Diagnostics> {
    let platform = output.platform();
    let (analysis, mut program) = monomorphized_entry(session, target, output, diagnostics)?;
    // An entry with a `js` file hands itself to the file, which is read and
    // checked here and bundled with the program below.
    let host_file = host_file(session, output, &analysis, diagnostics)?;
    if let Some(file) = &host_file {
        program.hosted.export = Some(output.entry_point().to_string());
        program.hosted.ui = file.exports.ui;
    }
    // The arch is `None` until a native backend has one to vary on: every
    // `Output` carries it and it is already in every key, but nothing below
    // here reads it while the only backend is JavaScript.
    let target = Target { platform, arch: None };
    // Read before `emit`, which takes the program by `&mut`. It is the same
    // text the backend is about to write into the module as `$ui_sheet`, so
    // the `.css` a WEB output writes and the `<style>` `mount` installs are
    // one string produced once.
    let stylesheet = program.stylesheet.clone();
    let (module, chunks) =
        emit_all(&mut program, &analysis.checked.tables, target, flags, diagnostics)?;
    let module = match host_file {
        Some(file) => crate::build::hosted::bundle(&module, &file.text, &file.exports, &file.structs),
        None => module,
    };
    Ok(Compiled { module, stylesheet, chunks })
}

/// An entry's `js` file, read and held to the production structs it
/// implements.
struct HostFile {
    text: String,
    exports: crate::build::hosted::Exports,
    /// The names of the structs it implements.
    structs: Vec<String>,
}

/// The production structs a platform's entry is handed that its `js` file
/// implements: the platform's own, each with the methods it declares without
/// a body. `module` is the platform's `platform.buri`, as it loads.
fn needed_structs(
    analysis: &crate::compiler::driver::Analysis,
    module: &str,
    point: &str,
) -> Vec<crate::build::hosted::Needed> {
    use crate::compiler::semantics::resolve::{declared_host, js_structs, own_fn};
    use crate::build::hosted::{Method, Needed};
    let tables = &analysis.checked.tables;
    let Some(platform) = analysis.loaded.find(module) else { return Vec::new() };
    let decl = own_fn(&analysis.loaded, &analysis.checked.scopes, module, point);
    let Some(host) = decl.and_then(|d| declared_host(tables, platform, &tables.fn_info(d).params)) else {
        return Vec::new();
    };
    let structs = js_structs(tables, platform, host).into_iter().filter(|(_, m)| !m.is_empty());
    // A method of no effect, as `web`'s `IndexedDb.get`, is its struct's to declare.
    let method = |owner: &str, f: &crate::compiler::semantics::types::FnInfo| Method {
        name: f.name.clone(),
        params: f.params.len(),
        effect: f.impl_of.map_or_else(|| owner.to_string(), |(t, _)| tables.trait_(t).name.clone()),
    };
    structs
        .map(|(con, methods)| {
            let name = tables.tycon(con).name.clone();
            let methods = methods.into_iter().map(|f| method(&name, tables.fn_info(f))).collect();
            Needed { name, methods }
        })
        .collect()
}

/// Reads and checks an entry's `js` file: every production struct the entry's
/// host holds of the platform's own is exported with every method, at the
/// right number of parameters. `None` for an entry with no `js` file and no
/// such struct, which starts itself.
///
/// A bundled platform's file is embedded in the toolchain, as its build file
/// is; a repository platform's is read from its package.
fn host_file(
    session: &mut Session,
    output: &Output,
    analysis: &crate::compiler::driver::Analysis,
    diagnostics: &mut Diagnostics,
) -> Result<Option<HostFile>, Diagnostics> {
    if !output.platform().is_javascript() {
        return Ok(None);
    }
    let point = output.entry_point();
    let (module, js) = match &output.custom {
        Some(custom) => (format!("//{}/platform.buri", custom.package_path()), custom.js.clone()),
        None => {
            let name = output.platform().slug();
            let js = crate::build::platforms::bundled(name)
                .and_then(|rule| rule.entries.iter().find(|e| e.name.value == point))
                .and_then(|e| e.js.as_ref().map(|js| js.value.clone()));
            (name.to_string(), js)
        }
    };
    let needed = needed_structs(analysis, &module, point);
    let Some(js) = js else {
        if let Some(first) = needed.first() {
            diagnostics.push(
                Diagnostic::templated("missing-host-file", output.span)
                    .with_bind("entry", point)
                    .with_bind("name", first.name.as_str()),
            );
            return Err(std::mem::take(diagnostics));
        }
        return Ok(None);
    };
    let file = match &output.custom {
        Some(custom) => {
            let Some((pid, _)) = session.workspace.platform_rule(&custom.label.value) else { return Ok(None) };
            let disk = session.workspace.package(pid).dir.join(&js);
            let rel = session.workspace.rel_of(&disk);
            match session.map.load(&rel, &disk) {
                Ok(file) => file,
                Err(e) => {
                    diagnostics.push(
                        Diagnostic::error(output.span, format!("cannot read {rel}: {e}"))
                            .with_fix("check the `js` file the platform's entry names exists"),
                    );
                    return Err(std::mem::take(diagnostics));
                }
            }
        }
        None => {
            let name = output.platform().slug();
            let Some(text) = crate::build::platforms::file(name, &js) else { return Ok(None) };
            session.map.embedded(&format!("{name}/{js}"), text)
        }
    };
    let text = session.map.text(file).to_string();
    let exports = crate::build::hosted::read(&text);
    // `buri:program` holds this artifact's entry and nothing else, and
    // `buri:ui` its two functions. Any other name would arrive `undefined`.
    for (module, name, at) in &exports.named {
        let offered: &[&str] = if *module == "buri:program" { &[point] } else { &["signal", "write"] };
        if !offered.contains(&name.as_str()) {
            diagnostics.push(
                Diagnostic::templated("unknown-export", Span::new(file, *at, *at))
                    .with_bind("path", *module)
                    .with_bind("name", name.as_str())
                    .with_note(format!("`{module}` exports {}", offered.iter().map(|o| format!("`{o}`")).collect::<Vec<_>>().join(" and "))),
            );
        }
    }
    if diagnostics.has_errors() {
        return Err(std::mem::take(diagnostics));
    }
    let gaps = crate::build::hosted::gaps(&exports, &needed);
    for gap in &gaps {
        let mut d = Diagnostic::templated("host-file-missing-method", Span::new(file, gap.at, gap.at))
            .with_bind("file", js.as_str())
            .with_bind("gap", gap.gap.as_str());
        if let Some(note) = &gap.note {
            d = d.with_note(note.clone());
        }
        diagnostics.push(d);
    }
    if !gaps.is_empty() {
        return Err(std::mem::take(diagnostics));
    }
    Ok(Some(HostFile { text, exports, structs: needed.into_iter().map(|n| n.name).collect() }))
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
    let content = if action == Action::Test { Content::Program } else { Content::Bytes };
    action_key_as(session, target, output, flags, action, content)
}

/// [`action_key`], with how the closure's sources are read named.
fn action_key_as(
    session: &Session,
    target: TargetId,
    output: &Output,
    flags: &Flags,
    action: Action,
    content: Content,
) -> ActionKey {
    let mut k = KeyBuilder::new(action, flags.mode);
    k.output(output);
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
    let closure = session.workspace.closure(target);
    for member in &closure {
        contribute_as(session, *member, &mut k, content);
    }
    // Every repository platform the binary's outputs name: its build file, its
    // `platform.buri` and sources, its `js` files and assets, and the
    // libraries it depends on. An edit to any of them is an edit to every
    // output built for it.
    for label in session.workspace.custom_platforms(target) {
        contribute_platform(session, &label, &closure, &mut k);
    }
    k.finish()
}

/// One repository platform's contribution to a key. See [`action_key`].
fn contribute_platform(session: &Session, label: &str, closure: &[TargetId], k: &mut KeyBuilder) {
    let workspace = &session.workspace;
    let Some((pid, files, members)) = platform_inputs(workspace, label) else { return };
    let package = workspace.package(pid);
    k.rule_identity(&package.label(), "platform", &files);
    for rel in &files {
        let disk = package.dir.join(rel);
        k.file(&workspace.rel_of(&disk), std::fs::read(&disk).ok().as_deref());
    }
    for member in members.into_iter().filter(|m| !closure.contains(m)) {
        contribute(session, member, k);
    }
}

/// What a repository platform's outputs are built from beside the binary: its
/// package, its own files package-relative and sorted (`BUILD.buri`,
/// `platform.buri`, sources, `js` files and assets), and every library in its
/// dependencies' closures. One enumeration, so the key and `--watch`'s input
/// set can't disagree.
pub fn platform_inputs(
    workspace: &crate::build::workspace::Workspace,
    label: &str,
) -> Option<(crate::build::workspace::PackageId, Vec<String>, Vec<TargetId>)> {
    let (pid, rule) = workspace.platform_rule(label)?;
    let mut files: Vec<String> = vec![String::from("BUILD.buri"), String::from("platform.buri")];
    files.extend(rule.sources.iter().map(|s| s.value.clone()));
    files.extend(rule.entries.iter().filter_map(|e| e.js.as_ref().map(|j| j.value.clone())));
    files.extend(rule.assets.iter().map(|a| a.value.clone()));
    files.sort();
    files.dedup();
    let mut members: Vec<TargetId> = Vec::new();
    for dep in &rule.dependencies {
        if let Some(t) = workspace.dep_target(&dep.value) {
            members.extend(workspace.closure(t));
        }
    }
    members.sort();
    members.dedup();
    Some((pid, files, members))
}

/// One target's own contribution to a key: its rule identity, and the contents
/// of the sources that rule names. Factored out of `action_key` so it can also
/// be taken alone — which is what `--explain` reports per closure member, and
/// what makes "editing this file changed this target's key and not that one"
/// something a test can watch rather than something a comment asserts.
fn contribute(session: &Session, member: TargetId, k: &mut KeyBuilder) {
    contribute_as(session, member, k, Content::Bytes);
}

/// How a key reads a source file.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Content {
    /// Every byte. An artifact's key, where a comment edit may move a debug location.
    Bytes,
    /// What the compiler sees ([`crate::parsing::lexer::program_text`]). A test
    /// verdict's key, so a comment or whitespace edit reuses the cached pass.
    Program,
}

/// A file's contents as `content` reads them, tagged so the two readings never meet.
pub fn read_as(rel: &str, bytes: Vec<u8>, content: Content) -> Vec<u8> {
    if content == Content::Program && rel.ends_with(".buri") {
        if let Some(mut text) =
            std::str::from_utf8(&bytes).ok().and_then(crate::parsing::lexer::program_text)
        {
            text.insert(0, b'p');
            return text;
        }
    }
    let mut out = Vec::with_capacity(bytes.len().saturating_add(1));
    out.push(b'b');
    out.extend_from_slice(&bytes);
    out
}

fn contribute_as(session: &Session, member: TargetId, k: &mut KeyBuilder, content: Content) {
    let package = session.workspace.package(member.package);
    let kind = member.kind.name();
    let sources = rule_files(&session.workspace, member);
    k.rule_identity(&package.label(), kind, &sources);
    // What a generator produced, rather than only what it was given. The
    // inputs above catch an edit to a declared file; this catches everything
    // else that decides the modules this rule is compiled from — the tool's own
    // sources above all, which are in no list here and are what a generator
    // *is*. Without it, editing the tool left every dependent's `link` key
    // where it was and the cache served the old artifact.
    for module in crate::build::generators::modules_of(&session.workspace, member) {
        k.input(&format!("{}/{}", package.label(), module.name), module.text.as_bytes());
    }
    // Read in parallel, hashed in order. A key is a fold over the sources in
    // sorted order and that fold stays exactly where it was, on this thread; a
    // library of three hundred and sixty files is three hundred and sixty
    // `open`/`read`/`close` round trips, and those are what the cores are idle
    // for. `parallel::map` returns in index order, so the bytes reach the
    // builder in the order `sources` is in.
    let contents: Vec<Option<Vec<u8>>> = crate::parallel::map(sources.len(), |i| {
        let rel = sources.get(i)?;
        let bytes = std::fs::read(package.dir.join(rel)).ok()?;
        Some(match content {
            Content::Bytes => bytes,
            Content::Program => read_as(rel, bytes, content),
        })
    });
    for (rel, contents) in sources.iter().zip(&contents) {
        k.file(&session.workspace.rel_of(&package.dir.join(rel)), contents.as_deref());
    }
}

/// Every file one rule names, package-relative and sorted: its entry module,
/// its `sources`, its `testing` sources, and its generators' inputs.
///
/// One enumeration, because two answers are built from it and must agree: a
/// build's action key, and the closure a remembered lint answer or a kept
/// language-server analysis is checked against
/// ([`crate::build::sources::closure_of`]). A file in one list and not the
/// other is a file whose edit one of them does not see.
///
/// A generator's input is in it like any other source: the modules it
/// becomes are a function of its bytes, so editing one moves the key exactly
/// as editing a source does.
pub fn rule_files(workspace: &crate::build::workspace::Workspace, member: TargetId) -> Vec<String> {
    let package = workspace.package(member.package);
    let entry = match member.kind {
        RuleKind::Library => "lib.buri",
        RuleKind::Binary => "main.buri",
        RuleKind::Tool => "tool.buri",
    };
    let mut files: Vec<String> = vec![entry.to_string()];
    match member.kind {
        RuleKind::Library => {
            if let Some(lib) = &package.build.library {
                files.extend(lib.sources.iter().map(|x| x.value.clone()));
                if let Some(testing) = &lib.testing {
                    files.push("testing/lib.buri".into());
                    files.extend(testing.sources.iter().map(|x| x.value.clone()));
                }
            }
        }
        RuleKind::Binary => {
            if let Some(bin) = &package.build.binary {
                files.extend(bin.sources.iter().map(|x| x.value.clone()));
            }
        }
        RuleKind::Tool => {
            if let Some(tool) = &package.build.tool {
                files.extend(tool.sources.iter().map(|x| x.value.clone()));
            }
        }
    }
    files.extend(crate::build::generators::inputs(workspace, member));
    files.sort();
    files
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
    k.output(output);
    contribute(session, target, &mut k);
    k.finish()
}

/// Reports every action a build of `target` involves, deepest first: one
/// `generate` line per rule that declares a generator, one `compile` line per
/// closure member, then the `link` that consumed them.
fn explain_closure(session: &Session, target: TargetId, output: &Output, flags: &Flags) {
    if !flags.explain {
        return;
    }
    for member in session.workspace.closure(target) {
        if !crate::build::generators::declared(&session.workspace, member).is_empty() {
            crate::build::cache::explain(
                true,
                crate::build::cache::Status::Keyed,
                Action::Generate,
                &session.workspace.label(member),
                &output.platform_label(),
                &crate::build::generators::rule_key(session, member, output, flags),
            );
        }
        let key = compile_key(session, member, output, flags);
        crate::build::cache::explain(
            true,
            crate::build::cache::Status::Keyed,
            Action::Compile,
            &session.workspace.label(member),
            &output.platform_label(),
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
    let mut k = suite_key(session, target, output, flags, Action::Test, Content::Program);
    goldens(&session.workspace.package(target.package).dir, &mut k);
    // A recording run and a comparing run are two kinds of result and must not
    // share a cache entry. `--update` paints goldens and never compares, so its
    // verdict is always "passed" — it proves a file was written, never that the
    // golden on disk is what a comparing run would paint now. Folding the flag in
    // gives the two runs separate keys, so a recording run neither is served a
    // comparing run's verdict nor writes one a later comparing run is served in
    // place of actually comparing. The `served` gate already keeps `--update`
    // from *reading* the cache at all; this is what keeps what it *writes* from
    // standing in for a comparison (buri-lang/buri#174).
    if flags.update {
        k.input("update", b"1");
    }
    k.finish()
}

/// The key a suite's build is filed under: the test binary it linked, the
/// JavaScript bundle it emitted, the errors that stopped it, or the fact that
/// it had no test to run.
///
/// Known before the front end runs, so a warm run of a failing suite starts its
/// binary or bundle again without checking, monomorphizing, emitting or
/// linking anything. It is [`test_key`]'s closure, read as the program text
/// too, so a comment or whitespace edit finds the record and builds nothing.
/// Such an edit can move the lines a recorded error or a failing test is
/// reported at, so the record writes its spans relative to the tokens around
/// them and reads them back against the edited files (`commands/test.rs`'s
/// `Anchors`). The binary or bundle it names is the one the old bytes built:
/// it differs from what the new bytes would build only in debug locations,
/// which no test prints.
///
/// It differs from [`test_key`] in two ways, each a difference between what a
/// binary depends on and what a verdict does:
///
/// - **No goldens and no `--update`.** They decide what the run does, and a
///   failing suite is run every time. The binary is the same either way.
/// - **The build graph, the `--filter` and the linker.** The graph is every
///   build file's bytes ([`graph_key`]), so a recorded error can't outlive an
///   edit to one. A filtered binary holds only the tests the filter selects.
///   And a binary is what one particular linker produced.
pub fn test_build_key(
    session: &Session,
    target: TargetId,
    output: &Output,
    flags: &Flags,
    graph: &ActionKey,
) -> ActionKey {
    let mut k = suite_key(session, target, output, flags, Action::Build, Content::Program);
    k.dependency(graph);
    if let Some(filter) = &flags.filter {
        k.input("filter", filter.as_bytes());
    }
    if target_of(output).platform.is_native() {
        if let Ok(linker) = link::select(target_of(output)) {
            let identity = linker.identity();
            k.linker(&identity.name, &identity.version);
            k.input("libc", identity.link.as_bytes());
        }
    }
    k.finish()
}

/// Every build file's bytes: `REPO.buri`, then each package's `BUILD.buri` in
/// the graph's order. See [`test_build_key`].
pub fn graph_key(session: &Session, flags: &Flags) -> ActionKey {
    let workspace = &session.workspace;
    let mut k = KeyBuilder::new(Action::Build, flags.mode);
    k.file("REPO.buri", std::fs::read(workspace.root.join("REPO.buri")).ok().as_deref());
    for package in &workspace.packages {
        let file = package.dir.join("BUILD.buri");
        k.file(&workspace.rel_of(&file), std::fs::read(&file).ok().as_deref());
    }
    k.finish()
}

/// The closure [`test_key`] and [`test_build_key`] share: the target's, each
/// test dependency's, and the suite's own sources, read as `content` says.
fn suite_key(
    session: &Session,
    target: TargetId,
    output: &Output,
    flags: &Flags,
    action: Action,
    content: Content,
) -> KeyBuilder {
    let base = action_key_as(session, target, output, flags, action, content);
    let mut k = KeyBuilder::new(action, flags.mode);
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
        contribute_as(session, member, &mut k, content);
    }
    let package = session.workspace.package(target.package);
    if let Some(suite) = package.test_suite(target.kind) {
        let mut files: Vec<String> =
            suite.sources.iter().map(|x| x.value.clone()).collect();
        files.sort();
        k.rule_identity(&package.label(), "test", &files);
        for rel in &files {
            let full = package.dir.join(rel);
            let contents = std::fs::read(&full).ok().map(|b| read_as(rel, b, content));
            k.file(rel, contents.as_deref());
        }
    }
    k
}

/// Every golden in the package's `test/__snapshots__`, which a snapshot
/// compares against (`cli/runtime/snapshot.rs`), so a suite's verdict depends
/// on them as much as on its sources.
///
/// The whole directory rather than the goldens this suite reads: which names a
/// suite snapshots is known only once it runs. A golden is `<name>.png`. A
/// `<name>.diff.png` is not one: the runtime writes it beside a changed
/// snapshot and removes it on a pass, so keying it would move the key under
/// the run that stores it. Sorted, so the key does not depend on the order the
/// directory lists in. Its being absent adds nothing, which is what a package
/// with no goldens has always keyed.
fn goldens(package_dir: &std::path::Path, k: &mut KeyBuilder) {
    for rel in golden_files(package_dir) {
        k.file(&rel, std::fs::read(package_dir.join(&rel)).ok().as_deref());
    }
}

/// The goldens [`goldens`] keys, package-relative and sorted. `--watch`
/// watches the same list.
pub fn golden_files(package_dir: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(package_dir.join("test").join("__snapshots__")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".png") && !n.ends_with(".diff.png"))
        .map(|n| format!("test/__snapshots__/{n}"))
        .collect();
    names.sort();
    names
}

/// The middle end and then a backend, over one monomorphized program.
///
/// This is the seam the native backends arrive at: everything above it is
/// shared, everything below it is [`backend::select`]'s answer. The one thing
/// that is still JavaScript-shaped is the return type — a `String`, because a
/// JavaScript artifact is text. A native artifact is bytes, and it is the
/// `link` step rather than this function that will produce it.
pub fn emit(
    program: &mut monomorphize::Program,
    tables: &crate::compiler::semantics::types::Tables,
    target: Target,
    flags: &Flags,
    diagnostics: &mut Diagnostics,
) -> Result<String, Diagnostics> {
    emit_all(program, tables, target, flags, diagnostics).map(|(module, _)| module)
}

/// The middle end and then the JavaScript backend, for a test suite run on
/// JavaScript: the module as text, because `buri test` appends its driver to
/// it before running it.
///
/// Unminified ([`Js::emit_unminified`](crate::compiler::backend::js::Js::emit_unminified)):
/// the bundle is run once and thrown away, so the minifier's passes are
/// seconds spent on bytes nobody keeps.
pub fn emit_test_bundle(
    program: &mut monomorphize::Program,
    tables: &crate::compiler::semantics::types::Tables,
    flags: &Flags,
    diagnostics: &mut Diagnostics,
) -> Result<String, Diagnostics> {
    prepare(program, Target { platform: crate::build::buildfile::Platform::Js, arch: None });
    let emitting = crate::profile::enter(crate::profile::Phase::Emit);
    let units = match crate::compiler::backend::js::Js.emit_unminified(
        program,
        tables,
        profile_of(flags),
    ) {
        Ok(units) => units,
        Err(errors) => {
            diagnostics.extend(errors.items);
            return Err(std::mem::take(diagnostics));
        }
    };
    drop(emitting);
    match units.into_iter().next().map(|unit| String::from_utf8(unit.bytes)) {
        Some(Ok(module)) => Ok(module),
        _ => {
            diagnostics.push(Diagnostic::error(
                Span::NONE,
                String::from("internal error: the backend emitted no text module"),
            ));
            Err(std::mem::take(diagnostics))
        }
    }
}

/// [`emit`], and the chunks `core/lazy` split out beside the module.
///
/// Two functions rather than one because the chunks are a build-system
/// concern — they are files, written and cached beside the artifact — and every
/// caller that only wants a program to run wants [`emit`].
pub fn emit_all(
    program: &mut monomorphize::Program,
    tables: &crate::compiler::semantics::types::Tables,
    target: Target,
    flags: &Flags,
    diagnostics: &mut Diagnostics,
) -> Result<(String, Vec<String>), Diagnostics> {
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
                output: target.platform.machine().to_string(),
                reason: "this toolchain has no backend for it".to_string(),
                fix: "add `{ platform: \"node\" }` to `outputs`".to_string(),
            });
            diagnostics.push(no_native_artifact(&gap, Span::NONE));
            return Err(std::mem::take(diagnostics));
        }
    };
    let opts = BackendOptions { profile, target, unit_prefix: "" };
    let emitting = crate::profile::enter(crate::profile::Phase::Emit);
    let units = match backend.emit(program, tables, &opts) {
        Ok(units) => units,
        Err(errors) => {
            diagnostics.extend(errors.items);
            return Err(std::mem::take(diagnostics));
        }
    };
    drop(emitting);
    // Unit zero is the module; anything after it is a `core/lazy` chunk, in
    // `$lazy`'s own numbering. The vector is the shape because a native build
    // emits one object per codegen unit; taking element zero here is the
    // JavaScript artifact's whole link step.
    let mut text = Vec::new();
    for unit in units {
        match String::from_utf8(unit.bytes) {
            Ok(source) => text.push(source),
            Err(_) => {
                diagnostics.push(Diagnostic::error(
                    Span::NONE,
                    String::from("internal error: the backend emitted bytes that are not text"),
                ));
                return Err(std::mem::take(diagnostics));
            }
        }
    }
    if text.is_empty() {
        diagnostics.push(Diagnostic::error(
            Span::NONE,
            String::from("internal error: the backend emitted no codegen unit"),
        ));
        return Err(std::mem::take(diagnostics));
    }
    let chunks = text.split_off(1);
    match text.into_iter().next() {
        Some(module) => Ok((module, chunks)),
        None => {
            diagnostics.push(Diagnostic::error(
                Span::NONE,
                String::from("internal error: the backend emitted no codegen unit"),
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
/// [`emit`], [`emit_test_bundle`] and [`compile_objects`] all call this, and
/// none of them decides anything else about the middle end.
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
    // A chunk is a second file beside the artifact, so only a target that
    // writes files can have one. `middle::chunks` takes `core/lazy`'s `load`
    // back out everywhere else, which is the identity the module promises.
    let _phase = crate::profile::enter(crate::profile::Phase::Middle);
    let opts = middle::Options {
        split_lazy: !target.platform.is_native(),
        ..middle::Options::default()
    };
    middle::run(program, &opts);
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
/// target is untouched, and so is a batch's: `link_test_binary` gives every
/// test binary the same empty prefix. Adding the term does invalidate every existing
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
    k.output(output);
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
    linker: &link::CDriver,
    unit_keys: &[ActionKey],
    runtime: link::RuntimeArchive,
) -> ActionKey {
    link_key_of(flags.mode, target_of(output), &linker.identity(), unit_keys, runtime)
}

/// [`link_key`] without a repository, which is what makes the three claims it
/// rests on testable as claims rather than as the shadow of a build: that the
/// unit keys enter **in order**, that the linker's identity enters at all, and
/// that the archive decision moves the key exactly when it moves.
pub fn link_key_of(
    mode: crate::commands::arguments::BuildMode,
    target: Target,
    linker: &link::LinkerIdentity,
    unit_keys: &[ActionKey],
    runtime: link::RuntimeArchive,
) -> ActionKey {
    let mut k = KeyBuilder::new(Action::Link, mode);
    k.platform(target.platform, target.arch);
    k.linker(&linker.name, &linker.version);
    // *How* the link runs, beside *who* runs it. The linker's banner does not
    // move when a toolchain gains a musl sysroot and starts linking
    // `-static-pie` against it, and the artifact is a different file — so
    // without this term the rebuilt toolchain is served the old one's
    // executable. See [`link::CDriver::link_identity`].
    k.input("libc", linker.link.as_bytes());
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
pub(crate) fn runtime_archive_hash() -> &'static str {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(runtime_native::archive_hash)
}

/// The `net`/`crypto` a link for `target` can answer, which is the archive it
/// will link against: the baked host archive for a host target, and the cross
/// archive `runtime_cross` builds for a cross one.
///
/// This is what makes a cross `net` program refuse *by name* at compile time
/// rather than at the system linker: the first cross archive is net-off and
/// crypto-off (`ring` cannot cross from a bare macOS host), so a cross build of
/// a program that reaches networking is told which operations its target's
/// runtime has no body for. A pure function of the toolchain and the target — it
/// reads the feature *decision*, not a built archive, so it costs nothing during
/// codegen.
fn runtime_features_for(target: Target) -> (bool, bool) {
    if link::is_host_target(target) {
        (runtime_native::net(), runtime_native::crypto())
    } else {
        crate::build::runtime_cross::features_for(target)
    }
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
        Some(arch) => format!("{}-{}", target.platform.machine(), arch.slug()),
        None => target.platform.machine().to_string(),
    };
    let js = "add `{ platform: \"node\" }` to `outputs`";
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
    Diagnostic::templated("native-artifact-unavailable", span)
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
/// their values' types, and in the structural operations that take one. But the
/// set a backend can ask `Layouts::of` about is **larger** than that, and the
/// difference is what buri-lang/buri#196 was: a generic intrinsic's body reads
/// the layout of a type its lowered signature has *erased*. `alloc.copyOut`'s
/// signature is `[Carried<T>]`, a list whose shallow `Layout` is `{ ptr, len }`
/// whatever `T` is — so a change to `T`'s shape moved neither the rendered IR
/// (the body is `= runtime`) nor a one-level layout of the type the unit names.
/// The backend recovers `T` from the signature and emits a copy walk sized to
/// it, so the *bytes* moved while the *key* did not, and the incremental build
/// served a copy stencil for the old shape — a heap corruption a cold build
/// never produced.
///
/// So the layout term is the **transitive closure** of every type a named type
/// reaches: its type arguments, an array's element, a tuple's or a closure's
/// parts, and a struct's or enum's field types. That is the whole set a unit's
/// glue can be sized from, reached across the pointers and erased carriers a
/// one-level `Layout` stops at. An inlined field was already caught by the
/// owner's own computed layout; a field behind a pointer, a list element, or an
/// erased carrier's element is caught now. See [`layout_closure_signature`].
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
            let seeds =
                types.iter().filter_map(|id| program.types.get(*id)).map(|info| info.ty);
            let shapes = layout_closure_signature(layouts, tables, seeds);
            (name, hash_bytes(text.as_bytes()), hash_bytes(shapes.as_bytes()))
        },
    )
}

/// The layout term for one unit: the computed layout of every type in the
/// transitive closure of the types the unit names, rendered as sorted text.
///
/// Seeded with the aggregates a unit's functions name ([`collect_types`]) and
/// then walked through every type each of those reaches — type arguments, an
/// array's element, a tuple's or a closure's parts, and a struct's fields or an
/// enum's variants' fields. A one-level `Layout` stops at a pointer, a list's
/// `{ ptr, len }`, or an erased carrier, so a type whose shape a unit's glue is
/// sized from — but which the unit reaches only across one of those boundaries —
/// would otherwise be absent from the key (buri-lang/buri#196). The closure
/// reaches it, so a change to its shape moves the layout hash and the unit is
/// rebuilt rather than served stale.
///
/// A `HashSet` of the visited `Ty` bounds the walk: a recursive type (a `Tree`
/// whose `Node` holds a `Tree`) is entered once. Each type is rendered by
/// `types::show`, which is derived from the type and not from its interning
/// index, so — like the sort being on text — the string moves only when a
/// layout it names moves.
fn layout_closure_signature(
    layouts: &mut layout::Layouts,
    tables: &Tables,
    seeds: impl Iterator<Item = crate::compiler::semantics::types::Ty>,
) -> String {
    use crate::compiler::semantics::types::{self, Ty, TyKind};

    let mut seen: std::collections::HashSet<Ty> = std::collections::HashSet::new();
    let mut stack: Vec<Ty> = seeds.collect();
    let mut lines: Vec<String> = Vec::new();
    while let Some(ty) = stack.pop() {
        // Only the shaped types have a layout worth folding in; a bare
        // parameter or an inference variable has none and cannot be reached in
        // a monomorphized program anyway.
        if matches!(ty.kind(), TyKind::Var(_) | TyKind::Param(_) | TyKind::SelfTy | TyKind::Error) {
            continue;
        }
        if !seen.insert(ty) {
            continue;
        }
        lines.push(format!("{} {:?}\n", types::show(tables, None, &[], &ty), layouts.of(ty)));
        // The types this one reaches: its arguments and its constituent parts,
        // then — for a nominal type — the types of its fields or its variants'
        // fields, which is where a shape reached only across a pointer lives.
        match ty.kind() {
            TyKind::Con(id, args) => {
                stack.extend(args.iter().cloned());
                stack.extend(types::field_types(tables, &ty));
                let variants = tables.tycon(*id).variants().len();
                for v in 0..variants {
                    stack.extend(types::variant_types(tables, &ty, v));
                }
            }
            TyKind::Array(el) => stack.push(*el),
            TyKind::Tuple(els) => stack.extend(els.iter().cloned()),
            TyKind::Fn(ps, r) => {
                stack.extend(ps.iter().cloned());
                stack.push(*r);
            }
            TyKind::Ctx(_) => stack.extend(types::field_types(tables, &ty)),
            _ => {}
        }
    }
    lines.sort();
    lines.concat()
}

/// The unit that carries a test binary's entry point, and a signature of the
/// test set that entry enumerates.
///
/// A test binary has no `main`. The backend synthesises an entry — the program
/// entry, the `test$N` thread doors, and the calls between them — from the
/// ordered set of test roots, and emits the whole of it into the unit that
/// holds the *first* test's function (`backend::stencil`'s `Root::Tests` arm).
/// None of that is an `ir::Func`, so [`unit_hashes`] — which renders a unit's
/// functions — cannot see it: the entry unit's object depends on which tests
/// exist, and its key did not. Adding or removing a test whose function lives
/// in *another* unit left this unit's key unchanged, so the cache served its
/// stale object — an entry still calling a `test$N` that no longer exists — and
/// the link failed on the undefined symbol until `buri clean` cleared it, which
/// `--force` did not (buri-lang/buri#175).
///
/// The remedy is to fold this signature into that one unit's `codegen` key: the
/// ordered `(module, name)` of every test root — which is what the entry's
/// calls are mangled from — and whether values cross tasks, which decides the
/// entry shim's marking. Only the entry unit depends on the set, so only its
/// key carries the term; every other unit keeps the membership-independent key
/// that lets a batch reuse it (`native_test_batch`).
///
/// `None` for a `main` program, whose entry *is* a rendered function and moves
/// with it, and for a test program the middle end rooted at nothing.
fn test_entry_signature(
    program: &monomorphize::Program,
    lowered: &ir::Program,
) -> Option<(usize, String)> {
    let monomorphize::ProgramRoots::Tests(tests) = &program.roots else {
        return None;
    };
    let unit = lowered.funcs.get(tests.first()?.func.index())?.unit as usize;
    let mut sig = String::new();
    for t in tests {
        sig.push_str(&t.module);
        sig.push('\0');
        sig.push_str(&t.name);
        sig.push('\n');
    }
    if lowered.crosses_tasks {
        sig.push_str("crosses-tasks\n");
    }
    Some((unit, sig))
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
/// The `Emitted::key` a backend attaches, if it attaches one, is *replaced* by
/// the one the build system computed. That is not a slight: the cache is the build system's, and
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
            out.push((Emitted { name: object_name(name), key: Some(key.clone()), bytes }, true));
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
            out.push((Emitted { name: object_name(name), key: Some(key.clone()), bytes }, true));
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
            Emitted { name: wanted, key: Some(key.clone()), bytes: unit.bytes.clone() },
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
    let (analysis, mut program) = monomorphized_entry(session, target, output, diagnostics)?;
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
    objects_named(&session.root, &prefix, &label, output, flags, program, tables, diagnostics)
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
    root: &std::path::Path,
    prefix: &str,
    label: &str,
    output: &Output,
    flags: &Flags,
    program: &mut monomorphize::Program,
    tables: &Tables,
    diagnostics: &mut Diagnostics,
) -> Result<Objects, Diagnostics> {
    let profile = profile_of(flags);
    let back_target = target_of(output);
    // The same composition `emit` runs, from the same function: this path
    // reaches a native backend and that one reaches JavaScript, and which
    // passes a target gets must not be a fact stated twice.
    let plan = prepare(program, back_target);
    // Lowered here for the unit *keys*, and then handed to the backend through
    // [`Backend::adopt_lowering`] so that the bytes are emitted from this same
    // IR instead of from a second copy of it. `middle::lower` is deterministic
    // and a pure function of the program, so the two agreed by construction —
    // and agreeing by construction is what made recomputing it pure waste. It
    // was one second of an eight-second `buri test //...` on a real repository:
    // `middle::rc::analyze` twice over the whole program and
    // `middle::lower::run_with` twice.
    //
    // This is a *hint*, not a second entry point: emission is still
    // `emit_units` and it still takes the `Program`. Both native backends take
    // it; a backend that ignores it compiles the same bytes it always did.
    //
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
    // The **target's** runtime capabilities, which are the host's for a host
    // build and the cross archive's for a cross one. `missing_intrinsics` folds
    // in the host's own gaps (the seam it has always had), and a cross target is
    // against an archive with fewer features — net-off and crypto-off on the
    // first one (`runtime_cross`) — so its gaps are folded in here, where the
    // target is known. On a host build `net`/`crypto` are the host's, both these
    // additions are empty, and the behaviour is byte-identical.
    let (net, crypto) = runtime_features_for(back_target);
    let mut missing = backend.missing_intrinsics(program, tables);
    missing.extend(backend::networking_gap_when(program, net));
    missing.extend(backend::cryptography_gap_when(program, crypto));
    missing.sort();
    missing.dedup();
    if !missing.is_empty() {
        // One diagnostic per cause rather than one per program: an operation a
        // toolchain built without the runtime's `net` feature cannot answer is
        // a different sentence, and asks for a different thing, from an
        // operation the backend has no body for. The split reads the target's
        // own answer, not the baked one, so a cross `net` program is named here
        // rather than at the system linker.
        let (networking, rest) = backend::split_networking_when(&missing, net);
        let (cryptography, rest) = backend::split_cryptography_when(&rest, crypto);
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
    // The one unit whose object depends on the set of tests rather than on the
    // functions it renders — the entry point the backend synthesises. Folding
    // the set into its `ir` term is what relinks a test binary from scratch when
    // a test is added or removed, instead of serving a stale entry that names a
    // `test$N` no longer defined ([`test_entry_signature`], buri-lang/buri#175).
    let entry = test_entry_signature(program, &lowered);
    // Every unit's forks read `buri_rt_shared_mask` in a program that can fan
    // out, so the answer is in every unit's key (buri-lang/buri#243).
    let shares = backend.forks_read_shared_mask()
        && crate::compiler::backend::runtime_table::shares_counts(&lowered);
    let keys: Vec<(String, ActionKey)> = unit_hashes(&lowered, tables)
        .into_iter()
        .enumerate()
        .map(|(u, (unit, ir_hash, layout_hash))| {
            let ir_hash = match &entry {
                Some((entry_unit, sig)) if *entry_unit == u => {
                    hash_bytes(format!("{ir_hash}\n{sig}").as_bytes())
                }
                _ => ir_hash,
            };
            let ir_hash =
                if shares { hash_bytes(format!("{ir_hash}\nshares-counts").as_bytes()) } else { ir_hash };
            let key = codegen_key(output, flags, &name, &identity, prefix, &ir_hash, &layout_hash);
            (unit, key)
        })
        .collect();

    // Moved into the emission below, which is the last reader of it: the keys
    // are computed and `unit_hashes` has already borrowed it.
    let mut lowered_for_backend = Some(lowered);
    let cache = Cache::open(root);
    let emitted = codegen_units_for(&cache, &keys, flags.force, |wanted| {
        let opts = BackendOptions { profile, target: back_target, unit_prefix: prefix };
        // Taken once: `codegen_units_for` runs this at most once, and a backend
        // offered nothing lowers for itself.
        if let Some(lowered) = lowered_for_backend.take() {
            backend.adopt_lowering(lowered);
        }
        // `Units::Only` is a membership test per unit, so a build that wants
        // every unit — a first build, or `--force` — says so rather than
        // scanning a list of every unit once per unit.
        let selection =
            if wanted.len() == keys.len() { Units::All } else { Units::Only(wanted) };
        let _phase = crate::profile::enter(crate::profile::Phase::Emit);
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
            &output.platform_label(),
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
    let path = artifact_path(session, target, output);
    let Some(linker) = linker_for(output, &mut diagnostics) else { return Err(diagnostics) };

    explain_closure(session, target, output, flags);
    let objects = compile_objects(session, target, output, flags, &mut diagnostics)?;
    let label = session.workspace.label(target);
    let prefix = session.workspace.package(target.package).path.clone();
    let hit = match link_cached(&session.root, &label, output, flags, linker, &objects, &path, &prefix) {
        Ok((_, hit)) => hit,
        Err(errors) => {
            diagnostics.extend(errors.items);
            return Err(diagnostics);
        }
    };
    let size = match hit.map_or_else(|| std::fs::metadata(&path).map(|m| m.len()), Ok) {
        Ok(size) => size,
        Err(e) => {
            diagnostics.push(Diagnostic::error(
                Span::NONE,
                format!("the link produced no {}: {e}", path.display()),
            ));
            return Err(diagnostics);
        }
    };
    link_out_symlink(session, output);
    Ok(Artifact { target, path, bytes: usize::try_from(size).unwrap_or(usize::MAX), cached: hit.is_some() })
}

/// The link step with the cache in front of it: the executable `objects` link
/// into, at `path`, and the `link` key it is filed under. `Some(size)` when the
/// cache had it and `None` when the link ran; `Err` holds the linker's errors
/// alone.
#[allow(
    clippy::too_many_arguments,
    reason = "where the cache is, the label, the output, the flags, the linker, the objects, \
              where the executable goes and the unit prefix: none derivable from another"
)]
fn link_cached(
    root: &std::path::Path,
    label: &str,
    output: &Output,
    flags: &Flags,
    linker: link::CDriver,
    objects: &Objects,
    path: &std::path::Path,
    prefix: &str,
) -> Result<(ActionKey, Option<u64>), Diagnostics> {
    // Asked here and again inside the linker, of the same objects, because it
    // is a pure function of them: the key has to name the command line the link
    // is about to run, and the linker has to build that command line.
    let runtime = link::runtime_archive_for(&objects.units);
    let key = link_key(output, flags, &linker, &objects.keys, runtime);
    let cache = Cache::open(root);
    let linker = linker.in_dir(link::dir(root, key.as_str())).from_cache(cache.clone());
    let explain_link = |status: crate::build::cache::Status| {
        crate::build::cache::explain(flags.explain, status, Action::Link, label, &output.platform_label(), &key);
    };
    // "The fastest link is the one that does not run": every unit's key
    // unchanged means the ordered list in `key` is unchanged, so the executable
    // in the cache is the executable this link would produce.
    if !flags.force {
        if let Some(entry) = cache.entry(&key) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(size) = write_executable(&entry, path) {
                explain_link(crate::build::cache::Status::Cached);
                return Ok((key, Some(size)));
            }
        }
    }
    explain_link(crate::build::cache::Status::Run);
    let opts = LinkOptions { profile: profile_of(flags), target: target_of(output), unit_prefix: prefix };
    let staged = link::run(&objects.units, &objects.rows, &linker, path, &opts)?;
    // The linker's own output, moved into the entry rather than a second copy
    // of it written from a full read of the artifact just placed. See
    // `Cache::put_file`. The copy at `path` is the one that runs.
    cache.put_file(&key, staged.path());
    Ok((key, None))
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
/// **One file per platform for the whole repository, not one per package.**
/// macOS charges about 200 ms the first time a newly created file is executed.
/// `link::place_from` leaves a file whose bytes already match alone, so a rerun
/// whose binary did not change skips that charge. New bytes always get a new
/// file, because rewriting an executable in place can get it killed by code
/// signing (see `link::place_from`).
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
/// rooted at tests rather than at a `main`. One suite's or a batch's, alike.
///
/// Written to the file [`claim_runner`] names, or to `private` when another
/// binary holds that file. The link is cached on the ordered `codegen` keys, so
/// an edit the front end erases relinks nothing.
///
/// The unit prefix is always empty. A batch spans packages, and a suite run alone
/// uses the same prefix so that its `codegen` keys meet a batch's wherever the
/// two programs agree on a unit.
///
/// The `link` key comes back with the binary, so the next run can start the
/// same binary again without compiling it ([`place_test_binary`]).
#[allow(
    clippy::too_many_arguments,
    reason = "where the cache is, the label, the fallback file, the output, the flags, \
              the program, its tables and the diagnostics: none derivable from another"
)]
pub fn link_test_binary(
    root: &std::path::Path,
    label: &str,
    private: PathBuf,
    output: &Output,
    flags: &Flags,
    program: &mut monomorphize::Program,
    tables: &Tables,
    diagnostics: &mut Diagnostics,
) -> Result<(TestBinary, ActionKey), Diagnostics> {
    let prefix = "";
    let Some(linker) = linker_for(output, diagnostics) else {
        return Err(std::mem::take(diagnostics));
    };
    let objects =
        objects_named(root, prefix, label, output, flags, program, tables, diagnostics)?;
    // Claimed after the objects exist and before anything is written, so a run
    // that fails to compile never takes the shared file at all.
    let binary = claim_runner(&root.join(".buri/out").join(output.dir()), private);
    match link_cached(root, label, output, flags, linker, &objects, binary.path(), prefix) {
        Ok((key, _)) => Ok((binary, key)),
        Err(errors) => {
            diagnostics.extend(errors.items);
            Err(std::mem::take(diagnostics))
        }
    }
}

/// The test binary a run before this one linked under `link`, put where it
/// runs from: the shared runner file where this process can take it, and
/// `private` where it cannot ([`claim_runner`]).
///
/// `None` when the cache no longer holds it, after `buri clean` or a toolchain
/// change, and the suite is then compiled again.
pub fn place_test_binary(
    root: &std::path::Path,
    output: &Output,
    private: PathBuf,
    link: &ActionKey,
) -> Option<TestBinary> {
    let entry = Cache::open(root).entry(link)?;
    let binary = claim_runner(&root.join(".buri/out").join(output.dir()), private);
    write_executable(&entry, binary.path()).ok()?;
    Some(binary)
}

/// Where a test binary whose first suite is `target` runs from when the shared
/// file is taken. One per target, so binaries built at the same time never share one.
pub fn private_test_binary(session: &Session, target: TargetId, output: &Output) -> PathBuf {
    session
        .root
        .join(".buri/out")
        .join(output.dir())
        .join(&session.workspace.package(target.package).path)
        .join(format!("test-{}", target.kind.name()))
}

/// Writes an executable, and makes it one.
///
/// A cached artifact is bytes out of a content-addressed store, and bytes out
/// of a store have no mode. Restoring the execute bit is what makes a cache hit
/// and a fresh link produce the same thing rather than a file that differs from
/// it in the one way `ls` shows and `cmp` does not.
fn write_executable(entry: &std::path::Path, path: &std::path::Path) -> std::io::Result<u64> {
    // Through `link::place_from`, which is where the rule about *not*
    // rewriting an artifact whose bytes are already there is stated, and which
    // is what a fresh link reaches its output through as well. Two ways of
    // putting an executable on disk would be two places for that rule to hold
    // in one of them.
    //
    // A path rather than the bytes since the entry became a file to stream:
    // the artifact is not read into this process at all, on either path
    // through here, and the comparison that decides whether to write is a
    // chunk of each file rather than two copies of a hundred megabytes.
    link::place_from(entry, path)
}

pub fn artifact_path(session: &Session, target: TargetId, output: &Output) -> PathBuf {
    let package = session.workspace.package(target.package);
    session.root.join(artifact_relative(&session.root, &package.path, output))
}

/// Where an output's artifact lands, relative to the repository `root`, for
/// the binary in the package at `package_path`.
pub fn artifact_relative(root: &Path, package_path: &str, output: &Output) -> PathBuf {
    let dir_name = if package_path.is_empty() {
        root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or("main".into())
    } else {
        // `rsplit` yields the whole string when there is no separator, so the
        // last segment is there for any non-empty path — and the branch above
        // is the empty one.
        package_path.rsplit('/').next().unwrap_or(package_path).to_string()
    };
    // An output that enters somewhere other than `main` is named after the
    // function it enters through, because two outputs of one binary otherwise
    // write one path. One entering through `main` keeps the directory's name.
    // A repository platform's entry is named after the platform's entry,
    // whichever function fills it: `fetch.mjs`.
    // So is a bundled platform's entry with a `js` file of its own, such as
    // `web`'s `main.mjs`: the files the platform ships beside it name it.
    let adapted = output.custom.is_none()
        && crate::build::platforms::bundled(output.platform().slug())
            .is_some_and(|rule| rule.entries.iter().any(|e| e.js.is_some()));
    let default = match (&output.custom, output.entry_name()) {
        (Some(custom), _) => custom.point.clone(),
        (None, _) if adapted => output.entry_point().to_string(),
        (None, "main") => dir_name,
        (None, entry) => entry.to_string(),
    };
    let base = output.artifact_name.as_ref().map_or(default, |a| a.value.clone());
    // The catch-all this used to end in would have given a WEB artifact no
    // extension at all. Every JavaScript platform writes an `.mjs`, and a
    // native one writes the bare name, so the match is over the two answers
    // rather than over one platform and everything else.
    let name = if output.platform().is_javascript() { format!("{base}.mjs") } else { base };
    Path::new(".buri/out").join(output.dir()).join(package_path).join(name)
}

/// Where chunk `n` of a module sits: `<artifact>.<n>.mjs`, beside it.
///
/// The module derives the same name from `import.meta.url` at run time
/// (`runtime.js`'s `$lazy`), so this and that are one convention written twice
/// and the file name is the whole of the agreement between them.
pub fn chunk_path(module: &Path, n: usize) -> PathBuf {
    let base = module
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| String::from("main"));
    module.with_file_name(format!("{base}.{n}.mjs"))
}

/// Every chunk of a module, with the path each is written to.
pub fn chunk_paths(module: &Path, chunks: &[String]) -> Vec<(PathBuf, String)> {
    chunks.iter().enumerate().map(|(n, text)| (chunk_path(module, n), text.clone())).collect()
}

/// A JavaScript artifact as the single blob the cache stores it under: the
/// module, the stylesheet, then each chunk.
///
/// A count, then each part's byte length and its bytes. Length-prefixed rather
/// than joined by a separator, because a part is generated JavaScript or CSS and
/// there is no byte sequence it cannot contain.
fn encode_parts<'a>(parts: impl Iterator<Item = &'a str>) -> Vec<u8> {
    let parts: Vec<&str> = parts.collect();
    let mut out = format!("{}\n", parts.len()).into_bytes();
    for part in parts {
        out.extend_from_slice(format!("{}\n", part.len()).as_bytes());
        out.extend_from_slice(part.as_bytes());
    }
    out
}

/// Stores a test suite's JavaScript bundle ([`emit_test_bundle`]) in the cache,
/// and answers the key it is under, which is the hash of the record.
///
/// For a suite that is to run again without being built: its build record
/// names this key (`commands/test.rs`). The record is the artifact's own
/// shape with one part, because a test bundle is never split into chunks.
pub fn put_test_bundle(root: &Path, module: &str) -> ActionKey {
    let bytes = encode_parts(std::iter::once(module));
    let key = ActionKey::of(&bytes);
    Cache::open(root).put(&key, &bytes);
    key
}

/// The bundle [`put_test_bundle`] stored under `key`, if the cache still
/// holds it.
pub fn get_test_bundle(root: &Path, key: &ActionKey) -> Option<String> {
    match decode_parts(&Cache::open(root).get(key)?)?.as_mut_slice() {
        [module] => Some(std::mem::take(module)),
        _ => None,
    }
}

/// The inverse. `None` for a blob this toolchain did not write, which a caller
/// reads as a cache miss.
fn decode_parts(bytes: &[u8]) -> Option<Vec<String>> {
    let (count, mut rest) = frame(bytes)?;
    let count: usize = count.parse().ok()?;
    let mut out = Vec::new();
    for _ in 0..count {
        let (len, body) = frame(rest)?;
        let len: usize = len.parse().ok()?;
        out.push(String::from_utf8(body.get(..len)?.to_vec()).ok()?);
        rest = body.get(len..)?;
    }
    Some(out)
}

/// One newline-terminated number, and everything after it.
fn frame(bytes: &[u8]) -> Option<(&str, &[u8])> {
    let end = bytes.iter().position(|b| *b == b'\n')?;
    let head = std::str::from_utf8(bytes.get(..end)?).ok()?;
    Some((head, bytes.get(end.checked_add(1)?..)?))
}

/// Where an entry's stylesheet sits: `<artifact>.css`, beside its module.
///
/// Written only when the entry uses styles: an empty file would be a request a
/// browser makes for nothing. The module carries the same rules either way,
/// and `mount` installs them when the document does not already link a sheet
/// with `id="buri-styles"`.
pub fn stylesheet_path(module: &Path) -> PathBuf {
    module.with_extension("css")
}

/// Writes them, answering whether every one landed. A failure is reported the
/// same way a failure to write the module is, because from a reader's side it
/// is the same mistake about the same directory.
fn write_companions(
    module: &Path,
    stylesheet: &str,
    chunks: &[String],
    diagnostics: &mut Diagnostics,
) -> bool {
    // A chunk left over from a build that had more of them is a file the
    // module no longer fetches and a reader would have to guess about.
    let mut stale = chunks.len();
    while std::fs::remove_file(chunk_path(module, stale)).is_ok() {
        stale = stale.saturating_add(1);
    }
    let sheet = (!stylesheet.is_empty()).then(|| (stylesheet_path(module), stylesheet.to_string()));
    let companions = sheet.into_iter().chain(chunk_paths(module, chunks));
    for (path, text) in companions {
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
    platform: &OutputPlatform,
    diagnostics: &mut Diagnostics,
) {
    check_visibility(session, target, diagnostics);
    check_platform_visibility(session, platform, diagnostics);
    check_tags_of(session, target, &session.workspace.policy_members(target, platform), diagnostics);
    check_platform(session, target, platform, diagnostics);
}

/// A repository platform's dependencies, held to visibility like a binary's:
/// each is visible to the platform's package, and so is every edge below it.
fn check_platform_visibility(session: &Session, platform: &OutputPlatform, diagnostics: &mut Diagnostics) {
    let OutputPlatform::Repository { label, .. } = platform else { return };
    let ws = &session.workspace;
    let Some((pid, rule)) = ws.platform_rule(label) else { return };
    let mut edges: Vec<(crate::build::workspace::PackageId, TargetId, Span)> = Vec::new();
    let mut members: Vec<TargetId> = Vec::new();
    for dep in &rule.dependencies {
        if let Some(t) = ws.dep_target(&dep.value) {
            edges.push((pid, t, dep.span));
            members.extend(ws.closure(t));
        }
    }
    members.sort();
    members.dedup();
    for member in members {
        for (dep, span) in ws.dep_edges(member) {
            if let Some(span) = span {
                edges.push((member.package, dep, span));
            }
        }
    }
    for (from, dep, span) in edges {
        if ws.visible(from, dep) {
            continue;
        }
        diagnostics.push(
            Diagnostic::templated("visibility-violation", span)
                .with_bind("from_target", ws.package(from).label())
                .with_bind("to_target", ws.label(dep))
                .with_bind("visible_to", ws.visibility_list(dep))
                .with_bind("to_package_path", ws.package(dep.package).path.clone()),
        );
    }
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
    check_tags_of(session, target, &session.workspace.closure(target), diagnostics);
}

/// [`check_tags`] over `members`: the target's closure, and for an output of
/// a repository platform, the platform's dependencies too.
fn check_tags_of(session: &Session, target: TargetId, members: &[TargetId], diagnostics: &mut Diagnostics) {
    // A tag `REPO.buri` does not declare is an error, not a no-op.
    for &member in members {
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

    let Some((a, a_by, b, b_by)) = session.workspace.forbidden_pair_of(members) else { return };
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

    let mut d = Diagnostic::templated("tag-conflict", span)
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
    platform: &OutputPlatform,
    diagnostics: &mut Diagnostics,
) {
    let members = session.workspace.policy_members(target, platform);
    let allowed = session.workspace.platforms_of(&members);
    if allowed.contains(platform) {
        return;
    }
    let label = session.workspace.label(target);
    let span = session
        .workspace
        .package(target.package)
        .build
        .binary
        .as_ref()
        .and_then(|b| b.outputs.iter().find(|o| &o.output_platform() == platform))
        .map(|o| o.span)
        .unwrap_or(Span::point(session.workspace.package(target.package).build_file_id, 0));

    let mut d = Diagnostic::templated("platform-violation", span)
        .with_bind("target", label.as_str())
        .with_bind("platform", platform.name());
    if let Some(found) = session.workspace.platform_blocker_of(&members, platform) {
        let blocker = found.member;
        d = d.with_note(found.why);
        if let Some(word) = found.forbidden {
            d = d.with_fix(format!(
                "drop the {} output, or take {word} out of the tag's `forbids` in REPO.buri",
                platform.name()
            ));
        }
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
    use crate::build::buildfile::Platform;
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
    // -- what a native refusal says -----------------------------------------
    //
    // buri-lang/buri#25 and buri-lang/buri#26 were one sentence serving three
    // causes: "the {platform} backend is not implemented", with a fix reading
    // "this toolchain emits JavaScript; build with `--output=js`". It was false
    // on the two that reach it most — a cross output on a host whose own native
    // build had just succeeded, and `--release` on a toolchain whose debug
    // build of the same output works. These rows are per cause, because the
    // point of the change is that the causes are told apart.

    /// A target this toolchain refuses on this host, whatever the host.
    ///
    /// **Not a Linux target**, which now links from every host
    /// (ARCHITECTURE.md §9). The refused direction is macOS: a Linux host cannot
    /// build a macOS artifact at all, and a macOS host has no cross-arch macOS
    /// runtime — so a macOS target of the non-host architecture is refused on
    /// every machine the suite runs on.
    fn cross_target() -> Target {
        let arch = match link::host_arch() {
            Some(Arch::X86_64) => Arch::Arm64,
            _ => Arch::X86_64,
        };
        Target { platform: Platform::Macos, arch: Some(arch) }
    }

    /// A cross output is refused by the **host**, and the sentence says so —
    /// on every profile, and without claiming anything about backends or about
    /// JavaScript (buri-lang/buri#25).
    #[test]
    fn a_cross_output_is_refused_by_the_host_and_not_by_a_backend() {
        let target = cross_target();
        let arch = target.arch.expect("the cross target names an architecture");
        for profile in [Profile::Debug, Profile::Release] {
            let gap = native_gap(target, profile)
                .unwrap_or_else(|| panic!("{target:?} was not refused in {profile:?}"));
            assert_eq!(gap.output, format!("{}-{}", target.platform.machine(), arch.slug()));
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

    /// The capability lift: a Linux target is *ready* from any host with the
    /// native backend and archive compiled in, where it used to be refused as a
    /// cross output. The runtime and sysroot are cross-built at link time
    /// (ARCHITECTURE.md §9), so readiness is not a claim that the cross build has
    /// happened — only that nothing structural forbids it.
    #[test]
    fn a_linux_output_is_ready_from_any_host() {
        // Only where this toolchain has a native backend and a runtime archive,
        // which is the same precondition the host's own output has; a
        // `--no-default-features` build has neither and refuses both.
        if !runtime_native::AVAILABLE
            || backend::select(
                Target { platform: Platform::Linux, arch: Some(Arch::X86_64) },
                Profile::Debug,
            )
            .is_err()
        {
            return;
        }
        for arch in [Arch::X86_64, Arch::Arm64] {
            let target = Target { platform: Platform::Linux, arch: Some(arch) };
            assert!(
                native_gap(target, Profile::Debug).is_none(),
                "a Linux output should be ready from this host: {target:?}"
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
        assert_eq!(d.code.as_deref(), Some("native-artifact-unavailable"));
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
                platform.slug()
            );
        }
    }
}
