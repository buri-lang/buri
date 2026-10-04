//! The `json` tool's `generate`: Buri types from a JSON Schema, and a value of them.
//!
//! ```text
//! { "type": "object",                    export struct Config {
//!   "properties": {                          export name: Str,
//!     "name": { "type": "string" },   ->     export port: Option<Int>,
//!     "port": { "type": "integer" } },   }
//!   "required": ["name"] }
//! ```
//!
//! Three modules come out of one mapping:
//!
//! - [`schema_module`]: the types a schema describes.
//! - [`data_module`]: a data file's schema's types, and its contents as an
//!   `export let` named after the file.
//! - [`contract_module`]: the types, and an exported `decode` that reads the
//!   strict JSON [`strict`] writes. The harness a tool with a contract runs
//!   under calls it on each input before the entry point sees it.
//!
//! Keywords that describe values rather than their shape, such as `pattern`,
//! `minimum` and `format`, stay the check's. A keyword that changes the shape
//! in a way no one Buri type follows is refused where it is written
//! (`json-untyped-keyword`).

use super::number::Number;
use super::schema::{Registry, SchemaFile};
use super::syntax::{Node, Range, Value};
use crate::languages::Finding;
use std::collections::{BTreeSet, HashMap};

/// A type as a module spells it.
#[derive(Clone, Debug, PartialEq)]
enum Ty {
    Str,
    Int,
    F64,
    Bool,
    Unit,
    /// Any value at all: `core/json`'s `Json`.
    Any,
    List(Box<Ty>),
    Option(Box<Ty>),
    /// An object of `additionalProperties` alone, as its entries in order.
    Map(Box<Ty>),
    Named(String),
}

impl Ty {
    fn optional(self) -> Ty {
        match self {
            Ty::Option(_) => self,
            other => Ty::Option(Box::new(other)),
        }
    }

    fn spelled(&self) -> String {
        match self {
            Ty::Str => "Str".to_string(),
            Ty::Int => "Int".to_string(),
            Ty::F64 => "F64".to_string(),
            Ty::Bool => "Bool".to_string(),
            Ty::Unit => "()".to_string(),
            Ty::Any => "Json".to_string(),
            Ty::List(t) => format!("[{}]", t.spelled()),
            Ty::Option(t) => format!("Option<{}>", t.spelled()),
            Ty::Map(t) => format!("[(Str, {})]", t.spelled()),
            Ty::Named(n) => n.clone(),
        }
    }

    fn uses_json(&self) -> bool {
        match self {
            Ty::Any => true,
            Ty::List(t) | Ty::Option(t) | Ty::Map(t) => t.uses_json(),
            _ => false,
        }
    }
}

struct Field {
    /// The property's name in the document.
    key: String,
    name: String,
    ty: Ty,
    docs: Vec<String>,
    at: (String, Range),
}

struct Variant {
    /// The string the document writes.
    value: String,
    name: String,
    payload: Option<String>,
}

enum Shape {
    Struct(Vec<Field>),
    Strings(Vec<Variant>),
    Tagged { tag: String, variants: Vec<Variant> },
    Alias(Ty),
}

struct Decl {
    name: String,
    shape: Shape,
    docs: Vec<String>,
    at: (String, Range),
}

/// A generated module: its text, and which region of it came from which span
/// of which schema, as `(start, end, file, span)`.
pub struct Module {
    pub text: String,
    pub anchors: Vec<(usize, usize, String, Range)>,
    /// The name of the root type.
    pub root: String,
}

/// Type names a module already has, so a schema cannot take them.
const RESERVED: &[&str] = &[
    "Str", "Int", "Float", "F32", "F64", "I8", "I16", "I32", "I64", "U8", "U16", "U32", "U64", "Bool",
    "Char", "Option", "Result", "Json", "Allocator", "Self",
];

const KEYWORDS: &[&str] = &[
    "as", "const", "context", "ctx", "derive", "effect", "else", "enum", "export", "false", "fn", "for", "from",
    "if", "impl", "import", "let", "match", "self", "struct", "test", "trait", "true", "type", "async", "await",
    "break", "continue", "do", "in", "is", "loop", "module", "mut", "opaque", "panic", "pub", "return",
    "unreachable", "use", "when", "where", "while", "with", "yield",
];

/// How deeply `$ref`s may nest before the schema is taken to loop.
const MAX_DEPTH: usize = 64;

struct Gen<'a, 'r> {
    reg: &'a Registry<'r>,
    decls: Vec<Decl>,
    named: HashMap<*const Node, String>,
    taken: BTreeSet<String>,
    refused: Vec<Finding>,
    depth: usize,
}

/// `Upper` camel case, from any spelling: `eu-west` is `EuWest`.
fn upper_camel(s: &str) -> String {
    let mut out = String::new();
    for word in s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()) {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.extend(chars);
        }
    }
    match out.chars().next() {
        None => "Value".to_string(),
        Some(c) if c.is_ascii_digit() => format!("V{out}"),
        Some(_) => out,
    }
}

