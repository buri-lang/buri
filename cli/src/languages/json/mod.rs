//! The built-in `json`, `jsonc` and `json5` languages: one reader, one
//! formatter and one schema check, told apart by [`Dialect`].

pub mod format;
pub mod number;
pub mod regex;
pub mod schema;
pub mod syntax;
pub mod types;

#[cfg(test)]
mod suite;

pub use syntax::Dialect;

use crate::languages::Finding;
use schema::{has_scheme, is_2020_12, local_path, Registry, SchemaFile};
use std::collections::BTreeMap;
use syntax::{Document, Node};

/// Reads `text`, or says where it stops being `dialect`.
fn read(path: &str, text: &str, dialect: Dialect) -> Result<Document, Finding> {
    syntax::parse(text, dialect).map_err(|e| {
        Finding::new("json-syntax", path, e.span, vec![("problem", e.problem), ("remedy", e.remedy)])
    })
}

/// The canonical text of a file, or `None` when it does not parse.
pub fn format(text: &str, dialect: Dialect) -> Option<String> {
    let doc = syntax::parse(text, dialect).ok()?;
    Some(crate::layout::render(&format::document(&doc, dialect)))
}

/// What a file's `"$schema"` says about where its schema is.
enum Named<'d> {
    /// The file is a schema itself.
    Schema,
    /// A checked-in schema, by repository path.
    File(String, &'d Node),
    /// Something that cannot be checked against; the finding says why.
    Refused(Finding),
}

/// Where a file's schema is. Under a contract, `contract` is the schema's
/// repository path, and the file may omit `"$schema"` or name the same one.
fn named<'d>(path: &str, doc: &'d Document, contract: Option<&str>) -> Named<'d> {
    let root = &doc.root;
    if let Some(contract) = contract {
        let Some(member) = root.member("$schema") else { return Named::File(contract.to_string(), root) };
        let value = &member.value;
        let written = value.as_str().unwrap_or_default();
        let local = match has_scheme(written) {
            true => None,
            false => local_path(path, written.split('#').next().unwrap_or_default()),
        };
        return match local.as_deref() == Some(contract) {
            true => Named::File(contract.to_string(), value),
            false => Named::Refused(Finding::new(
                "schema-mismatch",
                path,
                value.span,
                vec![("schema", written.to_string()), ("contract", format!("//{contract}"))],
            )),
        };
    }
    let Some(member) = root.member("$schema") else {
        let at = (root.span.0, root.span.0.saturating_add(1));
        return Named::Refused(Finding::new("json-without-schema", path, at, Vec::new()));
    };
    let value = &member.value;
    let Some(uri) = value.as_str() else {
        return Named::Refused(Finding::new("schema-not-local", path, value.span, vec![("schema", value.raw.clone())]));
    };
    if is_2020_12(uri) {
        return Named::Schema;
    }
    if uri.contains("json-schema.org") {
        return Named::Refused(Finding::new("json-schema-draft-unsupported", path, value.span, vec![("draft", uri.to_string())]));
    }
    let local = if has_scheme(uri) { None } else { local_path(path, uri.split('#').next().unwrap_or_default()) };
    match local {
        Some(file) => Named::File(file, value),
        None => Named::Refused(Finding::new("schema-not-local", path, value.span, vec![("schema", uri.to_string())])),
    }
}

/// Every schema file a check of `text` reads, by repository path, found by
/// following `"$schema"` and each `$ref` to another file.
///
/// A check reads nothing else, so these and the file itself are the whole of
/// its cache key.
pub fn schema_files(
    path: &str,
    text: &str,
    contract: Option<&str>,
    dialect_of: &dyn Fn(&str) -> Dialect,
    read_file: &mut dyn FnMut(&str) -> Option<String>,
) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let Ok(doc) = syntax::parse(text, dialect_of(path)) else { return files };
    let mut queue: Vec<(String, Document)> = Vec::new();
    match named(path, &doc, contract) {
        Named::Schema => queue.push((path.to_string(), doc)),
        Named::File(schema, _) => {
            if let Some(schema_text) = read_file(&schema) {
                if let Ok(parsed) = syntax::parse(&schema_text, dialect_of(&schema)) {
                    queue.push((schema.clone(), parsed));
                }
                files.insert(schema, schema_text);
            }
        }
        Named::Refused(_) => {}
    }
    while let Some((at, doc)) = queue.pop() {
        let file = SchemaFile { path: at, root: doc.root };
        for (_, target) in schema::file_references(&file) {
            let Some(target) = target else { continue };
            if files.contains_key(&target) || target == path || files.len() >= 100 {
                continue;
            }
            let Some(text) = read_file(&target) else { continue };
            if let Ok(parsed) = syntax::parse(&text, dialect_of(&target)) {
                queue.push((target.clone(), parsed));
            }
            files.insert(target, text);
        }
    }
    files
}

