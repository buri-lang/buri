//! Everything answered inside a `BUILD.buri` or a `REPO.buri`.
//!
//! These are textproto, not Buri, so the analysis every other position request
//! reads has nothing to say about one — `driver::analyze` never opens a build
//! file. What a build file has instead is two things nothing else has: the
//! build graph, and a normative schema. The strings that name something name
//! something the graph or the disk already knows — a dependency label is a
//! package, a source entry is a file beside it, a tag is a block in
//! `REPO.buri` — and every field name in the file is declared and documented
//! in `docs/reference/schema/build.proto`, which [`super::schema`] reads.
//!
//! So this one module answers four requests: definition, links, completion and
//! hover. Each jump lands at the top of the file it names — a label names a
//! package rather than a line, and choosing a rule inside its build file would
//! be answering a question the label does not ask.
//!
//! Diagnostics are here too, and for the plainest reason: a build file is read
//! by `textproto::parse` and by nothing else.

use crate::build::session::Session;
use crate::build::textproto::{self, Field, Value as Node};
use crate::diagnostics::{FileId, Span};
use crate::json::Value;
use std::path::{Path, PathBuf};
use super::convert::{self, Position};
use super::features::Markup;

/// Whether this is a file this module answers for, by the toolchain's own
/// rule: a build file is known by its name.
pub fn is_build_file(path: &Path) -> bool {
    matches!(path.file_name().and_then(|n| n.to_str()), Some("BUILD.buri" | "REPO.buri"))
}

/// What the string under the cursor names, if it names anything.
pub fn definition(
    session: &Session,
    path: &Path,
    text: &str,
    position: Position,
) -> Option<Value> {
    let offset = convert::offset_of(text, position);
    // Parsed from the buffer rather than taken from the workspace's own copy:
    // the workspace read this file from disk, and the offsets a request
    // carries are the editor's.
    let parsed = textproto::parse(text, FileId(0));
    let (field, entry) = string_at(&parsed.document.fields, offset)?;
    match field {
        // `visibility` is a label field too, and the entries in it that are not
        // packages — `//visibility:public` — simply name no package.
        "dependencies" | "visibility" => label_target(session, path, entry),
        "sources" | "proto_sources" => file_target(path, entry),
        // `tags` under a rule and `tags` under `forbids` both name a tag.
        "tags" => tag_target(session, path, entry),
        _ => None,
    }
}

/// Every string in a build file that names a file, underlined.
///
/// The same two producers `definition` answers with, run over all of them
/// instead of over the one under the cursor: a `sources` or `proto_sources`
/// entry is a file beside this one, and a dependency label is a package, which
/// is its `BUILD.buri`.
///
/// A `tags` entry is deliberately not a link. What a tag names is a block
/// inside `REPO.buri` — a line rather than a file — and a `DocumentLink` has
/// nowhere to put a line, so a link for one would land at the top of a file
/// and claim to have found the tag. `definition` still answers for it.
pub fn links(session: &Session, path: &Path, text: &str) -> Value {
    let parsed = textproto::parse(text, FileId(0));
    let mut found = Vec::new();
    strings(&parsed.document.fields, &mut |field, entry, span| {
        let target = match field {
            // `//visibility:public` is in a label field and names no package,
            // so it resolves to nothing and gets no underline.
            "dependencies" | "visibility" => label_path(session, path, entry),
            "sources" | "proto_sources" => file_path(path, entry),
            _ => None,
        };
        if let Some(target) = target {
            let (start, end) = super::links::inside_quotes(text, span);
            found.push((start, end, convert::uri_of(&target)));
        }
    });
    super::links::render(text, found)
}