/// `lower` camel case, from any spelling: `max_connections` is
/// `maxConnections`, and a keyword gets a trailing `_`.
fn lower_camel(s: &str) -> String {
    let upper = upper_camel(s);
    let mut chars = upper.chars();
    let mut out: String = chars.next().map(|c| c.to_ascii_lowercase()).into_iter().collect();
    out.extend(chars);
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, 'v');
    }
    if KEYWORDS.contains(&out.as_str()) {
        out.push('_');
    }
    out
}

/// A file's name up to its first `.`: `regions.schema.json` is `regions`.
fn stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.split('.').next().unwrap_or(name)
}

fn docs_of(node: &Node) -> Vec<String> {
    let text = node.get("description").and_then(Node::as_str).unwrap_or_default();
    text.lines().map(|l| l.trim_end().to_string()).collect()
}

/// The keywords with no sound type mapping, and why.
const REFUSED: &[(&str, &str)] = &[
    ("if", "`if`, `then` and `else` make a value's shape depend on its contents"),
    ("then", "`if`, `then` and `else` make a value's shape depend on its contents"),
    ("else", "`if`, `then` and `else` make a value's shape depend on its contents"),
    ("patternProperties", "`patternProperties` gives a property a type by its name's spelling"),
    ("dependentSchemas", "`dependentSchemas` changes an object's shape by which properties it has"),
    ("prefixItems", "`prefixItems` gives each position of an array its own type"),
    ("allOf", "`allOf` merges schemas, and a merge is not one type"),
    ("$dynamicRef", "`$dynamicRef` names a schema only a document's path decides"),
];

impl<'a, 'r> Gen<'a, 'r> {
    fn refuse(&mut self, file: &str, span: Range, keyword: &str, why: &str) {
        self.refused.push(Finding::new(
            "json-untyped-keyword",
            file,
            span,
            vec![("keyword", keyword.to_string()), ("why", why.to_string())],
        ));
    }

    /// A type name nothing else in the module has.
    fn fresh(&mut self, wanted: &str) -> String {
        let base = upper_camel(wanted);
        let mut name = base.clone();
        let mut n = 2usize;
        while self.taken.contains(&name) || RESERVED.contains(&name.as_str()) {
            name = format!("{base}{n}");
            n = n.saturating_add(1);
        }
        self.taken.insert(name.clone());
        name
    }

