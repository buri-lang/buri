//! `tool` rules: programs the build runs on a language's files.
//!
//! ```text
//! tool {                      // tools/lines/BUILD.buri
//!     check {}
//!     format {}
//! }
//! ```
//!
//! A tool's `tool.buri` exports one function per block, and has no `main`. To
//! run one, the build writes a `main` that calls `core/tool`'s `serve` with the
//! tool's entry points ([`harness`]), compiles it to JavaScript once per key,
//! and hands it one line of JSON on standard input:
//!
//! ```text
//! -> {"entry":"check","inputs":[{"path":"lib/a.lines","language":"lines","value":"..."}],"files":[]}
//! <- {"diagnostics":[...],"needs":[]}
//! ```
//!
//! An answer that `needs` a file is asked again with that file in `files`.
//! Every exchange is cached under the tool's program key and the request, so
//! editing the tool, the file, or a file it read asks again, and nothing else
//! does.
//!
//! The toolchain's own tools are `std/json`, whose `check` and `format` are
//! native ([`crate::languages::json`]), and `std/proto`, whose `generate` is a
//! Buri program like any other tool.

use crate::build::buildfile::{self, Output, Platform, Spanned};
use crate::build::cache::{Action, ActionKey, Cache, KeyBuilder};
use crate::build::session::Session;
use crate::build::workspace::{RuleKind, TargetId, Workspace};
use crate::commands::arguments::Flags;
use crate::diagnostics::{Diagnostic, Span};
use crate::json::Value;
use crate::languages::{Finding, Kind, Language, Said};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// The entry points a tool may export, one per block.
pub const ENTRY_POINTS: [&str; 3] = ["check", "format", "generate"];

/// The built-in JSON tool: `check` and `format`, in-tree.
pub const JSON: &str = "std/json";

/// The built-in `.proto` tool: `generate`, through `std/proto`.
pub const PROTO: &str = "std/proto";

/// What a tool name refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// A `tool` rule in this repository.
    Repo(TargetId),
    Json,
    Proto,
}

/// Why a name names no tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unresolved {
    /// A `//label` naming a binary, which is what a generator used to be.
    Binary,
    Nothing,
}

/// The tool a name refers to.
pub fn resolve(workspace: &Workspace, name: &str) -> Result<Tool, Unresolved> {
    match name {
        JSON => return Ok(Tool::Json),
        PROTO => return Ok(Tool::Proto),
        _ => {}
    }
    let path = name.strip_prefix("//").ok_or(Unresolved::Nothing)?;
    let package = workspace.package_by_path(path).ok_or(Unresolved::Nothing)?;
    let p = workspace.package(package);
    if p.has_tool() {
        return Ok(Tool::Repo(TargetId { package, kind: RuleKind::Tool }));
    }
    Err(if p.has_binary() { Unresolved::Binary } else { Unresolved::Nothing })
}

impl Tool {
    /// Whether the tool has the entry point `entry`.
    pub fn provides(self, workspace: &Workspace, entry: &str) -> bool {
        match self {
            Tool::Repo(t) => {
                workspace.package(t.package).build.tool.as_ref().is_some_and(|r| r.block(entry).is_some())
            }
            Tool::Json => matches!(entry, "check" | "format"),
            Tool::Proto => entry == "generate",
        }
    }

