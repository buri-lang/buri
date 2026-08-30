//! What a file appearing, moving or going away does to the repository:
//! `workspace/willCreateFiles`, `willRenameFiles`, `willDeleteFiles` and the
//! three `did*Files` notifications that follow them.
//!
//! A Buri module is not a file the compiler finds — it is a file a `BUILD.buri`
//! lists, and a path other modules write in an import. So a rename in the file
//! tree is two rewrites away from being a rename in the language, and an editor
//! that made one without the other leaves the repository not building. That is
//! the same failure `rename.rs` exists to prevent for names, arriving through a
//! different door.
//!
//! **What is answered and what is left to be a diagnostic.** The `sources`
//! entry and the import paths are rewritten, because both are restatements of
//! where the file is and neither is a decision. `dependencies` is not: a
//! cross-package move changes which libraries a package uses, and that answer
//! comes from analysing the repository *after* the move — so it arrives as an
//! ordinary `missing-dep` finding with the code action `buri gen` already
//! carries. A delete leaves its importers dangling for the same reason: which
//! of them should now import what is a judgement, and a server that guessed
//! would be editing code nobody asked it to.

use crate::build::regenerate;
use crate::build::session::Session;
use crate::build::textproto::{self, Document};
use crate::diagnostics::FileId;
use crate::json::Value;
use crate::parsing::tree::Item;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use super::build_files::is_build_file;
use super::convert;
use super::links::inside_quotes;
use super::state::State;

/// The files these operations are about, and the ones the server asks to have
/// watched.
///
/// One glob for both source languages: a `.buri` module and a `.proto` schema
/// are both listed by a rule, in `sources` and `proto_sources` respectively.
/// `matches: "file"` and not a folder — a folder is not a module, and the
/// modules inside one arrive as their own operations if the client sends them
/// at all.
pub const GLOB: &str = "**/*.{buri,proto}";

/// The `sources`-family fields a rule can hold, in the order they are searched
/// for an entry.
const FIELDS: [(&str, &[&str]); 7] = [
    ("library", &["sources"]),
    ("library", &["proto_sources"]),
    ("library", &["test", "sources"]),
    ("library", &["testing", "sources"]),
    ("binary", &["sources"]),
    ("binary", &["proto_sources"]),
    ("binary", &["test", "sources"]),
];

/// The three files a rule names by a field of its own rather than in a list.
const ENTRY_POINTS: [&str; 3] = ["lib.buri", "main.buri", "testing/lib.buri"];

/// The six operations this server takes part in, all under the one filter.
///
/// The glob matches a `BUILD.buri` and a `REPO.buri` too, because both wear the
/// extension. Those are guarded in the handlers instead: renaming a build file
/// is not renaming a module, and there is no glob that says "every `.buri` but
/// those two".
pub fn capability() -> Value {
    let filter = || {
        Value::object(vec![(
            "filters",
            Value::Array(vec![Value::object(vec![
                ("scheme", Value::str("file")),
                (
                    "pattern",
                    Value::object(vec![
                        ("glob", Value::str(GLOB)),
                        ("matches", Value::str("file")),
                    ]),
                ),
            ])]),
        )])
    };
    Value::object(vec![
        ("didCreate", filter()),
        ("didDelete", filter()),
        ("didRename", filter()),
        ("willCreate", filter()),
        ("willDelete", filter()),
        ("willRename", filter()),
    ])
}

// ---------------------------------------------------------------------------
// The three questions
// ---------------------------------------------------------------------------

/// A new file has to be listed by the package it lands in, or the compiler
/// never loads it.
pub fn will_create(state: &mut State, params: &Value) -> Option<Value> {
    let mut edits = Edits::default();
    for path in named(params, "uri") {
        if is_build_file(&path) {
            continue;
        }
        let Some(session) = state.session_for(&path) else { continue };
        let Some(place) = placement(&session, &path) else { continue };
        let Some((rule, field)) = destination(&place) else { continue };
        let change = Change::Added { rule, field, rel: place.rel.clone() };
        sources(state, &mut edits, &place.build_path, &change);
    }
    edits.finish()
}