    fn ty(&mut self, node: &'r Node, resource: usize, hint: &str) -> Ty {
        let resource = self.reg.enter(node, resource);
        let file = self.reg.file_of(resource);
        let members = match &node.value {
            Value::Bool(true) => return Ty::Any,
            Value::Bool(false) => {
                self.refuse(&file, node.span, "false", "`false` matches no value, so there is nothing to type");
                return Ty::Any;
            }
            Value::Object(members, _) => members,
            _ => return Ty::Any,
        };
        if let Some(name) = self.named.get(&(node as *const Node)) {
            return Ty::Named(name.clone());
        }
        for m in members {
            if let Some((_, why)) = REFUSED.iter().find(|(k, _)| *k == m.key) {
                self.refuse(&file, m.key_span, &m.key, why);
                return Ty::Any;
            }
            let schema_valued = matches!(m.value.value, Value::Object(..));
            if m.key.starts_with("unevaluated") && schema_valued {
                self.refuse(&file, m.key_span, &m.key, "a schema for the leftover members gives them a type no field or element has");
                return Ty::Any;
            }
            if m.key == "additionalProperties" && schema_valued && node.get("properties").is_some() {
                self.refuse(&file, m.key_span, &m.key, "a schema for the other properties beside `properties` makes an object both a struct and a map");
                return Ty::Any;
            }
        }
        if let Some(reference) = node.get("$ref").and_then(Node::as_str) {
            let Some((target_resource, target)) = self.reg.target(reference, resource) else { return Ty::Any };
            if self.depth >= MAX_DEPTH {
                self.refuse(&file, node.span, "$ref", "the references loop without reaching an object");
                return Ty::Any;
            }
            let fragment = reference.split_once('#').map(|(_, f)| f).unwrap_or_default();
            let name = match fragment.rsplit('/').next().filter(|n| !n.is_empty()) {
                Some(last) => last.to_string(),
                None => stem(&self.reg.file_of(target_resource)).to_string(),
            };
            self.depth = self.depth.saturating_add(1);
            let ty = self.ty(target, target_resource, &name);
            self.depth = self.depth.saturating_sub(1);
            return ty;
        }
        for keyword in ["oneOf", "anyOf"] {
            let Some(m) = node.member(keyword) else { continue };
            let Value::Array(items, _) = &m.value.value else { continue };
            let is_null = |n: &Node| n.get("type").and_then(Node::as_str) == Some("null");
            if let [a, b] = items.as_slice() {
                if is_null(a) != is_null(b) {
                    let other = if is_null(a) { b } else { a };
                    return self.ty(other, resource, hint).optional();
                }
            }
            if keyword == "oneOf" {
                if let Some(ty) = self.tagged(node, resource, items, hint) {
                    return ty;
                }
                self.refuse(&file, m.key_span, keyword, "a `oneOf` is typed only when every branch is an object with one required property whose `const` string names it");
            } else {
                self.refuse(&file, m.key_span, keyword, "an `anyOf` has no tag saying which branch a value is");
            }
            return Ty::Any;
        }
        if let Some(e) = node.member("enum") {
            let Value::Array(items, _) = &e.value.value else { return Ty::Any };
            let nullable = items.iter().any(|i| matches!(i.value, Value::Null));
            let strings: Vec<&str> = items.iter().filter_map(Node::as_str).collect();
            let others = items.len().saturating_sub(strings.len()).saturating_sub(usize::from(nullable));
            if strings.is_empty() || others > 0 {
                self.refuse(&file, e.key_span, "enum", "only an `enum` of strings is a Buri enum");
                return Ty::Any;
            }
            let name = self.fresh(node.get("title").and_then(Node::as_str).unwrap_or(hint));
            self.named.insert(node as *const Node, name.clone());
            let mut seen = BTreeSet::new();
            let mut variants = Vec::new();
            for s in strings {
                let variant = upper_camel(s);
                if !seen.insert(variant.clone()) {
                    self.refuse(&file, e.value.span, "enum", &format!("two values are both the variant `{variant}`"));
                    continue;
                }
                variants.push(Variant { value: s.to_string(), name: variant, payload: None });
            }
            self.decls.push(Decl { name: name.clone(), shape: Shape::Strings(variants), docs: docs_of(node), at: (file, node.span) });
            let ty = Ty::Named(name);
            return if nullable { ty.optional() } else { ty };
        }
        if let Some(c) = node.get("const") {
            return scalar_of(c);
        }
        let mut types: Vec<&str> = match node.get("type").map(|t| &t.value) {
            Some(Value::Str(s)) => vec![s.as_str()],
            Some(Value::Array(items, _)) => items.iter().filter_map(Node::as_str).collect(),
            _ => {
                if ["properties", "additionalProperties", "required"].iter().any(|k| node.get(k).is_some()) {
                    vec!["object"]
                } else if node.get("items").is_some() {
                    vec!["array"]
                } else {
                    Vec::new()
                }
            }
        };
        let nullable = types.contains(&"null");
        types.retain(|t| *t != "null");
        if types.len() == 2 && types.contains(&"integer") && types.contains(&"number") {
            types = vec!["number"];
        }
        let base = match types.as_slice() {
            [] if nullable => return Ty::Unit,
            [] => return Ty::Any,
            ["string"] => Ty::Str,
            ["integer"] => Ty::Int,
            ["number"] => Ty::F64,
            ["boolean"] => Ty::Bool,
            ["array"] => match node.get("items") {
                Some(items) => Ty::List(Box::new(self.ty(items, resource, hint))),
                None => Ty::List(Box::new(Ty::Any)),
            },
            ["object"] => self.object(node, resource, hint),
            _ => {
                let span = node.member("type").map_or(node.span, |m| m.key_span);
                self.refuse(&file, span, "type", "a value of several types has no one Buri type");
                return Ty::Any;
            }
        };
        if nullable { base.optional() } else { base }
    }

    fn object(&mut self, node: &'r Node, resource: usize, hint: &str) -> Ty {
        let file = self.reg.file_of(resource);
        let Some(Value::Object(properties, _)) = node.get("properties").map(|p| &p.value) else {
            return match node.get("additionalProperties") {
                Some(schema @ Node { value: Value::Object(..), .. }) => Ty::Map(Box::new(self.ty(schema, resource, hint))),
                _ => Ty::Any,
            };
        };
        let name = self.fresh(node.get("title").and_then(Node::as_str).unwrap_or(hint));
        self.named.insert(node as *const Node, name.clone());
        // Placed before its fields are typed, so the root comes first.
        let index = self.decls.len();
        self.decls.push(Decl { name: name.clone(), shape: Shape::Struct(Vec::new()), docs: docs_of(node), at: (file.clone(), node.span) });
        let required: BTreeSet<&str> = match node.get("required").map(|r| &r.value) {
            Some(Value::Array(items, _)) => items.iter().filter_map(Node::as_str).collect(),
            _ => BTreeSet::new(),
        };
        let mut fields: Vec<Field> = Vec::new();
        for m in properties {
            // `"$schema"` says where a file's schema is, and a property with
            // one value says nothing: neither is a field.
            if m.key == "$schema" || m.value.get("const").is_some() || matches!(m.value.value, Value::Bool(false)) {
                continue;
            }
            let field = lower_camel(&m.key);
            if let Some(other) = fields.iter().find(|f| f.name == field) {
                let why = format!("`{}` and `{}` are both the field `{field}`", other.key, m.key);
                self.refuse(&file, m.key_span, "properties", &why);
                continue;
            }
            let ty = self.ty(&m.value, resource, &format!("{name}{}", upper_camel(&m.key)));
            let ty = if required.contains(m.key.as_str()) { ty } else { ty.optional() };
            fields.push(Field { key: m.key.clone(), name: field, ty, docs: docs_of(&m.value), at: (file.clone(), m.key_span) });
        }
        if let Some(decl) = self.decls.get_mut(index) {
            decl.shape = Shape::Struct(fields);
        }
        Ty::Named(name)
    }

