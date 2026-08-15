//! `buri build` and `buri run`.
//!
//! Artifacts land in `.buri/out/<platform>/<package>/<artifact>`, where
//! `<artifact>` is the package's directory name unless the output overrides it
//! with `artifact_name`. Tags are not in the path, because they are not in the
//! cache key: a tag decides whether a build is permitted, never what it
//! produces.

use crate::buildfile::{Output, Platform};
use crate::cache::{Action, Cache, KeyBuilder};
use crate::cli::{Flags, Session};
use crate::codegen;
use crate::compile::Unit;
use crate::diag::{Diagnostic, Diagnostics, Span};
use crate::js;
use crate::mono;
use crate::workspace::{RuleKind, TargetId};
use std::path::PathBuf;

pub struct Artifact {
    pub target: TargetId,
    pub path: PathBuf,
    pub bytes: usize,
    pub cached: bool,
}

/// Builds one target for one output, returning the artifact's path.
pub fn build_target(
    s: &mut Session,
    target: TargetId,
    output: &Output,
    flags: &Flags,
) -> Result<Artifact, Diagnostics> {
    let mut diags = Diagnostics::new();
    let platform = output.platform.as_ref().map(|p| p.value).unwrap_or(Platform::Js);

    // Every check the graph can answer before a line is compiled.
    check_policy(s, target, platform, &mut diags);
    if diags.has_errors() {
        return Err(diags);
    }

    if platform != Platform::Js {
        diags.push(
            Diagnostic::error(
                output.span,
                format!("the {} backend is not implemented", platform.slug()),
            )
            .with_fix("this toolchain emits JavaScript; build with `--output=js`"),
        );
        return Err(diags);
    }

    // The key covers everything that can affect the artifact, so a hit means
    // the compiler has nothing to do.
    let key = action_key(s, target, output, flags, Action::Link);
    let path = artifact_path(s, target, output);
    let cache = Cache::open(&s.root);
    explain_closure(s, target, output, flags);
    let explain_link = |status: crate::cache::Status| {
        crate::cache::explain(
            flags.explain,
            status,
            Action::Link,
            &s.ws.label(target),
            platform,
            &key,
        )
    };
    if !flags.force {
        if let Some(bytes) = cache.get(&key) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::write(&path, &bytes).is_ok() {
                explain_link(crate::cache::Status::Cached);
                link_out_symlink(s, output);
                return Ok(Artifact { target, path, bytes: bytes.len(), cached: true });
            }
        }
    }
    explain_link(crate::cache::Status::Run);

    let unit = Unit { target: Some(target), platform, with_tests: false };
    let analysis = crate::driver::analyze(Some(&s.ws), &mut s.map, &unit);
    if analysis.diags.has_errors() {
        return Err(analysis.diags);
    }
    diags.extend(analysis.diags.items);

    let Some(entry) = analysis.checked.entry else {
        diags.push(
            Diagnostic::error(Span::NONE, format!("{} exports no `main`", s.ws.pkg(target.pkg).label()))
                .with_fix("add `export fn main(): Result<(), Str> { ... }` to its `main.buri`"),
        );
        return Err(diags);
    };

    let module_paths: Vec<String> =
        analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut program = mono::run(
        &analysis.checked,
        module_paths,
        &mut diags,
        mono::Roots::Main(entry),
    );
    if diags.has_errors() {
        return Err(diags);
    }

    let source = emit(&mut program, &analysis.checked.tables, flags, &mut diags)?;
    cache.put(&key, source.as_bytes());
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, &source) {
        diags.push(
            Diagnostic::error(Span::NONE, format!("cannot write {}: {e}", path.display()))
                .with_fix("check the directory exists and is writable"),
        );
        return Err(diags);
    }
    link_out_symlink(s, output);
    Ok(Artifact { target, path, bytes: source.len(), cached: false })
}

/// The key for one action on one target. Paths are repository-relative, so two
/// checkouts in different directories produce identical keys.
pub fn action_key(
    s: &Session,
    target: TargetId,
    output: &Output,
    flags: &Flags,
    action: Action,
) -> String {
    let mut k = KeyBuilder::new(action, &s.ws.repo.toolchain, flags.release);
    k.platform(
        output.platform.as_ref().map(|p| p.value).unwrap_or(Platform::Js),
        output.arch.as_ref().map(|a| a.value),
    );
    // Every target in the closure contributes its identity and its sources,
    // in a deterministic order.
    for member in s.ws.closure(target) {
        contribute(s, member, &mut k);
    }
    k.finish()
}