/// The centrepiece: a module moving takes its `sources` entry and every import
/// that names it along with it.
pub fn will_rename(state: &mut State, params: &Value) -> Option<Value> {
    let mut edits = Edits::default();
    for (old, new) in pairs(params) {
        // A build file rename is not a module rename. Moving a `BUILD.buri`
        // away deletes a package, which is a decision, not a restatement.
        if is_build_file(&old) || is_build_file(&new) {
            continue;
        }
        let Some(session) = state.session_for(&old) else { continue };
        let Some(before) = placement(&session, &old) else { continue };
        let Some(after) = placement(&session, &new) else { continue };

        if before.build_path == after.build_path {
            let change = Change::Renamed { from: before.rel.clone(), to: after.rel.clone() };
            sources(state, &mut edits, &before.build_path, &change);
        } else {
            // Across packages the entry moves between two build files. What
            // the move does to either package's `dependencies` is derived from
            // the repository as it will be, so it is left to `buri gen` and to
            // the `missing-dep` finding that asks for it.
            let dropped = Change::Dropped { rel: before.rel.clone() };
            sources(state, &mut edits, &before.build_path, &dropped);
            if let Some((rule, field)) = destination(&after) {
                let added = Change::Added { rule, field, rel: after.rel.clone() };
                sources(state, &mut edits, &after.build_path, &added);
            }
        }

        let (Some(from), Some(to)) = (before.module.as_deref(), after.module.as_deref()) else {
            continue;
        };
        if from != to {
            imports(state, &old, to, &mut edits);
        }
    }
    edits.finish()
}

/// A file going away stops being one of its package's sources, and nothing
/// else. See the module header on why the dangling imports stay dangling.
pub fn will_delete(state: &mut State, params: &Value) -> Option<Value> {
    let mut edits = Edits::default();
    for path in named(params, "uri") {
        if is_build_file(&path) {
            continue;
        }
        let Some(session) = state.session_for(&path) else { continue };
        let Some(place) = placement(&session, &path) else { continue };
        let change = Change::Dropped { rel: place.rel.clone() };
        sources(state, &mut edits, &place.build_path, &change);
    }
    edits.finish()
}

// ---------------------------------------------------------------------------
// The three notifications
// ---------------------------------------------------------------------------

/// The rename happened. The buffer the editor was holding is still the same
/// text under a new name, and the client is not required to close and reopen
/// it — so a buffer left filed under the old path would be layered over a file
/// that is gone.
pub fn moved(state: &mut State, params: &Value) {
    for (old, new) in pairs(params) {
        if let Some(text) = state.drop_buffer(&old) {
            state.set_buffer(new.clone(), text);
        }
        // The colours the client holds were computed for a document under its
        // old name, and the id it would quote back means nothing now.
        state.semantic_tokens.results.remove(&old);
        state.semantic_tokens.results.remove(&new);
    }
}