    /// A `oneOf` whose branches are objects told apart by one required
    /// property's `const` string: an enum, a variant per branch.
    fn tagged(&mut self, node: &'r Node, resource: usize, items: &'r [Node], hint: &str) -> Option<Ty> {
        let mut branches: Vec<(&'r Node, usize, Option<String>)> = Vec::new();
        for item in items {
            let r = self.reg.enter(item, resource);
            match item.get("$ref").and_then(Node::as_str) {
                Some(reference) => {
                    let (tr, target) = self.reg.target(reference, r)?;
                    let fragment = reference.split_once('#').map(|(_, f)| f).unwrap_or_default();
                    let name = fragment.rsplit('/').next().filter(|n| !n.is_empty()).map(str::to_string);
                    branches.push((target, tr, name));
                }
                None => branches.push((item, r, None)),
            }
        }
        let tag_of = |branch: &Node, key: &str| -> Option<String> {
            let required = match branch.get("required").map(|r| &r.value) {
                Some(Value::Array(items, _)) => items.iter().any(|i| i.as_str() == Some(key)),
                _ => false,
            };
            let value = branch.get("properties")?.get(key)?.get("const")?.as_str()?;
            required.then(|| value.to_string())
        };
        let first = branches.first()?.0;
        let Some(Value::Object(candidates, _)) = first.get("properties").map(|p| &p.value) else { return None };
        let tag = candidates.iter().map(|m| m.key.clone()).find(|key| {
            let values: Vec<Option<String>> = branches.iter().map(|(b, _, _)| tag_of(b, key)).collect();
            let unique: BTreeSet<&Option<String>> = values.iter().collect();
            values.iter().all(Option::is_some) && unique.len() == values.len()
        })?;
        let file = self.reg.file_of(resource);
        let name = self.fresh(node.get("title").and_then(Node::as_str).unwrap_or(hint));
        self.named.insert(node as *const Node, name.clone());
        let index = self.decls.len();
        self.decls.push(Decl { name: name.clone(), shape: Shape::Strings(Vec::new()), docs: docs_of(node), at: (file.clone(), node.span) });
        let mut variants = Vec::new();
        let mut seen = BTreeSet::new();
        for (branch, r, ref_name) in branches {
            let value = tag_of(branch, &tag).unwrap_or_default();
            let variant = upper_camel(&value);
            if !seen.insert(variant.clone()) {
                self.refuse(&file, node.span, "oneOf", &format!("two branches are both the variant `{variant}`"));
                continue;
            }
            let has_fields = match branch.get("properties").map(|p| &p.value) {
                Some(Value::Object(ms, _)) => ms.iter().any(|m| m.key != "$schema" && m.value.get("const").is_none()),
                _ => false,
            };
            let payload = match has_fields {
                false => None,
                true => {
                    let wanted = ref_name.unwrap_or_else(|| format!("{name}{variant}"));
                    match self.ty(branch, r, &wanted) {
                        Ty::Named(n) => Some(n),
                        _ => None,
                    }
                }
            };
            variants.push(Variant { value, name: variant, payload });
        }
        if let Some(decl) = self.decls.get_mut(index) {
            decl.shape = Shape::Tagged { tag, variants };
        }
        Some(Ty::Named(name))
    }

    fn decl(&self, name: &str) -> Option<&Decl> {
        self.decls.iter().find(|d| d.name == name)
    }
}

fn scalar_of(node: &Node) -> Ty {
    match &node.value {
        Value::Str(_) => Ty::Str,
        Value::Bool(_) => Ty::Bool,
        Value::Null => Ty::Unit,
        Value::Number(Number::Finite(d)) if d.is_integer() => Ty::Int,
        Value::Number(_) => Ty::F64,
        _ => Ty::Any,
    }
}

// ---------------------------------------------------------------------------
// Writing the module
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Writer {
    text: String,
    anchors: Vec<(usize, usize, String, Range)>,
}

impl Writer {
    fn line(&mut self, s: &str) {
        self.text.push_str(s);
        self.text.push('\n');
    }

    /// Writes `s` as one line, anchored to `at`.
    fn anchored(&mut self, s: &str, at: &(String, Range)) {
        let start = self.text.len();
        self.text.push_str(s);
        self.anchors.push((start, self.text.len(), at.0.clone(), at.1));
        self.text.push('\n');
    }