/// Checks one file against its schema. `files` is what [`schema_files`] read.
pub fn check(
    path: &str,
    text: &str,
    contract: Option<&str>,
    dialect_of: &dyn Fn(&str) -> Dialect,
    files: &BTreeMap<String, String>,
) -> Vec<Finding> {
    let doc = match read(path, text, dialect_of(path)) {
        Ok(doc) => doc,
        Err(finding) => return vec![finding],
    };
    let mut findings = Vec::new();
    let mut schemas: Vec<SchemaFile> = Vec::new();
    let (target, instance) = match named(path, &doc, contract) {
        Named::Refused(finding) => return vec![finding],
        Named::Schema => {
            schemas.push(SchemaFile { path: path.to_string(), root: doc.root.clone() });
            (None, None)
        }
        Named::File(schema, value) => {
            if !files.contains_key(&schema) {
                return vec![Finding::new("schema-not-found", path, value.span, vec![("path", schema)])];
            }
            (Some(schema), Some(&doc.root))
        }
    };
    for (file, text) in files {
        match read(file, text, dialect_of(file)) {
            Ok(parsed) => schemas.push(SchemaFile { path: file.clone(), root: parsed.root }),
            Err(finding) => findings.push(finding),
        }
    }
    // The schema a data file names states its own draft.
    if let Some(schema) = schemas.iter().find(|s| Some(&s.path) == target.as_ref()) {
        if let Some(own) = schema.root.get("$schema") {
            if !own.as_str().is_some_and(is_2020_12) {
                findings.push(Finding::new("json-schema-draft-unsupported", &schema.path, own.span, vec![("draft", own.as_str().map_or_else(|| own.raw.clone(), str::to_string))]));
            }
        }
    }
    if !findings.is_empty() {
        return findings;
    }
    let registry = Registry::new(&schemas);
    if !registry.problems().is_empty() {
        return registry.problems().to_vec();
    }
    let (Some(target), Some(instance)) = (target, instance) else { return Vec::new() };
    let Some(index) = schemas.iter().position(|s| s.path == target) else { return Vec::new() };
    schema::validate(&registry, index, instance)
        .into_iter()
        .map(|v| {
            Finding::new(
                "json-schema-violation",
                path,
                v.span,
                vec![("problem", v.problem), ("remedy", v.remedy), ("location", v.location)],
            )
        })
        .collect()
}

/// Every file a schema reaches through `$ref`, itself included: what
/// generating types from it reads.
pub fn schema_closure(
    path: &str,
    dialect_of: &dyn Fn(&str) -> Dialect,
    read_file: &mut dyn FnMut(&str) -> Option<String>,
) -> BTreeMap<String, String> {
    let Some(text) = read_file(path) else { return BTreeMap::new() };
    let mut files = schema_files(path, &text, None, dialect_of, read_file);
    files.insert(path.to_string(), text);
    files
}

/// The parsed files of a schema's closure, or why they are not one schema.
fn schema_set(
    path: &str,
    files: &BTreeMap<String, String>,
    dialect_of: &dyn Fn(&str) -> Dialect,
) -> Result<Vec<SchemaFile>, Vec<Finding>> {
    let Some(text) = files.get(path) else {
        return Err(vec![Finding::new("schema-not-found", path, (0, 0), vec![("path", path.to_string())])]);
    };
    let found = check(path, text, None, dialect_of, files);
    if !found.is_empty() {
        return Err(found);
    }
    let mut out = Vec::new();
    for (file, text) in files {
        let doc = read(file, text, dialect_of(file)).map_err(|f| vec![f])?;
        out.push(SchemaFile { path: file.clone(), root: doc.root });
    }
    Ok(out)
}

/// `std/json`'s `generate` on one input: a schema gives its types, and a data
/// file its schema's types and its contents as a value. `files` is what
/// [`schema_files`] read for it.
pub fn generate(
    path: &str,
    text: &str,
    dialect_of: &dyn Fn(&str) -> Dialect,
    files: &BTreeMap<String, String>,
) -> Result<types::Module, Vec<Finding>> {
    let doc = read(path, text, dialect_of(path)).map_err(|f| vec![f])?;
    match named(path, &doc, None) {
        Named::Refused(finding) => Err(vec![finding]),
        Named::Schema => {
            let mut all = files.clone();
            all.insert(path.to_string(), text.to_string());
            types::schema_module(&schema_set(path, &all, dialect_of)?, path)
        }
        Named::File(schema, _) => types::data_module(&schema_set(&schema, files, dialect_of)?, &schema, path, &doc.root),
    }
}