/// The delete happened: the buffer, the colours and the findings for a file
/// that is no longer there all go, and the editor is told the findings are
/// gone rather than left showing squiggles on a file nobody can open.
pub fn gone(state: &mut State, params: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for path in named(params, "uri") {
        state.drop_buffer(&path);
        state.semantic_tokens.results.remove(&path);
        let uri = convert::uri_of(&path);
        state.showing_parse_errors.remove(&uri);
        if state.published.remove(&uri).is_some() {
            out.push(super::publish(&uri, Vec::new()));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Where a file sits
// ---------------------------------------------------------------------------

/// A file's place in the build graph, computed from the path alone.
///
/// Nothing here reads the file, which is what makes it answerable for a file
/// that does not exist yet or is about to stop existing: which package owns a
/// path is the nearest `BUILD.buri` above it, and the module path is the name
/// that path is spelled with.
struct Placement {
    build_path: PathBuf,
    /// The path relative to the package directory, as a `sources` entry.
    rel: String,
    /// The module path an import writes to name this file, when it has one.
    module: Option<String>,
    has_library: bool,
    has_binary: bool,
}

fn placement(session: &Session, path: &Path) -> Option<Placement> {
    let package = session.workspace.owning_package(path)?;
    let p = session.workspace.package(package);
    let rel = path.strip_prefix(&p.dir).ok()?.display().to_string().replace('\\', "/");
    Some(Placement {
        build_path: p.build_path.clone(),
        module: module_path(&p.path, &rel),
        rel,
        has_library: p.has_library(),
        has_binary: p.has_binary(),
    })
}

/// The inverse of `Workspace::resolve_module`: the one path an import writes to
/// name this file.
///
/// Written out rather than resolved, because the file this is asked about is
/// either not there yet or about to go — and resolution insists the file exist.
///
/// Since an import names a file this is now only the repository-relative name
/// with `//` in front of it. The four shapes it used to spell differently —
/// surface, testing surface, entry point, inner module — are one shape, and a
/// package at the repository root no longer has a surface with no way to write
/// it down.
fn module_path(package_path: &str, rel: &str) -> Option<String> {
    if !crate::build::workspace::names_a_file(rel) {
        return None;
    }
    match package_path.is_empty() {
        true => Some(format!("//{rel}")),
        false => Some(format!("//{package_path}/{rel}")),
    }
}

/// The rule and the field a *new* entry belongs in, or `None` when placing it
/// is a judgement.
///
/// The judgement is `buri gen`'s and this refuses it for the same reason: in a
/// package with both a library and a binary, which rule owns a file is which
/// entry point's imports reach it, and nothing reaches a file that does not
/// exist yet. An entry point is `None` too — `lib.buri`, `main.buri` and
/// `testing/lib.buri` are named by the rule itself and are in no list.
fn destination(place: &Placement) -> Option<(&'static str, &'static [&'static str])> {
    if ENTRY_POINTS.contains(&place.rel.as_str()) {
        return None;
    }
    let rule = match (place.rel.starts_with("testing/"), place.has_library, place.has_binary) {
        // A `testing/` surface is a library's, and only a library declares one.
        (true, true, _) => "library",
        (true, false, _) => return None,
        (false, true, false) => "library",
        (false, false, true) => "binary",
        (false, _, _) => return None,
    };
    Some((rule, field_for(&place.rel)))
}

/// Which list holds a file, by the same reading of its path `buri gen` uses.
fn field_for(rel: &str) -> &'static [&'static str] {
    if rel.starts_with("testing/") {
        &["testing", "sources"]
    } else if rel.starts_with("test/") {
        &["test", "sources"]
    } else if rel.ends_with(".proto") {
        &["proto_sources"]
    } else {
        &["sources"]
    }
}

// ---------------------------------------------------------------------------
// The edits
// ---------------------------------------------------------------------------

/// One change to a package's source lists.
enum Change {
    Added { rule: &'static str, field: &'static [&'static str], rel: String },
    Renamed { from: String, to: String },
    Dropped { rel: String },
}

/// The `WorkspaceEdit` under construction.
#[derive(Default)]
struct Edits {
    /// Each build file this touches, as it was and as the changes so far leave
    /// it. Several operations may land in one package, and two whole-file edits
    /// over one file are two edits a client cannot apply.
    build_files: BTreeMap<PathBuf, (String, String)>,
    /// The import rewrites, by URI.
    imports: BTreeMap<String, Vec<Value>>,
}

impl Edits {
    fn finish(self) -> Option<Value> {
        let mut changes = self.imports;
        for (path, (before, after)) in self.build_files {
            if before == after {
                continue;
            }
            changes.entry(convert::uri_of(&path)).or_default().push(Value::object(vec![
                ("range", super::whole(&before)),
                ("newText", Value::str(after)),
            ]));
        }
        if changes.is_empty() {
            return None;
        }
        Some(Value::object(vec![(
            "changes",
            Value::Object(changes.into_iter().map(|(k, v)| (k, Value::Array(v))).collect()),
        )]))
    }
}

/// Applies one change to a package's `BUILD.buri`, through the buffer the
/// editor is holding for it when there is one.
fn sources(state: &State, edits: &mut Edits, build_path: &Path, change: &Change) {
    if !edits.build_files.contains_key(build_path) {
        let Some(text) = state.text_of(build_path) else { return };
        edits.build_files.insert(build_path.to_path_buf(), (text.clone(), text));
    }
    let Some((_, current)) = edits.build_files.get_mut(build_path) else { return };
    if let Some(next) = rewrite(current, change) {
        *current = next;
    }
}

/// The build file with one source entry added, renamed or dropped.
///
/// Reprinted whole rather than spliced: a list entry is surrounded by
/// punctuation this would have to reason about, and `textproto::print` is what
/// `buri format` and `buri gen` both leave behind — so the file this hands back
/// is one neither of them would change again.
fn rewrite(text: &str, change: &Change) -> Option<String> {
    let parsed = textproto::parse(text, FileId(0));
    if !parsed.errors.is_empty() {
        return None;
    }
    let mut document = parsed.document;
    let touched = match change {
        Change::Added { rule, field, rel } => {
            set(&mut document, rule, field, |list| list.insert(rel.clone()))
        }
        Change::Renamed { from, to } => match holder(&document, from) {
            Some((rule, field)) => set(&mut document, rule, field, |list| {
                list.remove(from);
                list.insert(to.clone())
            }),
            None => false,
        },
        Change::Dropped { rel } => match holder(&document, rel) {
            Some((rule, field)) => set(&mut document, rule, field, |list| list.remove(rel)),
            None => false,
        },
    };
    if !touched {
        return None;
    }
    let printed = textproto::print(&document);
    (printed != text).then_some(printed)
}

/// The rule and field that already list `rel`, if any do.
fn holder(document: &Document, rel: &str) -> Option<(&'static str, &'static [&'static str])> {
    FIELDS
        .into_iter()
        .find(|(rule, field)| regenerate::listed_at(document, rule, field).contains(rel))
}

/// Reads one managed list, lets the caller change it, and writes it back
/// sorted — which is the order `buri gen` leaves one in, so an edit from here
/// and a `gen` afterwards agree.
fn set(
    document: &mut Document,
    rule: &str,
    field: &[&str],
    change: impl FnOnce(&mut BTreeSet<String>) -> bool,
) -> bool {
    if document.get(rule).is_none() {
        return false;
    }
    let mut list = regenerate::listed_at(document, rule, field);
    if !change(&mut list) {
        return false;
    }
    let values: Vec<String> = list.into_iter().collect();
    regenerate::replace_list(document, rule, field, &values);
    true
}

/// Every import and re-export in every open repository that names the module
/// being renamed, rewritten to its new path.
///
/// Resolved rather than string-matched: what makes an import one of these is
/// that it names *this file*, and a module path has one spelling but a file has
/// several ways of being named — `//lib/money` is `lib.buri`, and a rename of
/// that file changes a path that never contained the file's name.
fn imports(state: &mut State, renamed: &Path, to: &str, edits: &mut Edits) {
    for root in state.roots.clone() {
        let Some(analyzed) = state.analyze_workspace(&root) else { continue };
        for module in &analyzed.analysis.loaded.modules {
            let file = analyzed.session.map.get(module.file);
            // A module generated from a schema has a `.proto` on disk and Buri
            // source in hand; there is nothing in that file to edit.
            if file.abs_path.extension().is_none_or(|x| x != "buri") {
                continue;
            }
            let uri = convert::uri_of(&file.abs_path);
            for item in &module.ast.items {
                let (written, span) = match item {
                    Item::Import(i) => (&i.path, i.path_span),
                    Item::ReExport(r) => (&r.path, r.path_span),
                    _ => continue,
                };
                let Ok(resolved) = analyzed.session.workspace.resolve_module(written) else {
                    continue;
                };
                if resolved.in_package().is_none_or(|m| m.file != renamed) {
                    continue;
                }
                let (start, end) = inside_quotes(&file.text, span);
                edits.imports.entry(uri.clone()).or_default().push(Value::object(vec![
                    (
                        "range",
                        Value::object(vec![
                            ("start", convert::position_of(&file.text, start).to_json()),
                            ("end", convert::position_of(&file.text, end).to_json()),
                        ]),
                    ),
                    ("newText", Value::str(to)),
                ]));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/// The paths a `CreateFilesParams` or a `DeleteFilesParams` names.
fn named(params: &Value, field: &str) -> Vec<PathBuf> {
    let Some(items) = params.get("files").and_then(|f| f.as_array()) else { return Vec::new() };
    items
        .iter()
        .filter_map(|f| f.get(field))
        .filter_map(|u| u.as_str())
        .filter_map(convert::path_of)
        .collect()
}

/// The old-and-new pairs a `RenameFilesParams` names.
fn pairs(params: &Value) -> Vec<(PathBuf, PathBuf)> {
    let Some(items) = params.get("files").and_then(|f| f.as_array()) else { return Vec::new() };
    items
        .iter()
        .filter_map(|f| {
            let old = f.get("oldUri").and_then(|u| u.as_str()).and_then(convert::path_of)?;
            let new = f.get("newUri").and_then(|u| u.as_str()).and_then(convert::path_of)?;
            Some((old, new))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The inverse of module resolution, at each of the four shapes a package
    /// module has — all one shape now — and at a file that is not a module.
    #[test]
    fn a_file_is_named_by_the_path_an_import_writes() {
        assert_eq!(module_path("lib/money", "lib.buri").as_deref(), Some("//lib/money/lib.buri"));
        assert_eq!(
            module_path("lib/money", "cents.buri").as_deref(),
            Some("//lib/money/cents.buri")
        );
        assert_eq!(module_path("lib/money", "main.buri").as_deref(), Some("//lib/money/main.buri"));
        assert_eq!(
            module_path("lib/money", "testing/lib.buri").as_deref(),
            Some("//lib/money/testing/lib.buri")
        );
        assert_eq!(
            module_path("lib/money", "shop.proto").as_deref(),
            Some("//lib/money/shop.proto")
        );
        // A package at the repository root: its surface is `//lib.buri`, which
        // under the old spelling was the one module with no path at all.
        assert_eq!(module_path("", "lib.buri").as_deref(), Some("//lib.buri"));
        assert_eq!(module_path("lib/money", "notes.txt"), None);
    }

    /// A package with both rules is where `buri gen` refuses to place a new
    /// file, and this refuses in the same place for the same reason.
    #[test]
    fn a_new_file_is_placed_only_where_there_is_nothing_to_decide() {
        let place = |rel: &str, library: bool, binary: bool| Placement {
            build_path: PathBuf::new(),
            rel: rel.to_string(),
            module: None,
            has_library: library,
            has_binary: binary,
        };
        assert_eq!(destination(&place("cents.buri", true, false)).map(|(r, _)| r), Some("library"));
        assert_eq!(destination(&place("cents.buri", false, true)).map(|(r, _)| r), Some("binary"));
        assert!(destination(&place("cents.buri", true, true)).is_none());
        assert!(destination(&place("lib.buri", true, false)).is_none());
        assert_eq!(
            destination(&place("testing/help.buri", true, true)).map(|(_, f)| f),
            Some(&["testing", "sources"][..])
        );
        assert_eq!(
            destination(&place("shop.proto", true, false)).map(|(_, f)| f),
            Some(&["proto_sources"][..])
        );
    }
}