    fn docs(&mut self, docs: &[String], indent: &str) {
        for d in docs {
            let line = if d.is_empty() { format!("{indent}///") } else { format!("{indent}/// {d}") };
            self.line(&line);
        }
    }
}

fn types(gen: &Gen, w: &mut Writer) {
    for decl in &gen.decls {
        let start = w.text.len();
        match &decl.shape {
            Shape::Alias(ty) => {
                w.docs(&decl.docs, "");
                w.line(&format!("export type {} = {};", decl.name, ty.spelled()));
            }
            Shape::Struct(fields) => {
                w.line(&format!("derive Equal, Show for {};", decl.name));
                w.docs(&decl.docs, "");
                if fields.is_empty() {
                    w.line(&format!("export struct {} {{}}", decl.name));
                } else {
                    w.line(&format!("export struct {} {{", decl.name));
                    for f in fields {
                        w.docs(&f.docs, "    ");
                        w.anchored(&format!("    export {}: {},", f.name, f.ty.spelled()), &f.at);
                    }
                    w.line("}");
                }
            }
            Shape::Strings(variants) | Shape::Tagged { variants, .. } => {
                w.line(&format!("derive Equal, Show for {};", decl.name));
                w.docs(&decl.docs, "");
                w.line(&format!("export enum {} {{", decl.name));
                for v in variants {
                    match &v.payload {
                        Some(p) => w.line(&format!("    {}({p}),", v.name)),
                        None => w.line(&format!("    {},", v.name)),
                    }
                }
                w.line("}");
            }
        }
        w.anchors.push((start, w.text.len(), decl.at.0.clone(), decl.at.1));
        w.line("");
    }
}

fn uses_json(gen: &Gen) -> bool {
    gen.decls.iter().any(|d| match &d.shape {
        Shape::Struct(fields) => fields.iter().any(|f| f.ty.uses_json()),
        Shape::Alias(ty) => ty.uses_json(),
        _ => false,
    })
}

/// The files a module is generated from, parsed, with the one the root type
/// comes from first.
fn generator<'a, 'r>(reg: &'a Registry<'r>) -> Gen<'a, 'r> {
    Gen { reg, decls: Vec::new(), named: HashMap::new(), taken: BTreeSet::new(), refused: Vec::new(), depth: 0 }
}

/// The root type of the schema file at `path`, declared.
fn root<'r>(gen: &mut Gen<'_, 'r>, files: &'r [SchemaFile], path: &str) -> String {
    let Some(file) = files.iter().find(|f| f.path == path) else { return String::new() };
    let Some(resource) = gen.reg.resource_of_file(path) else { return String::new() };
    let hint = file.root.get("title").and_then(Node::as_str).unwrap_or(stem(path)).to_string();
    match gen.ty(&file.root, resource, &hint) {
        Ty::Named(name) => name,
        other => {
            let name = gen.fresh(&hint);
            let decl = Decl { name: name.clone(), shape: Shape::Alias(other), docs: docs_of(&file.root), at: (path.to_string(), file.root.span) };
            gen.decls.insert(0, decl);
            name
        }
    }
}

fn header(w: &mut Writer, from: &str) {
    w.line(&format!("//! Generated by the `json` tool from `{from}`."));
    w.line("");
}

/// The types the schema at `path` describes. `files` is it and every file
/// it reaches, already a registry without problems.
pub fn schema_module(files: &[SchemaFile], path: &str) -> Result<Module, Vec<Finding>> {
    let reg = Registry::new(files);
    let mut gen = generator(&reg);
    let root = root(&mut gen, files, path);
    if !gen.refused.is_empty() {
        return Err(gen.refused);
    }
    let mut w = Writer::default();
    header(&mut w, path);
    if uses_json(&gen) {
        w.line("from \"core/json\" import { Json };");
        w.line("");
    }
    types(&gen, &mut w);
    Ok(finish(w, root))
}

/// The types of the data file's schema, and the file's contents as
/// `export let <file>: <Root>`.
pub fn data_module(files: &[SchemaFile], schema: &str, path: &str, data: &Node) -> Result<Module, Vec<Finding>> {
    let reg = Registry::new(files);
    let mut gen = generator(&reg);
    let root = root(&mut gen, files, schema);
    if !gen.refused.is_empty() {
        return Err(gen.refused);
    }
    let mut w = Writer::default();
    header(&mut w, path);
    if uses_json(&gen) {
        w.line("from \"core/json\" import { Json };");
        w.line("");
    }
    types(&gen, &mut w);
    let start = w.text.len();
    let value = literal(&gen, data, &Ty::Named(root.clone()), 0);
    w.line(&format!("export let {}: {root} = {value};", lower_camel(stem(path))));
    w.anchors.push((start, w.text.len().saturating_sub(1), path.to_string(), data.span));
    Ok(finish(w, root))
}

/// The types, and `decode`, which reads what [`strict`] writes as the root.
pub fn contract_module(files: &[SchemaFile], path: &str) -> Result<Module, Vec<Finding>> {
    let reg = Registry::new(files);
    let mut gen = generator(&reg);
    let root = root(&mut gen, files, path);
    if !gen.refused.is_empty() {
        return Err(gen.refused);
    }
    let mut w = Writer::default();
    header(&mut w, path);
    w.line("from \"platform/effect\" import { Allocator };");
    w.line("from \"core/json\" import * as json;");
    w.line("from \"core/json\" import { Json };");
    w.line("");
    types(&gen, &mut w);
    let mut helpers: BTreeSet<&'static str> = BTreeSet::new();
    w.line(&format!("/// Reads the value a tool under this contract is handed as a `{root}`."));
    w.line(&format!("export fn decode<C: Allocator>(ctx: C, text: Str): Result<{root}, Str> {{"));
    w.line("    let value = json.parse(ctx, text).mapErr(fn(_e) => \"the value is not JSON\")?;");
    w.line(&format!("    decode{root}(ctx, value)"));
    w.line("}");
    for decl in &gen.decls {
        w.line("");
        decoder(decl, &mut helpers, &mut w);
    }
    for helper in helpers {
        w.line("");
        w.text.push_str(helper_text(helper));
    }
    Ok(finish(w, root))
}