/// One target's own contribution to a key: its rule identity, and the contents
/// of the sources that rule names. Factored out of `action_key` so it can also
/// be taken alone — which is what `--explain` reports per closure member, and
/// what makes "editing this file changed this target's key and not that one"
/// something a test can watch rather than something a comment asserts.
fn contribute(s: &Session, member: TargetId, k: &mut KeyBuilder) {
    let pkg = s.ws.pkg(member.pkg);
    let kind = match member.kind {
        RuleKind::Library => "library",
        RuleKind::Binary => "binary",
    };
    let mut sources: Vec<String> = Vec::new();
    let entry = if member.kind == RuleKind::Library { "lib.buri" } else { "main.buri" };
    sources.push(entry.to_string());
    match member.kind {
        RuleKind::Library => {
            if let Some(lib) = &pkg.build.library {
                sources.extend(lib.sources.iter().map(|x| x.value.clone()));
                if lib.testing.present {
                    sources.push("testing/lib.buri".into());
                    sources.extend(lib.testing.sources.iter().map(|x| x.value.clone()));
                }
            }
        }
        RuleKind::Binary => {
            if let Some(bin) = &pkg.build.binary {
                sources.extend(bin.sources.iter().map(|x| x.value.clone()));
            }
        }
    }
    sources.sort();
    k.rule_identity(&pkg.label(), kind, &sources);
    for rel in &sources {
        let full = pkg.dir.join(rel);
        let contents = std::fs::read(&full).unwrap_or_default();
        k.input(&s.ws.rel_of(&full), &contents);
    }
}

/// The key for one target's own compilation: its identity and its own sources'
/// contents, and nothing from its dependencies.
///
/// This is not (yet) a cache key — no `compile` action is stored separately —
/// but it is the quantity the incrementality table in
/// HERMETICITY-AND-CACHING.md is written in terms of, so `--explain` reports
/// it and the tests compare it between two states of one tree.
pub fn compile_key(s: &Session, target: TargetId, output: &Output, flags: &Flags) -> String {
    let mut k = KeyBuilder::new(Action::Compile, &s.ws.repo.toolchain, flags.release);
    k.platform(
        output.platform.as_ref().map(|p| p.value).unwrap_or(Platform::Js),
        output.arch.as_ref().map(|a| a.value),
    );
    contribute(s, target, &mut k);
    k.finish()
}

/// Reports every action a build of `target` involves, deepest first: one
/// `compile` line per closure member, then the `link` that consumed them.
pub fn explain_closure(s: &Session, target: TargetId, output: &Output, flags: &Flags) {
    if !flags.explain {
        return;
    }
    let platform = output.platform.as_ref().map(|p| p.value).unwrap_or(Platform::Js);
    for member in s.ws.closure(target) {
        let key = compile_key(s, member, output, flags);
        crate::cache::explain(
            true,
            crate::cache::Status::Keyed,
            Action::Compile,
            &s.ws.label(member),
            platform,
            &key,
        );
    }
}

/// The key for a test suite: its own sources and data on top of the target's.
pub fn test_key(s: &Session, target: TargetId, output: &Output, flags: &Flags) -> String {
    let base = action_key(s, target, output, flags, Action::Test);
    let mut k = KeyBuilder::new(Action::Test, &s.ws.repo.toolchain, flags.release);
    k.dependency(&base);
    let pkg = s.ws.pkg(target.pkg);
    let suite = match target.kind {
        RuleKind::Library => pkg.build.library.as_ref().map(|l| &l.test),
        RuleKind::Binary => pkg.build.binary.as_ref().map(|b| &b.test),
    };
    if let Some(suite) = suite {
        let mut files: Vec<String> =
            suite.sources.iter().chain(suite.data.iter()).map(|x| x.value.clone()).collect();
        files.sort();
        k.rule_identity(&pkg.label(), "test", &files);
        for rel in &files {
            let full = pkg.dir.join(rel);
            k.input(rel, &std::fs::read(&full).unwrap_or_default());
        }
    }
    k.finish()
}