    /// The name a diagnostic calls it by.
    pub fn name(self, workspace: &Workspace) -> String {
        match self {
            Tool::Repo(t) => workspace.label(t),
            Tool::Json => JSON.to_string(),
            Tool::Proto => PROTO.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// What the graph can say before anything runs
// ---------------------------------------------------------------------------

/// Every tool reference in the repository that names nothing usable: a
/// `generators` entry's `tool`, and a language's `check`, `format` and
/// `generate`. Each names a tool with the entry point of that name.
pub fn validate(workspace: &Workspace) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for id in workspace.ids() {
        let build = &workspace.package(id).build;
        let generators = build.library.iter().flat_map(|l| l.generators.iter());
        for g in generators.chain(build.binary.iter().flat_map(|b| b.generators.iter())) {
            refer(workspace, &g.tool, "generate", true, &mut out);
        }
    }
    for language in &workspace.repo.languages.all {
        let Some(tools) = language.tools() else { continue };
        for entry in ENTRY_POINTS {
            if let Some(named) = tools.named(entry) {
                refer(workspace, named, entry, false, &mut out);
            }
        }
    }
    out
}

fn refer(workspace: &Workspace, named: &Spanned<String>, entry: &str, generator: bool, out: &mut Vec<Diagnostic>) {
    // Reported where the file was read, under the name it has now.
    if buildfile::RETIRED_TOOL_NAMES.iter().any(|(old, _)| *old == named.value) {
        return;
    }
    match resolve(workspace, &named.value) {
        Ok(tool) if tool.provides(workspace, entry) => {}
        Ok(_) => out.push(
            Diagnostic::templated("tool-without-entry-point", named.span)
                .with_bind("tool", named.value.as_str())
                .with_bind("entry", entry),
        ),
        Err(Unresolved::Binary) if generator => out.push(
            Diagnostic::templated("generator-is-a-binary", named.span).with_bind("tool", named.value.as_str()),
        ),
        Err(_) => out.push(Diagnostic::templated("no-such-tool", named.span).with_bind("tool", named.value.as_str())),
    }
}

/// What `tool.buri` has to say about its `BUILD.buri`: every block has its
/// exported function and every exported entry point its block, and `ctx` is
/// bounded by `Allocator` alone.
pub fn contract(module: &crate::parsing::tree::Module, tool: &buildfile::Tool) -> Vec<Diagnostic> {
    use crate::parsing::tree::{Item, ParamKind};
    let tree = &module.tree;
    let mut out = Vec::new();
    let exported: BTreeMap<&str, &crate::parsing::tree::FnDecl> = module
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Fn(f) if f.exported => Some((tree.name(f.name), &**f)),
            _ => None,
        })
        .filter(|(name, _)| ENTRY_POINTS.contains(name))
        .collect();
    for entry in ENTRY_POINTS {
        match (tool.block(entry), exported.get(entry)) {
            (Some(block), None) => {
                out.push(Diagnostic::templated("tool-entry-point-not-exported", block).with_bind("entry", entry))
            }
            (None, Some(f)) => {
                out.push(Diagnostic::templated("tool-entry-point-undeclared", f.name.span).with_bind("entry", entry))
            }
            _ => {}
        }
    }
    for (entry, f) in &exported {
        let ctx = f.params.iter().find(|p| p.kind == ParamKind::CtxParam).or(f.params.first());
        let Some(head) = ctx.and_then(|p| p.written_type()).and_then(|t| tree.type_head(t)) else { continue };
        let Some(generic) = f.generics.iter().find(|g| tree.name(g.name) == head) else { continue };
        for bound in tree.type_list(generic.bounds) {
            let effect = tree.type_head(*bound).unwrap_or_default();
            if effect != "Allocator" {
                out.push(
                    Diagnostic::templated("tool-context-beyond-allocator", tree.type_span(*bound))
                        .with_bind("entry", *entry)
                        .with_bind("effect", effect),
                );
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The program a tool is
// ---------------------------------------------------------------------------

/// The `main` the build writes for a tool: `serve`, over the entry points
/// `module` exports and `provides` says it has.
pub fn harness(module: &str, provides: &dyn Fn(&str) -> bool) -> String {
    let mut fields = String::new();
    for entry in ENTRY_POINTS {
        let value = match provides(entry) {
            true => format!(".Some(fn(c, request) => entry.{entry}(c, request))"),
            false => ".None".to_string(),
        };
        fields.push_str(&format!("        {entry}: {value},\n"));
    }
    format!(
        "from \"core/effect\" import {{ Allocator, Stdin, Stdout }};\n\
         from \"core/host\" import * as host;\n\
         from \"core/tool\" import * as tool;\n\
         from \"{module}\" import * as entry;\n\
         \n\
         export fn main(): Result<(), Str> {{\n\
         \x20   let ctx = context {{\n\
         \x20       Allocator: host.alloc,\n\
         \x20       Stdin: host.stdin,\n\
         \x20       Stdout: host.stdout,\n\
         \x20   }};\n\
         \x20   tool.serve(ctx, tool.Entries {{\n{fields}    }})\n\
         }}\n"
    )
}

/// What a runnable tool is compiled from: the module it imports, the package
/// its `main` stands in, and that `main`.
fn source(workspace: &Workspace, tool: Tool) -> Option<(Option<crate::build::workspace::PackageId>, String, String)> {
    match tool {
        Tool::Repo(t) => {
            let package = workspace.package(t.package);
            let module = package.module_path("tool.buri");
            let main = harness(&module, &|e| tool.provides(workspace, e));
            Some((Some(t.package), package.module_path("(tool main)"), main))
        }
        Tool::Proto => Some((None, "(std/proto main)".to_string(), harness("std/proto", &|e| e == "generate"))),
        Tool::Json => None,
    }
}

/// The key of the program a tool is: its `main`, and — for a tool of this
/// repository — the `link` key of everything it is compiled from. Computed
/// without building anything, so a cached answer costs no compile.
pub fn program_key(session: &Session, tool: Tool, flags: &Flags) -> ActionKey {
    let mut k = KeyBuilder::new(Action::Link, flags.mode);
    k.platform(Platform::Js, None);
    let name = tool.name(&session.workspace);
    k.rule_identity(&name, "tool", &[]);
    if let Some((_, _, main)) = source(&session.workspace, tool) {
        k.input("main", main.as_bytes());
    }
    if let Tool::Repo(t) = tool {
        k.dependency(&crate::build::actions::action_key(session, t, &Output::js(Span::NONE), flags, Action::Link));
    }
    k.finish()
}

/// The `.mjs` a tool is compiled to, built once per [`program_key`] and kept.
///
/// Written through a temporary and renamed, because two builds in one
/// repository may reach this at the same moment.
pub fn artifact(session: &Session, tool: Tool, flags: &Flags) -> Result<PathBuf, String> {
    let name = tool.name(&session.workspace);
    let Some((package, main_name, main)) = source(&session.workspace, tool) else {
        return Err(format!("`{name}` runs in-tree and has no program"));
    };
    let key = program_key(session, tool, flags);
    let dir = session.root.join(".buri/out/tools");
    let path = dir.join(format!("{}.mjs", key.as_str()));
    if path.is_file() {
        return Ok(path);
    }
    if let Some(why) = broken_contract(&session.workspace, tool) {
        return Err(format!("the tool does not build: {why}"));
    }
    let mut map = crate::diagnostics::SourceMap::new();
    let (js, _chunks) = crate::compiler::driver::compile_snippet_js_as(
        Some(&session.workspace),
        package,
        &mut map,
        &main_name,
        &main,
    )
    .map_err(|d| match d.items.first() {
        Some(first) => format!("the tool does not build: {}", map.render(first, false)),
        None => "the tool does not build".to_string(),
    })?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let staged = dir.join(format!("{}.mjs.{}", key.as_str(), std::process::id()));
    std::fs::write(&staged, js.as_bytes()).map_err(|e| format!("{}: {e}", staged.display()))?;
    std::fs::rename(&staged, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// The first thing [`contract`] says about a tool, so a tool whose blocks and
/// exports disagree is refused in those words rather than in the words of the
/// `main` that could not call it.
fn broken_contract(workspace: &Workspace, tool: Tool) -> Option<String> {
    let Tool::Repo(t) = tool else { return None };
    let package = workspace.package(t.package);
    let rule = package.build.tool.as_ref()?;
    let text = std::fs::read_to_string(package.dir.join("tool.buri")).ok()?;
    let parsed = crate::parsing::parser::parse(&text, crate::diagnostics::FileId(0));
    contract(&parsed.module, rule).first().map(|d| d.message.clone())
}

// ---------------------------------------------------------------------------
// Asking a tool
// ---------------------------------------------------------------------------

/// One question for one entry point: the request's fields besides `entry` and
/// `files`, and the name the answer is filed under.
pub struct Ask<'a> {
    pub tool: Tool,
    pub entry: &'static str,
    pub fields: Vec<(&'static str, Value)>,
    pub label: &'a str,
}

/// A tool's answer, after every `needs` it had was met.
pub struct Answer {
    /// What the last exchange was cached under: the tool, and the request
    /// with every file it read.
    pub key: ActionKey,
    pub line: String,
    pub value: Value,
    /// Every path the tool asked for, found or not.
    pub asked: BTreeSet<String>,
    /// Paths it asked for that are not in the repository.
    pub outside: Vec<String>,
}

/// One `{path, language, value}` input.
pub fn input(path: &str, language: &str, text: &str) -> Value {
    Value::object(vec![("path", Value::str(path)), ("language", Value::str(language)), ("value", Value::str(text))])
}

/// Asks, and asks again for as long as the answer needs files it has not
/// been handed. A path that names no file is left out of `files`, and a tool
/// that asks for nothing new has its answer.
pub fn exchange(
    session: &Session,
    ask: &Ask<'_>,
    read: &dyn Fn(&str) -> Option<String>,
    flags: &Flags,
) -> Result<Answer, String> {
    let program = program_key(session, ask.tool, flags);
    let cache = Cache::open(&session.root);
    let mut files: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut outside: Vec<String> = Vec::new();
    // Each round reads at least one file it had not, and a repository holds
    // finitely many; the bound is for a tool whose names never repeat.
    for _round in 0..64 {
        let mut fields = vec![("entry", Value::str(ask.entry))];
        fields.extend(ask.fields.iter().map(|(k, v)| (*k, v.clone())));
        let present = files
            .iter()
            .filter_map(|(p, t)| t.as_ref().map(|t| Value::Array(vec![Value::str(p), Value::str(t)])));
        fields.push(("files", Value::Array(present.collect())));
        let request = Value::object(fields).to_string();
        let action = match ask.entry {
            "generate" => Action::Generate,
            "format" => Action::Format,
            _ => Action::Check,
        };
        let mut k = KeyBuilder::new(action, flags.mode);
        k.rule_identity(ask.label, ask.entry, &[]);
        k.dependency(&program);
        k.input("request", request.as_bytes());
        let key = k.finish();
        let cached = match flags.force {
            true => None,
            false => cache.get(&key).and_then(|b| String::from_utf8(b).ok()),
        };
        let line = match cached {
            Some(line) => line,
            None => {
                let path = artifact(session, ask.tool, flags)?;
                let line = crate::build::generators::run_artifact(&path, &request)?;
                crate::json::parse(&line).map_err(|e| format!("the tool's answer is not JSON: {e}"))?;
                cache.put(&key, line.as_bytes());
                line
            }
        };
        let value = crate::json::parse(&line).map_err(|e| format!("the tool's answer is not JSON: {e}"))?;
        let mut fresh = false;
        for need in value.get("needs").and_then(Value::as_array).unwrap_or_default() {
            let Some(need) = need.as_str() else { continue };
            if files.contains_key(need) || outside.iter().any(|o| o == need) {
                continue;
            }
            match local(need) {
                Some(rel) => {
                    files.insert(need.to_string(), read(&rel));
                    fresh = true;
                }
                None => outside.push(need.to_string()),
            }
        }
        if !fresh {
            let asked = files.into_keys().collect();
            return Ok(Answer { key, line, value, asked, outside });
        }
    }
    Err("the tool kept asking for more files".to_string())
}

/// A repository path, or `None` for one that leaves the repository: a URL, an
/// absolute path, or one that climbs out with `..`.
fn local(path: &str) -> Option<String> {
    let rest = path.strip_prefix("//").unwrap_or(path);
    if rest.is_empty() || rest.starts_with('/') || rest.contains(':') || rest.contains('\\') {
        return None;
    }
    let mut out: Vec<&str> = Vec::new();
    for segment in rest.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            s => out.push(s),
        }
    }
    (!out.is_empty()).then(|| out.join("/"))
}

/// A tool's diagnostics, as generator diagnostics: the shape `core/tool`
/// shares with `core/codegen`.
fn diagnostics(value: &Value) -> Vec<crate::build::generators::Diagnostic> {
    let text = |d: &Value, name| d.get(name).and_then(Value::as_str).map(str::to_string);
    value
        .get("diagnostics")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|d| {
            let origin = d.get("origin").and_then(|o| {
                let span = o.get("span")?;
                let offset = |n| span.get(n).and_then(Value::as_u32).map(|n| n as usize);
                Some(crate::build::generators::Origin { file: text(o, "file")?, span: (offset("start")?, offset("end")?) })
            });
            Some(crate::build::generators::Diagnostic {
                code: text(d, "code")?,
                message: text(d, "message")?,
                note: text(d, "note"),
                fix: text(d, "fix"),
                origin,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Checking and formatting a file
// ---------------------------------------------------------------------------

/// One file's check, whoever checks its language.
pub struct Checked {
    pub key: ActionKey,
    pub findings: Vec<Finding>,
    /// Every other repository path the check looked for, found or not.
    pub asked: BTreeSet<String>,
}

/// Checks one file a rule references, keyed and cached. `None` for a file in
/// no language, or in one no tool checks.
pub fn check_file(
    session: &Session,
    rel: &str,
    text: &str,
    read: &dyn Fn(&str) -> Option<String>,
    flags: &Flags,
) -> Option<Checked> {
    let languages = &session.workspace.repo.languages;
    let language = languages.of(rel)?;
    let named = match &language.kind {
        Kind::BuiltIn(_) => return Some(check_json(session, language, rel, text, read, flags)),
        Kind::Custom(tools) => tools.check.as_ref()?,
    };
    let tool = resolve(&session.workspace, &named.value).ok()?;
    if tool == Tool::Json {
        return Some(check_json(session, language, rel, text, read, flags));
    }
    let ask = Ask {
        tool,
        entry: "check",
        fields: vec![("inputs", Value::Array(vec![input(rel, &language.name, text)]))],
        label: rel,
    };
    Some(match exchange(session, &ask, read, flags) {
        Ok(answer) => {
            let mut findings: Vec<Finding> = diagnostics(&answer.value)
                .into_iter()
                .map(|d| {
                    let (file, span) = d.origin.map_or((rel.to_string(), (0, 0)), |o| (o.file, o.span));
                    Finding {
                        code: d.code,
                        file,
                        span,
                        binds: Vec::new(),
                        said: Some(Box::new(Said { message: d.message, note: d.note, fix: d.fix })),
                    }
                })
                .collect();
            findings.extend(
                answer.outside.iter().map(|p| Finding::new("schema-not-local", rel, (0, 0), vec![("schema", p.clone())])),
            );
            Checked { key: answer.key, findings, asked: answer.asked }
        }
        Err(why) => {
            let mut k = KeyBuilder::new(Action::Check, flags.mode);
            k.rule_identity(rel, "failed", &[]);
            k.input(rel, text.as_bytes());
            let failed = Finding::new("tool-failed", rel, (0, 0), vec![("tool", named.value.clone()), ("why", why)]);
            Checked { key: k.finish(), findings: vec![failed], asked: BTreeSet::new() }
        }
    })
}

/// `std/json`'s check, in-tree: keyed on the file and every schema it reads.
fn check_json(
    session: &Session,
    language: &Language,
    rel: &str,
    text: &str,
    read: &dyn Fn(&str) -> Option<String>,
    flags: &Flags,
) -> Checked {
    let languages = &session.workspace.repo.languages;
    let check = crate::languages::Check::prepare(languages, rel, text.to_string(), read);
    let mut k = KeyBuilder::new(Action::Check, flags.mode);
    k.rule_identity(rel, &language.name, &[]);
    k.input(rel, text.as_bytes());
    for (path, contents) in &check.reads {
        k.input(path, contents.as_bytes());
    }
    let key = k.finish();
    let cache = Cache::open(&session.root);
    let cached = match flags.force {
        true => None,
        false => cache
            .get(&key)
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|text| Finding::decode(&text)),
    };
    let findings = match cached {
        Some(found) => found,
        None => {
            let found = check.run(languages);
            cache.put(&key, Finding::encode(&found).as_bytes());
            found
        }
    };
    Checked { key, findings, asked: check.asked }
}

/// What formatting one file comes to.
#[derive(Debug, PartialEq, Eq)]
pub enum Formatted {
    /// The canonical text.
    Text(String),
    /// The file does not parse, or its tool would not format it: left alone.
    Refused,
    /// Nothing formats this file's language.
    Unformatted,
}

/// Formats one file in a language, through whichever tool formats it.
pub fn format_file(session: &Session, rel: &str, text: &str, flags: &Flags) -> Formatted {
    let refused = |t: Option<String>| t.map_or(Formatted::Refused, Formatted::Text);
    let languages = &session.workspace.repo.languages;
    let Some(language) = languages.of(rel) else { return Formatted::Unformatted };
    let named = match &language.kind {
        Kind::BuiltIn(dialect) => return refused(crate::languages::json::format(text, *dialect)),
        Kind::Custom(tools) => match &tools.format {
            Some(named) => named,
            None => return Formatted::Unformatted,
        },
    };
    let tool = match resolve(&session.workspace, &named.value) {
        Ok(Tool::Json) => return refused(crate::languages::json::format(text, crate::languages::json::Dialect::Json)),
        Ok(tool) => tool,
        Err(_) => return Formatted::Unformatted,
    };
    let ask = Ask { tool, entry: "format", fields: vec![("input", input(rel, &language.name, text))], label: rel };
    let read = |_: &str| None;
    let Ok(answer) = exchange(session, &ask, &read, flags) else { return Formatted::Refused };
    refused(answer.value.get("doc").and_then(doc).map(|d| crate::layout::render(&d)))
}

/// A `core/format` doc, from the tagged arrays `serve` writes.
fn doc(value: &Value) -> Option<crate::layout::Doc> {
    use crate::layout::Doc;
    let items = value.as_array()?;
    let (tag, rest) = items.split_first()?;
    let text = |i: usize| rest.get(i).and_then(Value::as_str).map(str::to_string);
    let docs = |i: usize| -> Option<Vec<Doc>> { rest.get(i)?.as_array()?.iter().map(doc).collect() };
    Some(match tag.as_str()? {
        "text" => Doc::Text(text(0)?),
        "concat" => Doc::Concat(docs(0)?),
        "line" => Doc::Line,
        "softline" => Doc::SoftLine,
        "hardline" => Doc::HardLine,
        "group" => Doc::Group(docs(0)?),
        "indent" => Doc::Indent(docs(0)?),
        "ifbreak" => Doc::IfBreak(Box::new(doc(rest.first()?)?), Box::new(doc(rest.get(1)?)?)),
        "linesuffix" => Doc::LineSuffix(text(0)?),
        "breakparent" => Doc::BreakParent,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_needed_path_stays_in_the_repository() {
        assert_eq!(local("schemas/a.json").as_deref(), Some("schemas/a.json"));
        assert_eq!(local("//schemas/./a.json").as_deref(), Some("schemas/a.json"));
        assert_eq!(local("lib/../schemas/a.json").as_deref(), Some("schemas/a.json"));
        for outside in ["../a.json", "/etc/passwd", "https://example.com/a.json", "", "lib/.."] {
            assert_eq!(local(outside), None, "{outside}");
        }
    }

    #[test]
    fn every_doc_variant_reads() {
        let written = r#"["group",[["text","["],["indent",[["softline"],["text","1,"],["line"],["text","2"]]],
            ["ifbreak",["text",","],["text",""]],["softline"],["hardline"],["linesuffix"," // a"],["breakparent"],
            ["concat",[]],["text","]"]]]"#;
        let value = crate::json::parse(written).unwrap();
        let read = doc(&value).expect("the doc reads");
        assert!(matches!(read, crate::layout::Doc::Group(ref all) if all.len() == 9));
        assert_eq!(doc(&crate::json::parse(r#"["nonsense"]"#).unwrap()), None);
    }

    #[test]
    fn the_harness_calls_only_what_the_tool_has() {
        let main = harness("//tools/lines/tool.buri", &|e| e == "check");
        assert!(main.contains("check: .Some(fn(c, request) => entry.check(c, request))"), "{main}");
        assert!(main.contains("format: .None"), "{main}");
        assert!(main.contains("from \"//tools/lines/tool.buri\" import * as entry;"), "{main}");
    }
}