fn finish(mut w: Writer, root: String) -> Module {
    while w.text.ends_with("\n\n") {
        w.text.pop();
    }
    w.anchors.sort_by_key(|a| (a.0, std::cmp::Reverse(a.1)));
    Module { text: w.text, anchors: w.anchors, root }
}

// ---------------------------------------------------------------------------
// Decoders
// ---------------------------------------------------------------------------

/// A `Result<T, Str>` expression reading `json`, a `Json`, as `ty`.
fn read(ty: &Ty, ctx: &str, json: &str, depth: usize, helpers: &mut BTreeSet<&'static str>) -> String {
    let nested = |helper: &'static str, t: &Ty, helpers: &mut BTreeSet<&'static str>| {
        helpers.insert(helper);
        let (c, item) = (format!("c{depth}"), format!("item{depth}"));
        let inner = read(t, &c, &item, depth.saturating_add(1), helpers);
        format!("{helper}({ctx}, {json}, fn({c}, {item}) => {inner})")
    };
    match ty {
        Ty::Str => simple("readStr", json, helpers),
        Ty::Int => simple("readInt", json, helpers),
        Ty::F64 => simple("readF64", json, helpers),
        Ty::Bool => simple("readBool", json, helpers),
        Ty::Unit => simple("readUnit", json, helpers),
        Ty::Any => format!(".Ok({json})"),
        Ty::Named(n) => format!("decode{n}({ctx}, {json})"),
        Ty::List(t) => nested("readList", t, helpers),
        Ty::Option(t) => nested("readOption", t, helpers),
        Ty::Map(t) => nested("readMap", t, helpers),
    }
}

fn simple(helper: &'static str, json: &str, helpers: &mut BTreeSet<&'static str>) -> String {
    helpers.insert(helper);
    format!("{helper}({json})")
}

fn decoder(decl: &Decl, helpers: &mut BTreeSet<&'static str>, w: &mut Writer) {
    let name = &decl.name;
    let mut body = String::new();
    match &decl.shape {
        Shape::Alias(ty) => body.push_str(&format!("    {}\n", read(ty, "ctx", "value", 0, helpers))),
        Shape::Struct(fields) if fields.is_empty() => body.push_str(&format!("    .Ok({name} {{}})\n")),
        Shape::Struct(fields) => {
            body.push_str(&format!("    .Ok({name} {{\n"));
            for f in fields {
                helpers.insert("member");
                let json = format!("member(value, {})", quote(&f.key));
                body.push_str(&format!("        {}: {}?,\n", f.name, read(&f.ty, "ctx", &json, 0, helpers)));
            }
            body.push_str("    })\n");
        }
        Shape::Strings(variants) => {
            helpers.insert("readStr");
            body.push_str("    match (readStr(value)?) {\n");
            for v in variants {
                body.push_str(&format!("        {} => .Ok({name}.{}),\n", quote(&v.value), v.name));
            }
            body.push_str(&format!("        _ => .Err(\"expected a {name}\"),\n    }}\n"));
        }
        Shape::Tagged { tag, variants } => {
            helpers.insert("readStr");
            helpers.insert("member");
            body.push_str(&format!("    match (readStr(member(value, {}))?) {{\n", quote(tag)));
            for v in variants {
                let made = match &v.payload {
                    Some(p) => format!("{name}.{}(decode{p}(ctx, value)?)", v.name),
                    None => format!("{name}.{}", v.name),
                };
                body.push_str(&format!("        {} => .Ok({made}),\n", quote(&v.value)));
            }
            body.push_str(&format!("        _ => .Err(\"expected a {name}\"),\n    }}\n"));
        }
    }
    w.line(&format!("fn decode{name}<C: Allocator>(ctx: C, value: Json): Result<{name}, Str> {{"));
    w.text.push_str(&body);
    w.line("}");
}