/// Runs monomorphization output through the optimiser, code generation and the
/// minifier.
pub fn emit(
    program: &mut mono::Program,
    tables: &crate::types::Tables,
    flags: &Flags,
    diags: &mut Diagnostics,
) -> Result<String, Diagnostics> {
    let release = flags.release;
    // Both build modes, so that `release_and_debug_agree` keeps covering the
    // optimiser rather than only the part of it release turns on.
    crate::opt::run(program, &crate::opt::Options::default());

    let opts = codegen::Options {
        pretty: !release,
        debug_names: !release,
        defensive_aborts: !release,
    };
    let out = codegen::generate(program, tables, &opts);

    let missing = codegen::check_intrinsics(&out.missing_intrinsics);
    if !missing.is_empty() {
        let mut unique: Vec<String> = missing;
        unique.sort();
        unique.dedup();
        diags.push(
            Diagnostic::error(
                Span::NONE,
                format!("the runtime has no implementation of {}", unique.join(", ")),
            )
            .with_fix("report it: this is a toolchain bug, not a problem with your program"),
        );
        return Err(std::mem::take(diags));
    }

    let mopts = js::MinifyOptions {
        // Debug builds stay readable: the names are what make a stack trace
        // useful, and `--release` is where size matters.
        mangle: release,
        fold: true,
        drop_unreachable: true,
    };
    let stmts = js::minify(out.stmts, &out.roots, &mopts);
    Ok(js::print(&stmts, !release))
}

pub fn artifact_path(s: &Session, target: TargetId, output: &Output) -> PathBuf {
    let pkg = s.ws.pkg(target.pkg);
    let dir_name = if pkg.path.is_empty() {
        s.root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or("main".into())
    } else {
        pkg.path.rsplit('/').next().unwrap().to_string()
    };
    let base = output.artifact_name.clone().unwrap_or(dir_name);
    let name = match output.platform.as_ref().map(|p| p.value) {
        Some(Platform::Js) => format!("{base}.mjs"),
        _ => base,
    };
    s.root.join(".buri/out").join(output.dir()).join(&pkg.path).join(name)
}

/// A convenience symlink pointing at the most recent output directory.
fn link_out_symlink(s: &Session, output: &Output) {
    let link = s.root.join("out");
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
    s: &Session,
    target: TargetId,
    platform: Platform,
    diags: &mut Diagnostics,
) {
    check_visibility(s, target, diags);
    check_tags(s, target, diags);
    check_platform(s, target, platform, diags);
}

pub fn check_visibility(s: &Session, target: TargetId, diags: &mut Diagnostics) {
    for member in s.ws.closure(target) {
        for (dep, span) in s.ws.dep_edges(member) {
            let Some(span) = span else { continue };
            if s.ws.visible(member.pkg, dep) {
                continue;
            }
            let from = s.ws.pkg(member.pkg).label();
            let to = s.ws.label(dep);
            let to_path = s.ws.pkg(dep.pkg).path.clone();
            diags.push(
                Diagnostic::error(span, format!("{from} depends on {to}, which is not visible to it"))
                    .with_code("visibility-violation")
                    .with_label("not visible")
                    .with_note(format!("{to} is visible to: {}", s.ws.visibility_list(dep)))
                    .with_fix(format!(
                        "add \"{from}\" to visibility in {to_path}/BUILD.buri"
                    )),
            );
        }
    }
}