/// Every string the document writes, with the name of the field holding it.
fn strings(fields: &[Field], out: &mut impl FnMut(&str, &str, crate::diagnostics::Span)) {
    for field in fields {
        match &field.value {
            Node::Str(s, span) => out(&field.name, s, *span),
            Node::Message(m, _) => strings(&m.fields, out),
            Node::List(items, _) => {
                for item in items {
                    match item {
                        Node::Str(s, span) => out(&field.name, s, *span),
                        Node::Message(m, _) => strings(&m.fields, out),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

/// The repository root, spelled the way the editor spells it.
///
/// A `sources` entry is a path beside the build file the request named, so it
/// answers in the client's own names whatever they are. A label went through
/// the workspace, whose root came from the process working directory — and
/// `getcwd` resolves symbolic links, so a repository reached through one had
/// every label answering with a path the editor never asked about while every
/// source went on working. Walking up from the file the request named keeps
/// the two halves in one namespace.
fn root_of(session: &Session, build_file: &Path) -> PathBuf {
    crate::build::workspace::find_root(build_file).unwrap_or_else(|| session.root.clone())
}

/// The string the offset is inside, and the name of the field holding it.
///
/// One field covers an offset, so the walk descends rather than searching: a
/// list is looked through for the entry, a block for the field.
fn string_at(fields: &[Field], offset: u32) -> Option<(&str, &str)> {
    let field = fields.iter().find(|f| covers(f.span, offset))?;
    match &field.value {
        Node::Str(s, span) if covers(*span, offset) => Some((&field.name, s)),
        Node::Message(m, _) => string_at(&m.fields, offset),
        Node::List(items, _) => items.iter().find(|i| covers(i.span(), offset)).and_then(|i| {
            match i {
                Node::Str(s, _) => Some((field.name.as_str(), s.as_str())),
                // `outputs: [{ platform: JS }]` — a list of blocks.
                Node::Message(m, _) => string_at(&m.fields, offset),
                _ => None,
            }
        }),
        _ => None,
    }
}

fn covers(span: crate::diagnostics::Span, offset: u32) -> bool {
    span.start <= offset && offset <= span.end
}

/// A dependency label names a package, and a package is its `BUILD.buri`.
/// `//lib/money/testing` is a surface of the same package, declared in the
/// same file, so the two labels answer alike.
///
/// The whole label is tried before the surface is stripped off it: a package
/// may be written at `lib/testing`, and stripping first sent every label under
/// such a directory to the package above — or, where there was none, nowhere
/// at all.
fn label_target(session: &Session, build_file: &Path, label: &str) -> Option<Value> {
    Some(convert::top_of(&label_path(session, build_file, label)?))
}

fn label_path(session: &Session, build_file: &Path, label: &str) -> Option<PathBuf> {
    let path = label.strip_prefix("//")?;
    let id = session
        .workspace
        .package_by_path(path)
        .or_else(|| session.workspace.package_by_path(path.strip_suffix("/testing")?))?;
    let package = &session.workspace.package(id).path;
    let mut file = root_of(session, build_file);
    if !package.is_empty() {
        file.push(package);
    }
    file.push("BUILD.buri");
    Some(file)
}

/// A source entry is a path relative to the package's own directory, which is
/// the directory this build file is in.
fn file_target(build_file: &Path, entry: &str) -> Option<Value> {
    Some(convert::top_of(&file_path(build_file, entry)?))
}

fn file_path(build_file: &Path, entry: &str) -> Option<PathBuf> {
    let file = build_file.parent()?.join(entry);
    // A declared file that is not there is a build error somebody else
    // reports; there is still nowhere to send the editor.
    file.is_file().then_some(file)
}

/// A tag is declared once, in `REPO.buri`, and named from wherever it applies.
/// This is the one jump that lands on a name rather than on a file, because a
/// tag block is a name.
fn tag_target(session: &Session, build_file: &Path, name: &str) -> Option<Value> {
    let span = session.workspace.repo.tag(name)?.name.span;
    if span.is_none() {
        return None;
    }
    let file = session.map.get(span.file);
    if file.abs_path.as_os_str().is_empty() {
        return None;
    }
    Some(Value::object(vec![
        ("uri", Value::str(convert::uri_of(&root_of(session, build_file).join("REPO.buri")))),
        ("range", convert::range(&file.text, span)),
    ]))
}

// ---------------------------------------------------------------------------
// Completion and hover
// ---------------------------------------------------------------------------

/// What the cursor is in, worked out by reading the file up to it.
///
/// Not `textproto::parse`: a build file being typed into is a build file with
/// an unclosed brace in it, and a parse of that has no node where the cursor
/// is. Reading forwards to the cursor is what still answers — the block stack,
/// the field whose value this is, and what has already been written in the
/// block are all things the text before the cursor says outright.
struct Cursor {
    /// The block the cursor is inside, named by the field that opened it. The
    /// empty name is the file itself.
    block: String,
    /// The fields already written in that block.
    written: Vec<String>,
    /// The field whose value the cursor is in, if it is in one.
    field: Option<String>,
    /// Whether the cursor is inside a quoted string.
    quoted: bool,
    /// What has been typed of the entry so far, and where it starts.
    prefix: String,
    from: u32,
}

/// Reads the file up to `offset`.
fn cursor_at(text: &str, offset: u32) -> Cursor {
    let bytes = text.as_bytes();
    let end = (offset as usize).min(bytes.len());
    // Innermost last. The file itself is the outermost block, and it is never
    // popped.
    let mut blocks: Vec<(String, Vec<String>)> = vec![(String::new(), Vec::new())];
    let mut word: Option<String> = None;
    let mut field: Option<String> = None;
    let mut quoted_from: Option<usize> = None;
    let mut lists: usize = 0;
    let mut i = 0;
    while i < end {
        let Some(c) = bytes.get(i).copied() else { break };
        if quoted_from.is_some() {
            match c {
                b'\\' => i = i.saturating_add(1),
                b'"' => quoted_from = None,
                _ => {}
            }
            i = i.saturating_add(1);
            continue;
        }
        match c {
            // A comment runs to the end of the line, and this is the reader
            // that knows it.
            b'#' => {
                while i < end && bytes.get(i).is_some_and(|c| *c != b'\n') {
                    i = i.saturating_add(1);
                }
            }
            b'"' => {
                quoted_from = Some(i.saturating_add(1));
                i = i.saturating_add(1);
            }
            b'{' => {
                // `library {` names the block with the word before it, and
                // `outputs: [{` with the field, the `:` having taken that word.
                let name = word.take().or_else(|| field.take()).unwrap_or_default();
                if let Some(outer) = blocks.last_mut() {
                    outer.1.push(name.clone());
                }
                blocks.push((name, Vec::new()));
                field = None;
                i = i.saturating_add(1);
            }
            b'}' => {
                if blocks.len() > 1 {
                    blocks.pop();
                }
                field = None;
                word = None;
                i = i.saturating_add(1);
            }
            b'[' => {
                lists = lists.saturating_add(1);
                i = i.saturating_add(1);
            }
            b']' => {
                lists = lists.saturating_sub(1);
                field = None;
                word = None;
                i = i.saturating_add(1);
            }
            b':' => {
                if let Some(name) = word.take() {
                    if let Some(block) = blocks.last_mut() {
                        block.1.push(name.clone());
                    }
                    field = Some(name);
                }
                i = i.saturating_add(1);
            }
            // A newline ends a field's value, unless a list is holding it open
            // across lines.
            b'\n' | b',' => {
                if lists == 0 {
                    field = None;
                }
                word = None;
                i = i.saturating_add(1);
            }
            _ if is_word(c) => {
                let start = i;
                while i < end && bytes.get(i).is_some_and(|c| is_word(*c)) {
                    i = i.saturating_add(1);
                }
                word = text.get(start..i).map(str::to_string);
            }
            _ => i = i.saturating_add(1),
        }
    }
    // What is being typed: the string so far when the cursor is in one, and
    // the word so far otherwise.
    let (prefix, from) = match quoted_from {
        Some(start) => (text.get(start..end).unwrap_or("").to_string(), start as u32),
        None => {
            let before = text.get(..end).unwrap_or("");
            let start = before
                .char_indices()
                .rev()
                .take_while(|(_, c)| c.is_ascii() && is_word(*c as u8))
                .last()
                .map_or(before.len(), |(i, _)| i);
            (before.get(start..).unwrap_or("").to_string(), start as u32)
        }
    };
    let (block, written) = blocks.pop().unwrap_or_default();
    Cursor { block, written, field, quoted: quoted_from.is_some(), prefix, from }
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// One row an editor can show: what to write, what kind of thing it is, the
/// declaration behind it, and the prose written for it.
type Entry = (String, i64, String, Vec<String>);

/// What could be written where the cursor is in a `BUILD.buri` or a
/// `REPO.buri`.
///
/// Three kinds of thing are written in one of these files and each has one
/// source of truth: a field name and an enum constant come from the schema,
/// and a string names something the repository holds — a package, a file
/// beside this one, or a tag `REPO.buri` declares. Nothing here is a list
/// written out by hand.
pub fn completion(session: &Session, path: &Path, text: &str, position: Position) -> Value {
    let cursor = cursor_at(text, convert::offset_of(text, position));
    let replacing = (cursor.from, cursor.from.saturating_add(cursor.prefix.len() as u32));
    let schema = super::schema::schema();
    let entries: Vec<Entry> = match (&cursor.field, cursor.quoted) {
        // A string entry: what it names depends on the field holding it.
        (Some(field), true) => match field.as_str() {
            "dependencies" => labels(session),
            "visibility" => visibilities(session),
            "sources" | "proto_sources" => files(path, text, field),
            "tags" => tags(session),
            _ => Vec::new(),
        },
        // An unquoted value: an enum constant, or a `true`/`false`.
        (Some(field), false) => constants(schema, &cursor.block, field),
        // No field yet, so a field name — the schema's list for this block,
        // less the ones already written that may be written only once.
        (None, _) => fields(schema, &cursor),
    };
    Value::Array(
        entries
            .iter()
            .filter(|(label, ..)| label.starts_with(&cursor.prefix))
            .enumerate()
            .map(|(rank, (label, kind, detail, docs))| {
                // The schema's own order, kept: `sources` before
                // `dependencies` is how a build file is read, and alphabetical
                // order would be a different file every time.
                let rank = char::from_digit((rank % 10) as u32, 10).unwrap_or('9');
                let mut row =
                    super::completion::item(label, *kind, detail, rank, text, replacing, Value::Null);
                // The prose is compiled into this binary already, so there is
                // nothing for a `completionItem/resolve` to fetch and no reason
                // to withhold it.
                if let (Value::Object(fields), false) = (&mut row, docs.is_empty()) {
                    fields.insert(
                        "documentation".to_string(),
                        Value::object(vec![
                            ("kind", Value::str("markdown")),
                            ("value", Value::str(docs.join("\n"))),
                        ]),
                    );
                }
                row
            })
            .collect(),
    )
}

/// The fields this block may still be given.
fn fields(schema: &super::schema::Schema, cursor: &Cursor) -> Vec<Entry> {
    textproto::schema_order(&cursor.block)
        .iter()
        .filter_map(|name| schema.field(&cursor.block, name))
        .filter(|f| f.repeated || !cursor.written.contains(&f.name))
        // 5 field.
        .map(|f| (f.name.clone(), 5, f.signature.clone(), f.docs.clone()))
        .collect()
}

/// The constants a field's enum declares, or the two a `bool` field takes.
///
/// The zero value is not offered: `PLATFORM_UNSPECIFIED` is what an unset
/// field already means, and writing it is a longer way of writing nothing.
fn constants(schema: &super::schema::Schema, block: &str, field: &str) -> Vec<Entry> {
    if schema.is_boolean(block, field) {
        // 12 value.
        return ["false", "true"]
            .iter()
            .map(|v| ((*v).to_string(), 12, String::new(), Vec::new()))
            .collect();
    }
    let Some(enumeration) = schema.enumeration(block, field) else { return Vec::new() };
    enumeration
        .constants
        .iter()
        .filter(|c| !c.name.ends_with("_UNSPECIFIED"))
        // 20 enum member.
        .map(|c| (c.name.clone(), 20, c.signature.clone(), c.docs.clone()))
        .collect()
}

/// Every package in the repository, as the label a `dependencies` entry
/// writes. A library's testing surface is a second label on the same package,
/// and it is offered wherever that surface exists.
fn labels(session: &Session) -> Vec<Entry> {
    let mut out = Vec::new();
    for package in &session.workspace.packages {
        if !package.has_library() {
            continue;
        }
        // 9 module: a label names a package, which is where a module comes
        // from.
        out.push((package.label(), 9, "library".to_string(), Vec::new()));
        if package.build.library.as_ref().is_some_and(|l| l.testing.is_some()) {
            out.push((
                format!("{}/testing", package.label()),
                9,
                "testing surface".to_string(),
                Vec::new(),
            ));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The visibilities a rule may declare: the two words, the whole repository,
/// and every package by name.
fn visibilities(session: &Session) -> Vec<Entry> {
    // 12 value throughout: none of these names a declaration.
    let mut out = vec![
        ("//visibility:public".to_string(), 12, "every package".to_string(), Vec::new()),
        ("//visibility:private".to_string(), 12, "this package only".to_string(), Vec::new()),
        ("//...".to_string(), 12, "every package in this repository".to_string(), Vec::new()),
    ];
    for package in &session.workspace.packages {
        out.push((package.label(), 12, "one package".to_string(), Vec::new()));
        out.push((
            format!("{}/...", package.label()),
            12,
            "that package and everything under it".to_string(),
            Vec::new(),
        ));
    }
    out
}

/// The files beside this build file that the file does not list yet.
///
/// The same walk `buri gen` derives `sources` from, so what an editor offers
/// and what `gen` would write are one list. `lib.buri` and `main.buri` are
/// left out: those two are a rule's surface and are never written in
/// `sources`.
fn files(build_file: &Path, text: &str, field: &str) -> Vec<Entry> {
    let Some(dir) = build_file.parent() else { return Vec::new() };
    let mut sources = Vec::new();
    let mut schemas = Vec::new();
    crate::build::regenerate::collect(dir, dir, &mut sources, &mut schemas);
    let found = if field == "proto_sources" { schemas } else { sources };
    found
        .into_iter()
        .filter(|name| !matches!(name.as_str(), "lib.buri" | "main.buri" | "REPO.buri"))
        // Already written somewhere in this file, so offering it again would
        // be offering a duplicate the build rejects.
        .filter(|name| !text.contains(&format!("\"{name}\"")))
        // 17 file.
        .map(|name| (name, 17, String::new(), Vec::new()))
        .collect()
}

/// The tag vocabulary `REPO.buri` declares, each with its own `doc` — the
/// sentence written to be read exactly here.
fn tags(session: &Session) -> Vec<Entry> {
    session
        .workspace
        .repo
        .tags
        .iter()
        .map(|t| {
            let docs = if t.doc.is_empty() { Vec::new() } else { vec![t.doc.clone()] };
            // 12 value.
            (t.name.value.clone(), 12, "tag".to_string(), docs)
        })
        .collect()
}

/// What the schema says about the name under the cursor.
///
/// A field name, a block name and an enum constant are the three things a
/// build file writes that the schema documents, and each answers with the
/// declaration it has there plus the prose written above it — the same shape a
/// hover over Buri source has, because it is rendered by the same function.
pub fn hover(text: &str, position: Position, kind: Markup) -> Option<Value> {
    let offset = convert::offset_of(text, position);
    let (word, from, to) = word_at(text, offset)?;
    let cursor = cursor_at(text, from);
    if cursor.quoted {
        return None;
    }
    let schema = super::schema::schema();
    let span = Span::new(FileId(0), from as usize, to as usize);
    let (signature, docs) = match &cursor.field {
        // A value: the constant its enum declares.
        Some(field) => {
            let enumeration = schema.enumeration(&cursor.block, field)?;
            let constant = enumeration.constants.iter().find(|c| c.name == word)?;
            (constant.signature.clone(), constant.docs.clone())
        }
        // A name in field position. A field that opens a block carries two
        // pieces of prose — what the field is for, and what the message it
        // holds is — and a reader wants both.
        None => {
            let field = schema.field(&cursor.block, &word)?;
            let mut docs = field.docs.clone();
            if let Some(block) = schema.block(&word) {
                if !block.docs.is_empty() {
                    docs.push(String::new());
                    docs.extend(block.docs.iter().cloned());
                }
            }
            (field.signature.clone(), docs)
        }
    };
    Some(super::features::rendered(text, "proto", &signature, &docs, span, kind))
}

/// The word the offset is on, and its extent.
fn word_at(text: &str, offset: u32) -> Option<(String, u32, u32)> {
    let bytes = text.as_bytes();
    let at = (offset as usize).min(bytes.len());
    let mut start = at;
    while start > 0 && bytes.get(start.saturating_sub(1)).copied().is_some_and(is_word) {
        start = start.saturating_sub(1);
    }
    let mut end = at;
    while bytes.get(end).copied().is_some_and(is_word) {
        end = end.saturating_add(1);
    }
    let word = text.get(start..end)?;
    (!word.is_empty()).then(|| (word.to_string(), start as u32, end as u32))
}

/// Every syntax error a build file holds.
///
/// This is the reader a build file is written against, and running the *Buri*
/// parser over one instead is what made a `# comment` — the comment syntax
/// textproto has and Buri does not — an error on every keystroke.
pub fn diagnostics(text: &str) -> Vec<crate::diagnostics::Diagnostic> {
    textproto::parse(text, FileId(0)).errors
}