fn helper_text(helper: &str) -> &'static str {
    match helper {
        "member" => "fn member(value: Json, name: Str): Json {\n    value.get(name).withDefault(Json.Null)\n}\n",
        "readStr" => "fn readStr(value: Json): Result<Str, Str> {\n    value.asStr().okOr(\"expected a string\")\n}\n",
        "readInt" => "fn readInt(value: Json): Result<Int, Str> {\n    value.asInt().okOr(\"expected an integer\")\n}\n",
        "readBool" => "fn readBool(value: Json): Result<Bool, Str> {\n    value.asBool().okOr(\"expected true or false\")\n}\n",
        "readUnit" => "fn readUnit(value: Json): Result<(), Str> {\n    if (value.isNull()) { .Ok(()) } else { .Err(\"expected null\") }\n}\n",
        // `strict` writes JSON5's three non-finite numbers as strings.
        "readF64" => concat!(
            "fn readF64(value: Json): Result<F64, Str> {\n",
            "    match (value) {\n",
            "        .Num(x) => .Ok(x),\n",
            "        .Str(s) => {\n",
            "            match (s) {\n",
            "                \"Infinity\" => .Ok(1.0 / 0.0),\n",
            "                \"-Infinity\" => .Ok(-1.0 / 0.0),\n",
            "                \"NaN\" => .Ok(0.0 / 0.0),\n",
            "                _ => .Err(\"expected a number\"),\n",
            "            }\n",
            "        },\n",
            "        _ => .Err(\"expected a number\"),\n",
            "    }\n",
            "}\n",
        ),
        "readList" => concat!(
            "fn readList<T, C: Allocator>(\n",
            "    ctx: C,\n",
            "    value: Json,\n",
            "    read: fn(C, Json) => Result<T, Str>,\n",
            "): Result<[T], Str> {\n",
            "    value.asArray().okOr(\"expected an array\")?.mapResultCtx(ctx, read)\n",
            "}\n",
        ),
        "readOption" => concat!(
            "fn readOption<T, C: Allocator>(\n",
            "    ctx: C,\n",
            "    value: Json,\n",
            "    read: fn(C, Json) => Result<T, Str>,\n",
            "): Result<Option<T>, Str> {\n",
            "    match (value) {\n",
            "        .Null => .Ok(.None),\n",
            "        _ => .Ok(.Some(read(ctx, value)?)),\n",
            "    }\n",
            "}\n",
        ),
        "readMap" => concat!(
            "fn readMap<T, C: Allocator>(\n",
            "    ctx: C,\n",
            "    value: Json,\n",
            "    read: fn(C, Json) => Result<T, Str>,\n",
            "): Result<[(Str, T)], Str> {\n",
            "    value\n",
            "        .asObject()\n",
            "        .okOr(\"expected an object\")?\n",
            "        .mapResultCtx(ctx, fn(c, entry) => {\n",
            "            let (key, item) = entry;\n",
            "            .Ok((key, read(c, item)?))\n",
            "        })\n",
            "}\n",
        ),
        _ => "",
    }
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

/// A Buri string literal.
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn float(n: &Number) -> String {
    match n {
        Number::Infinity(false) => "1.0 / 0.0".to_string(),
        Number::Infinity(true) => "-1.0 / 0.0".to_string(),
        Number::NaN => "0.0 / 0.0".to_string(),
        Number::Finite(d) => {
            let text = format!("{:?}", d.to_f64());
            match (text.contains('.'), text.split_once('e')) {
                (false, Some((m, e))) => format!("{m}.0e{e}"),
                _ => text,
            }
        }
    }
}

fn integer(n: &Number) -> String {
    match n {
        Number::Finite(d) => d.to_i64().map_or_else(|| float(n), |i| i.to_string()),
        other => float(other),
    }
}

fn pad(indent: usize) -> String {
    "    ".repeat(indent)
}

/// A list literal, one element a line.
fn list(items: Vec<String>, indent: usize) -> String {
    if items.is_empty() {
        return "[]".to_string();
    }
    let inner = pad(indent.saturating_add(1));
    let mut out = String::from("[\n");
    for item in items {
        out.push_str(&format!("{inner}{item},\n"));
    }
    out.push_str(&format!("{}]", pad(indent)));
    out
}

/// `node`, a value the check has passed, as a Buri expression of type `ty`.
fn literal(gen: &Gen, node: &Node, ty: &Ty, indent: usize) -> String {
    let next = indent.saturating_add(1);
    match (ty, &node.value) {
        (Ty::Option(_), Value::Null) => ".None".to_string(),
        (Ty::Option(t), _) => format!(".Some({})", literal(gen, node, t, indent)),
        (Ty::Str, Value::Str(s)) => quote(s),
        (Ty::Int, Value::Number(n)) => integer(n),
        (Ty::F64, Value::Number(n)) => float(n),
        (Ty::Bool, Value::Bool(b)) => b.to_string(),
        (Ty::Unit, _) => "()".to_string(),
        (Ty::List(t), Value::Array(items, _)) => {
            list(items.iter().map(|i| literal(gen, i, t, next)).collect(), indent)
        }
        (Ty::Map(t), Value::Object(members, _)) => list(
            members.iter().map(|m| format!("({}, {})", quote(&m.key), literal(gen, &m.value, t, next))).collect(),
            indent,
        ),
        (Ty::Named(name), _) => named(gen, node, name, indent),
        (_, _) => json_literal(node, indent),
    }
}