/// The module a contract's `type_schema` gives a tool: its types and
/// `decode`. `files` is what [`schema_closure`] read.
pub fn contract(
    path: &str,
    dialect_of: &dyn Fn(&str) -> Dialect,
    files: &BTreeMap<String, String>,
) -> Result<types::Module, Vec<Finding>> {
    types::contract_module(&schema_set(path, files, dialect_of)?, path)
}

/// A file's value as strict JSON, for a tool that takes it typed. `None` when
/// it does not parse.
pub fn strict(text: &str, dialect: Dialect) -> Option<String> {
    Some(types::strict(&syntax::parse(text, dialect).ok()?.root))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(path: &str, text: &str, others: &[(&str, &str)]) -> Vec<Finding> {
        let others: BTreeMap<String, String> =
            others.iter().map(|(p, t)| (p.to_string(), t.to_string())).collect();
        let dialect = |_: &str| Dialect::Json;
        let mut read_file = |p: &str| others.get(p).cloned();
        let files = schema_files(path, text, None, &dialect, &mut read_file);
        check(path, text, None, &dialect, &files)
    }

    fn codes(findings: &[Finding]) -> Vec<&str> {
        findings.iter().map(|f| f.code.as_str()).collect()
    }

    const SCHEMA: &str = r##"{
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {
            "$schema": { "type": "string" },
            "name": { "type": "string", "minLength": 2 },
            "port": { "type": "integer", "minimum": 1, "maximum": 65535 },
            "tags": { "type": "array", "items": { "$ref": "#/$defs/tag" }, "uniqueItems": true }
        },
        "required": ["name"],
        "additionalProperties": false,
        "$defs": { "tag": { "type": "string", "pattern": "^[a-z]+$" } }
    }"##;

    #[test]
    fn a_valid_file_has_nothing_to_say() {
        let data = r##"{ "$schema": "app.schema.json", "name": "web", "port": 80, "tags": ["a", "b"] }"##;
        assert_eq!(run("lib/app/app.json", data, &[("lib/app/app.schema.json", SCHEMA)]), vec![]);
    }

    #[test]
    fn each_violation_is_reported_where_it_is() {
        let data = r##"{ "$schema": "//lib/app/app.schema.json", "port": 1.5, "tags": ["a", "B", "a"], "x": 1 }"##;
        let found = run("lib/app/app.json", data, &[("lib/app/app.schema.json", SCHEMA)]);
        assert_eq!(codes(&found), vec!["json-schema-violation"; 5], "{found:#?}");
    }

    #[test]
    fn the_schema_must_be_local_and_2020_12() {
        let remote = r##"{ "$schema": "https://example.com/app.schema.json" }"##;
        assert_eq!(codes(&run("a.json", remote, &[])), vec!["schema-not-local"]);
        let outside = r##"{ "$schema": "../../app.schema.json" }"##;
        assert_eq!(codes(&run("lib/a.json", outside, &[])), vec!["schema-not-local"]);
        let draft = r##"{ "$schema": "http://json-schema.org/draft-07/schema#" }"##;
        assert_eq!(codes(&run("a.json", draft, &[])), vec!["json-schema-draft-unsupported"]);
        let missing = r##"{ "$schema": "nope.json" }"##;
        assert_eq!(codes(&run("a.json", missing, &[])), vec!["schema-not-found"]);
        assert_eq!(codes(&run("a.json", "{}", &[])), vec!["json-without-schema"]);
    }

    #[test]
    fn a_schema_is_read_as_one() {
        let bad = r##"{ "$schema": "https://json-schema.org/draft/2020-12/schema", "items": [{}], "minimum": "x" }"##;
        assert_eq!(codes(&run("s.json", bad, &[])), vec!["json-schema-invalid", "json-schema-invalid"]);
        assert_eq!(run("s.json", SCHEMA, &[]), vec![]);
    }

    #[test]
    fn a_ref_reaches_another_file() {
        let a = r##"{ "$schema": "https://json-schema.org/draft/2020-12/schema", "$ref": "b.json#/$defs/n" }"##;
        let b = r##"{ "$defs": { "n": { "type": "number" } } }"##;
        let data = r##"{ "$schema": "a.json" }"##;
        let found = run("d.json", data, &[("a.json", a), ("b.json", b)]);
        assert_eq!(codes(&found), vec!["json-schema-violation"]);
    }

    #[test]
    fn unevaluated_properties_see_through_all_of() {
        let s = r##"{ "$schema": "https://json-schema.org/draft/2020-12/schema",
            "allOf": [{ "properties": { "$schema": true, "a": true } }],
            "unevaluatedProperties": false }"##;
        assert_eq!(run("d.json", r##"{ "$schema": "s.json", "a": 1 }"##, &[("s.json", s)]), vec![]);
        let found = run("d.json", r##"{ "$schema": "s.json", "b": 1 }"##, &[("s.json", s)]);
        assert_eq!(codes(&found), vec!["json-schema-violation"]);
    }
}
