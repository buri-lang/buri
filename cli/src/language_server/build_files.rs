//! Go-to-definition inside a `BUILD.buri` or a `REPO.buri`.
//!
//! These are textproto, not Buri, so the analysis every other position request
//! reads has nothing to say about one — `driver::analyze` never opens a build
//! file. What a build file has instead is the build graph, and the strings in
//! it that name something name something the graph or the disk already knows:
//! a dependency label is a package, a source entry is a file beside it, and a
//! tag is a block in `REPO.buri`.
//!
//! Every jump lands at the top of the file it names. A label names a package
//! rather than a line, and choosing a rule inside its build file would be
//! answering a question the label does not ask.

use crate::build::session::Session;
use crate::build::textproto::{self, Field, Value as Node};
use crate::diagnostics::FileId;
use crate::json::Value;
use std::path::{Path, PathBuf};
use super::convert::{self, Position};

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
        "sources" | "proto_sources" | "data" => file_target(path, entry),
        // `tags` under a rule and `tags` under `forbids` both name a tag.
        "tags" => tag_target(session, path, entry),
        _ => None,
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
    Some(convert::top_of(&file))
}

/// A source or data entry is a path relative to the package's own directory,
/// which is the directory this build file is in.
fn file_target(build_file: &Path, entry: &str) -> Option<Value> {
    let file = build_file.parent()?.join(entry);
    // A declared file that is not there is a build error somebody else
    // reports; there is still nowhere to send the editor.
    file.is_file().then(|| convert::top_of(&file))
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