fn named(gen: &Gen, node: &Node, name: &str, indent: usize) -> String {
    let Some(decl) = gen.decl(name) else { return json_literal(node, indent) };
    match &decl.shape {
        Shape::Alias(ty) => literal(gen, node, ty, indent),
        Shape::Struct(fields) => struct_literal(gen, node, name, fields, indent),
        Shape::Strings(variants) => {
            let value = node.as_str().unwrap_or_default();
            let variant = variants.iter().find(|v| v.value == value).map_or("", |v| v.name.as_str());
            format!("{name}.{variant}")
        }
        Shape::Tagged { tag, variants } => {
            let value = node.get(tag).and_then(Node::as_str).unwrap_or_default();
            let Some(v) = variants.iter().find(|v| v.value == value) else { return json_literal(node, indent) };
            match &v.payload {
                Some(p) => format!("{name}.{}({})", v.name, named(gen, node, p, indent)),
                None => format!("{name}.{}", v.name),
            }
        }
    }
}

fn struct_literal(gen: &Gen, node: &Node, name: &str, fields: &[Field], indent: usize) -> String {
    if fields.is_empty() {
        return format!("{name} {{}}");
    }
    let inner = pad(indent.saturating_add(1));
    let mut out = format!("{name} {{\n");
    for f in fields {
        let value = match node.get(&f.key) {
            Some(v) => literal(gen, v, &f.ty, indent.saturating_add(1)),
            None => ".None".to_string(),
        };
        out.push_str(&format!("{inner}{}: {value},\n", f.name));
    }
    out.push_str(&format!("{}}}", pad(indent)));
    out
}

/// A `core/json` `Json` expression.
fn json_literal(node: &Node, indent: usize) -> String {
    let next = indent.saturating_add(1);
    match &node.value {
        Value::Null => "Json.Null".to_string(),
        Value::Bool(b) => format!("Json.Bool({b})"),
        Value::Number(n) => format!("Json.Num({})", float(n)),
        Value::Str(s) => format!("Json.Str({})", quote(s)),
        Value::Array(items, _) => {
            format!("Json.Array({})", list(items.iter().map(|i| json_literal(i, next)).collect(), indent))
        }
        Value::Object(members, _) => format!(
            "Json.Object({})",
            list(members.iter().map(|m| format!("({}, {})", quote(&m.key), json_literal(&m.value, next))).collect(), indent)
        ),
    }
}

/// A document as strict JSON, which is what a tool under a contract is
/// handed. Comments go; JSON5's `Infinity`, `-Infinity` and `NaN`, which
/// strict JSON cannot spell, are written as those strings.
pub fn strict(node: &Node) -> String {
    let mut out = String::new();
    write_strict(node, &mut out);
    out
}

fn write_strict(node: &Node, out: &mut String) {
    match &node.value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(&b.to_string()),
        Value::Number(Number::Infinity(false)) => out.push_str("\"Infinity\""),
        Value::Number(Number::Infinity(true)) => out.push_str("\"-Infinity\""),
        Value::Number(Number::NaN) => out.push_str("\"NaN\""),
        Value::Number(n @ Number::Finite(d)) => match d.to_i64() {
            Some(i) => out.push_str(&i.to_string()),
            None => out.push_str(&float(n)),
        },
        Value::Str(s) => out.push_str(&crate::json::Value::str(s).to_string()),
        Value::Array(items, _) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_strict(item, out);
            }
            out.push(']');
        }
        Value::Object(members, _) => {
            out.push('{');
            for (i, m) in members.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&crate::json::Value::str(&m.key).to_string());
                out.push(':');
                write_strict(&m.value, out);
            }
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::languages::json::syntax::{parse, Dialect};

    fn files(texts: &[(&str, &str)]) -> Vec<SchemaFile> {
        texts
            .iter()
            .map(|(p, t)| SchemaFile { path: p.to_string(), root: parse(t, Dialect::Json5).expect("parses").root })
            .collect()
    }

    #[test]
    fn names_are_buri_names() {
        assert_eq!(upper_camel("eu-west"), "EuWest");
        assert_eq!(upper_camel("3d"), "V3d");
        assert_eq!(lower_camel("max_connections"), "maxConnections");
        assert_eq!(lower_camel("type"), "type_");
        assert_eq!(stem("lib/a/regions.schema.json"), "regions");
    }

    #[test]
    fn strict_json_spells_what_json5_adds() {
        let doc = parse("{ a: Infinity, b: -Infinity, c: NaN, d: 0x10, e: .5, // x\n }", Dialect::Json5).unwrap();
        assert_eq!(strict(&doc.root), r#"{"a":"Infinity","b":"-Infinity","c":"NaN","d":16,"e":0.5}"#);
    }

    #[test]
    fn a_float_literal_always_has_a_point() {
        let doc = parse("[1e300, 2.5, 3]", Dialect::Json).unwrap();
        let Value::Array(items, _) = &doc.root.value else { panic!() };
        let spelled: Vec<String> = items.iter().map(|i| match &i.value { Value::Number(n) => float(n), _ => String::new() }).collect();
        assert_eq!(spelled, vec!["1.0e300", "2.5", "3.0"]);
    }

    #[test]
    fn a_schema_with_no_type_is_refused_where_it_says_so() {
        let fs = files(&[("s.json", r#"{ "if": {}, "then": {} }"#)]);
        let Err(refused) = schema_module(&fs, "s.json") else { panic!("refused") };
        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0].code, "json-untyped-keyword");
    }
}