/// Two tags that forbid each other may not appear anywhere in the same
/// dependency closure. The path is printed because in a repository of any size
/// the interesting question is never "which library is tagged `server`" but
/// "who dragged it in".
pub fn check_tags(s: &Session, target: TargetId, diags: &mut Diagnostics) {
    // A tag `REPO.buri` does not declare is an error, not a no-op.
    for member in s.ws.closure(target) {
        for tag in s.ws.tags(member) {
            if s.ws.repo.tag(&tag.value).is_none() {
                let known: Vec<&str> =
                    s.ws.repo.tags.iter().map(|t| t.name.value.as_str()).collect();
                let mut d = Diagnostic::error(tag.span, format!("unknown tag \"{}\"", tag.value))
                    .with_code("unknown-tag")
                    .with_note("no `tag` block in REPO.buri declares this name");
                // A near miss is a guess about which of the two fixes is meant,
                // not a replacement for saying what to do. Both go in the one
                // `fix`, because a diagnostic carries only one.
                d = match crate::buildfile::nearest(&tag.value, &known) {
                    Some(near) => d.with_fix(format!(
                        "did you mean \"{near}\"? — or declare \"{}\" with a `tag` block in REPO.buri",
                        tag.value
                    )),
                    None => {
                        d.with_fix("declare it with a `tag` block in REPO.buri, or drop it here")
                    }
                };
                diags.push(d);
            }
        }
    }

    let Some((a, a_by, b, b_by)) = s.ws.forbidden_pair(target) else { return };
    let label = s.ws.label(target);
    let a_label = s.ws.label(a_by);
    let b_label = s.ws.label(b_by);
    let span = s
        .ws
        .tags(target)
        .iter()
        .map(|t| t.span)
        .next()
        .unwrap_or(Span::point(s.ws.pkg(target.pkg).build_file_id, 0));

    let mut d = Diagnostic::error(
        span,
        format!("{label} cannot contain both \"{a}\" and \"{b}\" code"),
    )
    .with_code("tag-violation")
    .with_fix(format!(
        "drop one of the two dependencies, or split {label} into a target per side"
    ));
    d = d.with_note(if a_by == target {
        format!("\"{a}\" is carried by {label} itself")
    } else {
        format!("\"{a}\" is carried by {a_label}")
    });
    let mut second = if b_by == target {
        format!("\"{b}\" is carried by {label} itself")
    } else {
        format!("\"{b}\" is carried by {b_label}")
    };
    if let Some(path) = s.ws.dep_path(target, b_by) {
        if path.len() > 1 {
            let names: Vec<String> = path.iter().map(|(t, _)| s.ws.label(*t)).collect();
            second.push_str(&format!("\n    reached by: {}", names.join(" -> ")));
        }
    }
    d = d.with_note(second);
    // The doc strings are printed because the tag is a policy, and the policy
    // should say why.
    for name in [&a, &b] {
        let doc = s.ws.tag_doc(name);
        if !doc.is_empty() {
            d = d.with_note(format!("\"{name}\": {doc}"));
        }
    }
    diags.push(d);
}

pub fn check_platform(
    s: &Session,
    target: TargetId,
    platform: Platform,
    diags: &mut Diagnostics,
) {
    let allowed = s.ws.platforms(target);
    if allowed.contains(&platform) {
        return;
    }
    let label = s.ws.label(target);
    let span = s
        .ws
        .pkg(target.pkg)
        .build
        .binary
        .as_ref()
        .and_then(|b| b.outputs.iter().find(|o| o.platform.as_ref().is_some_and(|p| p.value == platform)))
        .map(|o| o.span)
        .unwrap_or(Span::point(s.ws.pkg(target.pkg).build_file_id, 0));

    let mut d = Diagnostic::error(
        span,
        format!("{label} cannot be built for {}", platform.slug()),
    )
    .with_code("platform-violation")
    .with_fix(format!(
        "drop the {} output, or widen the tag's `requires {{ platforms }}` in REPO.buri",
        platform.slug()
    ));
    if let Some((blocker, why)) = s.ws.platform_blocker(target, platform) {
        d = d.with_note(why);
        if let Some(path) = s.ws.dep_path(target, blocker) {
            if path.len() > 1 {
                let names: Vec<String> = path.iter().map(|(t, _)| s.ws.label(*t)).collect();
                d = d.with_note(format!("reached by: {}", names.join(" -> ")));
            }
        }
        for tag in s.ws.tags(blocker) {
            let doc = s.ws.tag_doc(&tag.value);
            if !doc.is_empty() {
                d = d.with_note(format!("\"{}\": {doc}", tag.value));
            }
        }
    } else if allowed.is_empty() {
        d = d.with_note("its dependency closure admits no platform at all");
    }
    diags.push(d);
}

/// The outputs a `build` invocation should produce for one target.
pub fn selected_outputs(s: &Session, target: TargetId, flags: &Flags) -> Vec<Output> {
    if target.kind != RuleKind::Binary {
        return Vec::new();
    }
    let Some(bin) = &s.ws.pkg(target.pkg).build.binary else { return Vec::new() };
    let mut outputs = bin.outputs.clone();
    if outputs.is_empty() {
        // A binary with no declared output still builds for the host, which is
        // what `buri run` needs.
        outputs.push(Output {
            platform: Some(crate::buildfile::Sp::new(Platform::Js, Span::NONE)),
            ..Output::default()
        });
    }
    match &flags.output {
        Some(sel) => outputs.into_iter().filter(|o| o.matches_selector(sel)).collect(),
        None => outputs,
    }
}
