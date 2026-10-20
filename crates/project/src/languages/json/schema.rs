//! JSON Schema 2020-12: reading a schema, and checking a value against it.
//!
//! Every keyword of the core, applicator, unevaluated and validation
//! vocabularies is enforced:
//!
//! ```text
//! $ref $dynamicRef $defs $id $anchor $dynamicAnchor
//! allOf anyOf oneOf not if then else dependentSchemas
//! prefixItems items contains properties patternProperties additionalProperties propertyNames
//! unevaluatedItems unevaluatedProperties
//! type enum const multipleOf maximum exclusiveMaximum minimum exclusiveMinimum
//! maxLength minLength pattern maxItems minItems uniqueItems maxContains minContains
//! maxProperties minProperties required dependentRequired
//! ```
//!
//! `format`, the content keywords and the annotations (`title`, `default`, …)
//! are annotations in 2020-12 and assert nothing. A `$ref` reaches a pointer or
//! an anchor in its own file, another checked-in file by a relative or `//`
//! path, or a resource by its absolute `$id`; nothing is fetched. The draft-07
//! spellings with a 2020-12 replacement are refused by name, because 2020-12
//! would silently ignore them.

use super::number::Decimal;
use super::regex::{Regex, TooCostly};
use super::syntax::{Member, Node, Range, Value};
use crate::languages::Finding;
use std::collections::{BTreeSet, HashMap};

/// The one meta-schema this toolchain knows, by name.
pub const DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";

/// Whether a `$schema` names the 2020-12 meta-schema.
pub fn is_2020_12(uri: &str) -> bool {
    uri.strip_suffix('#').unwrap_or(uri) == DRAFT_2020_12
}

/// Whether a string is a URI with a scheme, like `https://…` or `urn:…`.
pub fn has_scheme(s: &str) -> bool {
    let Some((scheme, _)) = s.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// A repository path for `reference`, written relative to the file `from` or
/// as a `//` path. `None` when it leaves the repository.
pub fn local_path(from: &str, reference: &str) -> Option<String> {
    let reference = percent_decode(reference);
    let (mut parts, rest): (Vec<&str>, &str) = match reference.strip_prefix("//") {
        Some(rest) => (Vec::new(), rest),
        None => {
            if reference.starts_with('/') {
                return None;
            }
            let dir = from.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
            (
                dir.split('/').filter(|s| !s.is_empty()).collect(),
                reference.as_str(),
            )
        }
    };
    for seg in rest.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            seg => parts.push(seg),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

/// The relative `reference` resolved against the absolute URI `base`, as RFC
/// 3986 §5.2 does it, less queries. Both callers handle an absolute reference
/// first.
fn resolve_uri(base: &str, reference: &str) -> String {
    let base = base.split('#').next().unwrap_or_default();
    let (scheme, rest) = base.split_once(':').unwrap_or(("", base));
    if let Some(net) = reference.strip_prefix("//") {
        return format!("{scheme}://{net}");
    }
    let (authority, path) = match rest.strip_prefix("//") {
        Some(r) => {
            let at = r.find('/').unwrap_or(r.len());
            (
                format!("//{}", r.get(..at).unwrap_or_default()),
                r.get(at..).unwrap_or_default(),
            )
        }
        None => (String::new(), rest),
    };
    let (reference, fragment) = match reference.split_once('#') {
        Some((r, f)) => (r, format!("#{f}")),
        None => (reference, String::new()),
    };
    let merged = if reference.is_empty() {
        path.to_string()
    } else if reference.starts_with('/') {
        reference.to_string()
    } else {
        match path.rsplit_once('/') {
            Some((dir, _)) => format!("{dir}/{reference}"),
            None if authority.is_empty() => reference.to_string(),
            None => format!("/{reference}"),
        }
    };
    let mut out: Vec<&str> = Vec::new();
    let segments: Vec<&str> = merged.split('/').collect();
    let last = segments.len().saturating_sub(1);
    for (i, seg) in segments.iter().enumerate() {
        match *seg {
            "." => {}
            ".." => {
                if out.len() > 1 {
                    out.pop();
                }
            }
            s => out.push(s),
        }
        // A trailing `.` or `..` still leaves the path a directory.
        if i == last && matches!(*seg, "." | "..") {
            out.push("");
        }
    }
    format!("{scheme}:{authority}{}{fragment}", out.join("/"))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hex = bytes.get(i.saturating_add(1)..i.saturating_add(3));
            if let Some(v) = hex
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(v);
                i = i.saturating_add(3);
                continue;
            }
        }
        out.push(b);
        i = i.saturating_add(1);
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// One schema file, parsed.
pub struct SchemaFile {
    pub path: String,
    pub root: Node,
}

/// The schema files one check reads: the one it is about, and every other
/// file in play.
pub struct SchemaSet {
    pub main: SchemaFile,
    pub rest: Vec<SchemaFile>,
}

/// The references a schema file makes to other files: `(span, repository path)`.
pub fn file_references(file: &SchemaFile) -> Vec<(Range, Option<String>)> {
    let mut out = Vec::new();
    collect_refs(&file.root, &mut |reference, span| {
        let base = reference.split('#').next().unwrap_or_default();
        if base.is_empty() || has_scheme(base) {
            return;
        }
        out.push((span, local_path(&file.path, base)));
    });
    out
}

fn collect_refs(node: &Node, f: &mut dyn FnMut(&str, Range)) {
    match &node.value {
        Value::Object(members, _) => {
            for m in members {
                if matches!(m.key.as_str(), "$ref" | "$dynamicRef") {
                    if let Value::Str(s) = &m.value.value {
                        f(s, m.value.span);
                        continue;
                    }
                }
                collect_refs(&m.value, f);
            }
        }
        Value::Array(items, _) => items.iter().for_each(|i| collect_refs(i, f)),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Reading a schema
// ---------------------------------------------------------------------------

struct Resource<'r> {
    file: usize,
    root: &'r Node,
    /// The absolute `$id` of the resource, which relative references resolve
    /// against before they resolve against the file.
    id: Option<String>,
}

/// A reference the registry resolved: what it says, and the schema it names
/// with the resource that schema is in.
#[derive(Clone, Copy)]
pub struct Reference<'r> {
    pub written: &'r str,
    pub resource: usize,
    pub target: &'r Node,
}

/// The resource the main file's root is: [`Registry::new`] adds it first.
const MAIN: usize = 0;

/// Every schema file in play, read and cross-referenced.
pub struct Registry<'r> {
    /// The main file first, so its root is resource [`MAIN`].
    files: Vec<&'r SchemaFile>,
    main: &'r SchemaFile,
    resources: Vec<Resource<'r>>,
    /// Each resource's `$anchor`s and `$dynamicAnchor`s, by resource and name.
    anchors: HashMap<(usize, String), &'r Node>,
    dynamic_anchors: HashMap<(usize, String), &'r Node>,
    by_id: HashMap<String, usize>,
    by_file: HashMap<String, usize>,
    /// Which resource a node is the root of.
    root_of: HashMap<*const Node, usize>,
    /// Where each schema node sits: its file, its JSON pointer, and the
    /// resource it is in.
    location: HashMap<*const Node, (usize, String, usize)>,
    /// What each reference resolved to, by the string node that writes it.
    targets: HashMap<*const Node, Reference<'r>>,
    /// Each object schema's keywords, as reading the schema understood them.
    keywords: HashMap<*const Node, Vec<Keyword<'r>>>,
    regexes: HashMap<String, Regex>,
    problems: Vec<Finding>,
}

/// One keyword of an object schema.
struct Keyword<'r> {
    member: &'r Member,
    kind: Kind<'r>,
}

/// What reading a keyword learned about its value, so checking a value does
/// not have to learn it again.
enum Kind<'r> {
    /// `$ref` and `$dynamicRef`, resolved.
    Ref(Reference<'r>),
    /// `multipleOf` and the bounds.
    Number(&'r Decimal),
    /// `type`, as the names it lists.
    Type(Vec<&'r str>),
    Pattern(&'r str, Regex),
    /// `patternProperties`, `dependentSchemas` and `dependentRequired`.
    Entries(&'r [Member]),
    /// A keyword checking reads straight from the schema.
    Other,
}

/// A keyword as walking the schema leaves it: a reference has to wait until
/// every resource is known before it resolves.
enum Read<'r> {
    Kind(Kind<'r>),
    Reference,
}

/// What walking the schemas finds that has to wait for the walk to finish.
#[derive(Default)]
struct Found<'r> {
    /// Each reference: its file, span and text, the string node, and the
    /// resource it is written in.
    refs: Vec<(usize, Range, &'r str, *const Node, usize)>,
    schemas: Vec<(*const Node, Vec<(&'r Member, Read<'r>)>)>,
}

const TYPES: &[&str] = &[
    "null", "boolean", "object", "array", "number", "string", "integer",
];

const SCHEMA_KEYWORDS: &[&str] = &[
    "additionalProperties",
    "propertyNames",
    "items",
    "contains",
    "not",
    "if",
    "then",
    "else",
    "unevaluatedItems",
    "unevaluatedProperties",
];

const SCHEMA_MAPS: &[&str] = &[
    "properties",
    "patternProperties",
    "$defs",
    "definitions",
    "dependentSchemas",
];

const SCHEMA_LISTS: &[&str] = &["allOf", "anyOf", "oneOf", "prefixItems"];

const COUNTS: &[&str] = &[
    "maxLength",
    "minLength",
    "maxItems",
    "minItems",
    "maxContains",
    "minContains",
    "maxProperties",
    "minProperties",
];

const BOUNDS: &[&str] = &["maximum", "exclusiveMaximum", "minimum", "exclusiveMinimum"];

/// The draft-07 keywords 2020-12 replaced, with what replaced them.
const RETIRED: &[(&str, &str)] = &[
    (
        "additionalItems",
        "use `items`, which holds the schema for the items after `prefixItems`",
    ),
    (
        "dependencies",
        "use `dependentRequired` for a list of names, and `dependentSchemas` for a schema",
    ),
    ("$recursiveRef", "use `$dynamicRef`"),
    ("$recursiveAnchor", "use `$dynamicAnchor`"),
];

fn escape_pointer(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn is_count(n: &Node) -> bool {
    matches!(&n.value, Value::Number(super::number::Number::Finite(d)) if d.is_integer() && !d.is_negative())
}

impl<'r> Registry<'r> {
    /// Reads every file as a schema. What is wrong with one is in
    /// [`Registry::problems`].
    pub fn new(set: &'r SchemaSet) -> Registry<'r> {
        let files: Vec<&SchemaFile> = std::iter::once(&set.main).chain(&set.rest).collect();
        let mut reg = Registry {
            files: files.clone(),
            main: &set.main,
            resources: Vec::new(),
            anchors: HashMap::new(),
            dynamic_anchors: HashMap::new(),
            by_id: HashMap::new(),
            by_file: HashMap::new(),
            root_of: HashMap::new(),
            location: HashMap::new(),
            targets: HashMap::new(),
            keywords: HashMap::new(),
            regexes: HashMap::new(),
            problems: Vec::new(),
        };
        let mut found = Found::default();
        for (i, file) in files.into_iter().enumerate() {
            let id = file
                .root
                .get("$id")
                .and_then(Node::as_str)
                .filter(|s| has_scheme(s))
                .map(|s| s.trim_end_matches('#').to_string());
            let r = reg.add_resource(i, &file.root, id);
            reg.by_file.insert(file.path.clone(), r);
            reg.walk(&file.root, i, String::new(), r, &mut found);
        }
        // Every reference resolves, and whatever it lands on is a schema too.
        let mut at = 0usize;
        while let Some((file, span, reference, node, resource)) = found.refs.get(at).copied() {
            at = at.saturating_add(1);
            match reg.resolve(reference, resource) {
                Ok((target_resource, target)) => {
                    reg.targets.insert(
                        node,
                        Reference {
                            written: reference,
                            resource: target_resource,
                            target,
                        },
                    );
                    if !reg.location.contains_key(&(target as *const Node)) {
                        let pointer = reference
                            .split_once('#')
                            .map(|(_, f)| f.to_string())
                            .unwrap_or_default();
                        let target_file =
                            reg.resources.get(target_resource).map_or(file, |r| r.file);
                        reg.walk(target, target_file, pointer, target_resource, &mut found);
                    }
                }
                Err(e) => reg.unresolved(file, span, reference, e),
            }
        }
        for (node, read) in found.schemas {
            let keywords = read
                .into_iter()
                .map(|(member, read)| {
                    let kind = match read {
                        Read::Kind(kind) => kind,
                        Read::Reference => {
                            // One that did not resolve is a problem already,
                            // and a schema with a problem checks nothing.
                            let Some(target) = reg.targets.get(&(&member.value as *const Node))
                            else {
                                return Keyword {
                                    member,
                                    kind: Kind::Other,
                                };
                            };
                            Kind::Ref(*target)
                        }
                    };
                    Keyword { member, kind }
                })
                .collect();
            reg.keywords.insert(node, keywords);
        }
        reg
    }

    pub fn problems(&self) -> &[Finding] {
        &self.problems
    }

    /// The main file, and the resource its root is.
    pub fn main(&self) -> (usize, &'r SchemaFile) {
        (MAIN, self.main)
    }

    /// The resource `node` is in, given the one its parent is in.
    pub fn enter(&self, node: &Node, current: usize) -> usize {
        self.root_of
            .get(&(node as *const Node))
            .copied()
            .unwrap_or(current)
    }

    /// What the `$ref` in `schema` names, when it has one.
    pub fn reference(&self, schema: &Node) -> Option<Reference<'r>> {
        schema
            .get("$ref")
            .and_then(|written| self.targets.get(&(written as *const Node)))
            .copied()
    }

    /// The repository path of the file a resource is in.
    pub fn file_of(&self, resource: usize) -> String {
        self.resources
            .get(resource)
            .map(|r| self.path_of(r.file))
            .unwrap_or_default()
    }

    fn add_resource(&mut self, file: usize, root: &'r Node, id: Option<String>) -> usize {
        let index = self.resources.len();
        if let Some(id) = &id {
            self.by_id.insert(id.clone(), index);
        }
        self.resources.push(Resource { file, root, id });
        self.root_of.insert(root as *const Node, index);
        index
    }

    fn path_of(&self, file: usize) -> String {
        self.files
            .get(file)
            .map(|f| f.path.clone())
            .unwrap_or_default()
    }

    fn problem(
        &mut self,
        file: usize,
        span: Range,
        problem: impl Into<String>,
        remedy: impl Into<String>,
    ) {
        self.problems.push(Finding::new(
            "json-invalid-schema",
            &self.path_of(file),
            span,
            vec![("problem", problem.into()), ("remedy", remedy.into())],
        ));
    }

    fn unresolved(&mut self, file: usize, span: Range, reference: &str, e: Unresolved) {
        let path = self.path_of(file);
        let finding = match e {
            Unresolved::NotLocal => Finding::new(
                "schema-outside-repository",
                &path,
                span,
                vec![("schema", reference.to_string())],
            ),
            Unresolved::NoFile(missing) => {
                Finding::new("unknown-schema", &path, span, vec![("path", missing)])
            }
            Unresolved::NoTarget => Finding::new(
                "json-invalid-schema",
                &path,
                span,
                vec![
                    (
                        "problem",
                        format!("`{reference}` points at nothing in the schema"),
                    ),
                    (
                        "remedy",
                        "correct the pointer or the anchor it names".to_string(),
                    ),
                ],
            ),
        };
        self.problems.push(finding);
    }

    /// The pattern, compiled, or `None` with the problem reported.
    fn regex(&mut self, file: usize, span: Range, pattern: &str) -> Option<Regex> {
        if let Some(r) = self.regexes.get(pattern) {
            return Some(r.clone());
        }
        match Regex::new(pattern) {
            Ok(r) => {
                self.regexes.insert(pattern.to_string(), r.clone());
                Some(r)
            }
            Err(why) => {
                self.problem(
                    file,
                    span,
                    format!("the pattern `{pattern}` does not read: {why}"),
                    "write an ECMA-262 regular expression this toolchain reads; `buri docs guides/json` lists the syntax",
                );
                None
            }
        }
    }

    /// Checks that `node` is a schema, and records what it declares.
    fn walk(
        &mut self,
        node: &'r Node,
        file: usize,
        pointer: String,
        resource: usize,
        found: &mut Found<'r>,
    ) {
        if self.location.contains_key(&(node as *const Node)) {
            return;
        }
        self.location
            .insert(node as *const Node, (file, pointer.clone(), resource));
        let members = match &node.value {
            Value::Bool(_) => return,
            Value::Object(members, _) => members,
            _ => {
                return self.problem(
                    file,
                    node.span,
                    "a schema is an object or `true` or `false`",
                    "write an object here",
                );
            }
        };
        let mut resource = resource;
        if let Some(id) = node.member("$id") {
            match id.value.as_str() {
                Some(s) if has_scheme(s) => {
                    let id = s.trim_end_matches('#').to_string();
                    if !pointer.is_empty() {
                        resource = self.add_resource(file, node, Some(id));
                    }
                }
                // A file's own relative `$id` names the file, which is how it
                // is reached anyway.
                Some(_) if pointer.is_empty() => {}
                // Under an absolute `$id`, a relative one resolves against it.
                Some(s) if self.resources.get(resource).is_some_and(|r| r.id.is_some()) => {
                    let base = self
                        .resources
                        .get(resource)
                        .and_then(|r| r.id.clone())
                        .unwrap_or_default();
                    let id = resolve_uri(&base, s).trim_end_matches('#').to_string();
                    resource = self.add_resource(file, node, Some(id));
                }
                Some(s) => self.problem(
                    file,
                    id.value.span,
                    format!("`\"$id\": \"{s}\"` is relative, which only a schema's root may be"),
                    "give the subschema an absolute `$id`, or move it to a file of its own",
                ),
                None => self.problem(
                    file,
                    id.value.span,
                    "`$id` is a string",
                    "write the URI as a string",
                ),
            }
        }
        // An anchor names the schema object it is written in.
        for key in ["$anchor", "$dynamicAnchor"] {
            let Some(name) = node
                .get(key)
                .and_then(Node::as_str)
                .filter(|n| is_anchor(n))
            else {
                continue;
            };
            let map = if key == "$anchor" {
                &mut self.anchors
            } else {
                &mut self.dynamic_anchors
            };
            map.insert((resource, name.to_string()), node);
        }
        let read = members
            .iter()
            .map(|m| (m, self.keyword(m, file, &pointer, resource, found)))
            .collect();
        found.schemas.push((node as *const Node, read));
    }

    /// Checks one keyword's value, and says what it learned.
    #[allow(
        clippy::too_many_lines,
        reason = "one arm per keyword, in the order the vocabularies list them"
    )]
    fn keyword(
        &mut self,
        m: &'r Member,
        file: usize,
        pointer: &str,
        resource: usize,
        found: &mut Found<'r>,
    ) -> Read<'r> {
        use super::number::Number;
        let v = &m.value;
        let here = format!("{pointer}/{}", escape_pointer(&m.key));
        let key = m.key.as_str();
        if let Some((_, remedy)) = RETIRED.iter().find(|(k, _)| *k == key) {
            self.problem(
                file,
                m.key_span,
                format!("`{key}` is not a JSON Schema 2020-12 keyword"),
                *remedy,
            );
            return Read::Kind(Kind::Other);
        }
        let kind = match key {
            "$schema" => {
                if !v.as_str().is_some_and(is_2020_12) {
                    self.problems.push(Finding::new(
                        "json-schema-draft-unsupported",
                        &self.path_of(file),
                        v.span,
                        vec![(
                            "draft",
                            v.as_str().map_or_else(|| v.raw.clone(), str::to_string),
                        )],
                    ));
                }
                Kind::Other
            }
            "$ref" | "$dynamicRef" => match v.as_str() {
                Some(s) => {
                    found
                        .refs
                        .push((file, v.span, s, v as *const Node, resource));
                    return Read::Reference;
                }
                None => {
                    self.problem(
                        file,
                        v.span,
                        format!("`{key}` is a string"),
                        "write the reference as a string",
                    );
                    Kind::Other
                }
            },
            "$anchor" | "$dynamicAnchor" if !v.as_str().is_some_and(is_anchor) => {
                self.problem(
                    file,
                    v.span,
                    format!(
                        "`{key}` is a name: a letter or `_`, then letters, digits, `-`, `.` or `_`"
                    ),
                    "rename the anchor",
                );
                Kind::Other
            }
            "items" if matches!(v.value, Value::Array(..)) => {
                self.problem(
                    file,
                    m.key_span,
                    "`items` holds one schema in 2020-12",
                    "use `prefixItems` for a list of schemas, one per position",
                );
                Kind::Other
            }
            k if SCHEMA_KEYWORDS.contains(&k) => {
                self.walk(v, file, here, resource, found);
                Kind::Other
            }
            k if SCHEMA_MAPS.contains(&k) => match &v.value {
                Value::Object(entries, _) => {
                    for e in entries {
                        if k == "patternProperties" {
                            self.regex(file, e.key_span, &e.key);
                        }
                        self.walk(
                            &e.value,
                            file,
                            format!("{here}/{}", escape_pointer(&e.key)),
                            resource,
                            found,
                        );
                    }
                    Kind::Entries(entries)
                }
                _ => {
                    self.problem(
                        file,
                        v.span,
                        format!("`{k}` is an object of schemas"),
                        "write an object",
                    );
                    Kind::Other
                }
            },
            k if SCHEMA_LISTS.contains(&k) => {
                match &v.value {
                    Value::Array(items, _) if !items.is_empty() => {
                        for (i, item) in items.iter().enumerate() {
                            self.walk(item, file, format!("{here}/{i}"), resource, found);
                        }
                    }
                    _ => self.problem(
                        file,
                        v.span,
                        format!("`{k}` is a non-empty array of schemas"),
                        "write an array with at least one schema",
                    ),
                }
                Kind::Other
            }
            "type" => {
                let names: Vec<Option<&str>> = match &v.value {
                    Value::Str(s) => vec![Some(s.as_str())],
                    Value::Array(items, _) => items.iter().map(Node::as_str).collect(),
                    _ => Vec::new(),
                };
                let unique: BTreeSet<&Option<&str>> = names.iter().collect();
                let known: Vec<&str> = names
                    .iter()
                    .flatten()
                    .copied()
                    .filter(|n| TYPES.contains(n))
                    .collect();
                if !known.is_empty() && known.len() == names.len() && unique.len() == names.len() {
                    Kind::Type(known)
                } else {
                    self.problem(
                        file,
                        v.span,
                        format!("`type` is one of {}, or a list of them", TYPES.join(", ")),
                        "correct the type's name",
                    );
                    Kind::Other
                }
            }
            "enum" if !matches!(v.value, Value::Array(..)) => {
                self.problem(
                    file,
                    v.span,
                    "`enum` is an array",
                    "write the allowed values in an array",
                );
                Kind::Other
            }
            "multipleOf" => match &v.value {
                Value::Number(Number::Finite(d)) if !d.is_zero() && !d.is_negative() => {
                    Kind::Number(d)
                }
                _ => {
                    self.problem(
                        file,
                        v.span,
                        "`multipleOf` is a number greater than zero",
                        "write a positive number",
                    );
                    Kind::Other
                }
            },
            k if BOUNDS.contains(&k) => match &v.value {
                Value::Number(Number::Finite(d)) => Kind::Number(d),
                _ => {
                    self.problem(file, v.span, format!("`{k}` is a number"), "write a number");
                    Kind::Other
                }
            },
            k if COUNTS.contains(&k) && !is_count(v) => {
                self.problem(
                    file,
                    v.span,
                    format!("`{k}` is a whole number, zero or more"),
                    "write a whole number",
                );
                Kind::Other
            }
            "uniqueItems" if !matches!(v.value, Value::Bool(_)) => {
                self.problem(
                    file,
                    v.span,
                    "`uniqueItems` is `true` or `false`",
                    "write a boolean",
                );
                Kind::Other
            }
            "pattern" => match v.as_str() {
                Some(p) => {
                    if let Some(regex) = self.regex(file, v.span, p) {
                        Kind::Pattern(p, regex)
                    } else {
                        Kind::Other
                    }
                }
                None => {
                    self.problem(
                        file,
                        v.span,
                        "`pattern` is a string",
                        "write the pattern as a string",
                    );
                    Kind::Other
                }
            },
            "required" if !is_name_list(v) => {
                self.problem(
                    file,
                    v.span,
                    "`required` is an array of property names, each once",
                    "write the names as strings",
                );
                Kind::Other
            }
            "dependentRequired" => match &v.value {
                Value::Object(entries, _) if entries.iter().all(|e| is_name_list(&e.value)) => {
                    Kind::Entries(entries)
                }
                _ => {
                    self.problem(
                        file,
                        v.span,
                        "`dependentRequired` maps a property name to an array of names",
                        "write an object of arrays of names",
                    );
                    Kind::Other
                }
            },
            _ => Kind::Other,
        };
        Read::Kind(kind)
    }

    /// The schema `reference` names, from inside `resource`.
    fn resolve(&self, reference: &str, resource: usize) -> Result<(usize, &'r Node), Unresolved> {
        let (base, fragment) = match reference.split_once('#') {
            Some((b, f)) => (b, Some(f)),
            None => (reference, None),
        };
        let target = if base.is_empty() {
            resource
        } else {
            self.resource_named(base, resource)?
        };
        let r = self.resources.get(target).ok_or(Unresolved::NoTarget)?;
        let fragment = percent_decode(fragment.unwrap_or_default());
        if fragment.is_empty() {
            return Ok((target, r.root));
        }
        if let Some(pointer) = fragment.strip_prefix('/') {
            let mut node = r.root;
            for raw in pointer.split('/') {
                let seg = raw.replace("~1", "/").replace("~0", "~");
                node = match &node.value {
                    Value::Object(..) => node.get(&seg).ok_or(Unresolved::NoTarget)?,
                    Value::Array(items, _) => seg
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| items.get(i))
                        .ok_or(Unresolved::NoTarget)?,
                    _ => return Err(Unresolved::NoTarget),
                };
            }
            // A pointer into an embedded resource lands in that resource, at
            // its root or anywhere inside it.
            let ptr = node as *const Node;
            let inside = self.location.get(&ptr).map(|(_, _, r)| *r);
            let owner = self.root_of.get(&ptr).copied().or(inside).unwrap_or(target);
            return Ok((owner, node));
        }
        let key = (target, fragment);
        let node = self
            .anchors
            .get(&key)
            .or_else(|| self.dynamic_anchors.get(&key))
            .ok_or(Unresolved::NoTarget)?;
        Ok((target, node))
    }

    fn resource_named(&self, base: &str, from: usize) -> Result<usize, Unresolved> {
        if has_scheme(base) {
            return self
                .by_id
                .get(base.trim_end_matches('#'))
                .copied()
                .ok_or(Unresolved::NotLocal);
        }
        let r = self.resources.get(from).ok_or(Unresolved::NoTarget)?;
        if let Some(id) = &r.id {
            if let Some(i) = self.by_id.get(&resolve_uri(id, base)) {
                return Ok(*i);
            }
        }
        let from_path = self.path_of(r.file);
        let path = local_path(&from_path, base).ok_or(Unresolved::NotLocal)?;
        self.by_file
            .get(&path)
            .copied()
            .ok_or(Unresolved::NoFile(path))
    }

    fn keyword_location(&self, schema: &Node, keyword: &str) -> String {
        match self.location.get(&(schema as *const Node)) {
            Some((file, pointer, _)) => format!(
                "{}#{pointer}/{}",
                self.path_of(*file),
                escape_pointer(keyword)
            ),
            None => keyword.to_string(),
        }
    }
}

fn is_anchor(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
}

fn is_name_list(v: &Node) -> bool {
    match &v.value {
        Value::Array(items, _) => {
            let names: Vec<&str> = items.iter().filter_map(Node::as_str).collect();
            let unique: BTreeSet<&&str> = names.iter().collect();
            names.len() == items.len() && unique.len() == names.len()
        }
        _ => false,
    }
}

#[derive(Debug)]
enum Unresolved {
    NotLocal,
    NoFile(String),
    NoTarget,
}

// ---------------------------------------------------------------------------
// Checking a value
// ---------------------------------------------------------------------------

/// One way a value fails its schema.
#[derive(Clone, Debug, PartialEq)]
pub struct Violation {
    pub span: Range,
    pub problem: String,
    pub remedy: String,
    /// The keyword that failed, as `file#/json/pointer`.
    pub location: String,
}

/// What a schema looked at, which `unevaluatedItems` and
/// `unevaluatedProperties` ask about.
#[derive(Default)]
struct Evaluated {
    props: BTreeSet<String>,
    all_props: bool,
    items: BTreeSet<usize>,
    all_items: bool,
}

impl Evaluated {
    fn merge(&mut self, other: Evaluated) {
        self.props.extend(other.props);
        self.all_props |= other.all_props;
        self.items.extend(other.items);
        self.all_items |= other.all_items;
    }
}

/// The most violations one file reports.
const MAX_VIOLATIONS: usize = 100;

/// How deeply `$ref`s may nest before the schema is taken to loop.
const MAX_DEPTH: usize = 200;

struct Checker<'a, 'r> {
    reg: &'a Registry<'r>,
    /// The resources entered, outermost first, for `$dynamicRef`.
    dynamic: Vec<usize>,
    depth: usize,
}

/// Checks `instance` against the root of the registry's main file.
pub fn validate(reg: &Registry, instance: &Node) -> Vec<Violation> {
    let (resource, main) = reg.main();
    let root = &main.root;
    let mut c = Checker {
        reg,
        dynamic: Vec::new(),
        depth: 0,
    };
    let mut out = Vec::new();
    c.check(root, resource, instance, &mut out);
    out.truncate(MAX_VIOLATIONS);
    out
}

fn kind(v: &Node) -> &'static str {
    match &v.value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(super::number::Number::Finite(d)) if d.is_integer() => "an integer",
        Value::Number(_) => "a number",
        Value::Str(_) => "a string",
        Value::Array(..) => "an array",
        Value::Object(..) => "an object",
    }
}

fn type_phrase(t: &str) -> &'static str {
    match t {
        "null" => "null",
        "boolean" => "a boolean",
        "object" => "an object",
        "array" => "an array",
        "number" => "a number",
        "string" => "a string",
        _ => "an integer",
    }
}

fn has_type(v: &Node, t: &str) -> bool {
    use super::number::Number;
    match (t, &v.value) {
        ("null", Value::Null) | ("boolean", Value::Bool(_)) | ("string", Value::Str(_)) => true,
        ("object", Value::Object(..))
        | ("array", Value::Array(..))
        | ("number", Value::Number(_)) => true,
        ("integer", Value::Number(Number::Finite(d))) => d.is_integer(),
        _ => false,
    }
}

/// A value as a diagnostic quotes it.
fn show(v: &Node) -> String {
    match &v.value {
        Value::Array(..) => "the array".to_string(),
        Value::Object(..) => "the object".to_string(),
        _ => {
            let raw: String = v.raw.chars().take(40).collect();
            if raw.len() < v.raw.len() {
                format!("`{raw}…`")
            } else {
                format!("`{raw}`")
            }
        }
    }
}

/// JSON's equality: numbers by value, objects regardless of key order.
fn equal(a: &Node, b: &Node) -> bool {
    match (&a.value, &b.value) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Array(x, _), Value::Array(y, _)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| equal(a, b))
        }
        (Value::Object(x, _), Value::Object(y, _)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|m| b.get(&m.key).is_some_and(|other| equal(&m.value, other)))
        }
        _ => false,
    }
}

fn count(v: &Node) -> usize {
    match &v.value {
        Value::Number(super::number::Number::Finite(d)) => {
            usize::try_from(d.to_f64() as u64).unwrap_or(usize::MAX)
        }
        _ => 0,
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

impl<'a, 'r> Checker<'a, 'r> {
    fn violation(
        &self,
        out: &mut Vec<Violation>,
        schema: &Node,
        keyword: &str,
        span: Range,
        problem: String,
        remedy: impl Into<String>,
    ) {
        out.push(Violation {
            span,
            problem,
            remedy: remedy.into(),
            location: self.reg.keyword_location(schema, keyword),
        });
    }

    /// Whether `instance` passes `schema`, reporting nothing.
    fn passes(&mut self, schema: &'r Node, resource: usize, instance: &Node) -> (bool, Evaluated) {
        let mut scratch = Vec::new();
        let e = self.check(schema, resource, instance, &mut scratch);
        (scratch.is_empty(), e)
    }

    fn check(
        &mut self,
        schema: &'r Node,
        resource: usize,
        instance: &Node,
        out: &mut Vec<Violation>,
    ) -> Evaluated {
        let keywords = match (
            &schema.value,
            self.reg.keywords.get(&(schema as *const Node)),
        ) {
            (Value::Bool(true), _) => return Evaluated::default(),
            (Value::Bool(false), _) => {
                self.violation(
                    out,
                    schema,
                    "",
                    instance.span,
                    format!(
                        "the schema allows no value here, and this is {}",
                        kind(instance)
                    ),
                    "delete it",
                );
                return Evaluated::default();
            }
            (_, Some(keywords)) => keywords,
            (_, None) => return Evaluated::default(),
        };
        if self.depth > MAX_DEPTH {
            self.violation(
                out,
                schema,
                "$ref",
                instance.span,
                "the schema refers to itself without end at this value".to_string(),
                "break the loop of `$ref`s in the schema",
            );
            return Evaluated::default();
        }
        self.depth = self.depth.saturating_add(1);
        // A resource's root, or any schema in a resource a `$ref` just entered.
        let entered = self
            .reg
            .root_of
            .get(&(schema as *const Node))
            .copied()
            .or_else(|| (self.dynamic.last() != Some(&resource)).then_some(resource));
        let resource = entered.unwrap_or(resource);
        if let Some(r) = entered {
            self.dynamic.push(r);
        }
        let mut seen = Evaluated::default();
        for kw in keywords {
            self.keyword(schema, resource, kw, instance, out, &mut seen);
        }
        // Last, because they ask what everything else looked at.
        for Keyword { member: m, .. } in keywords {
            match m.key.as_str() {
                "unevaluatedProperties" => self
                    .unevaluated_properties(schema, resource, &m.value, instance, out, &mut seen),
                "unevaluatedItems" => {
                    self.unevaluated_items(schema, resource, &m.value, instance, out, &mut seen)
                }
                _ => {}
            }
        }
        if entered.is_some() {
            self.dynamic.pop();
        }
        self.depth = self.depth.saturating_sub(1);
        seen
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one arm per keyword, in the order the vocabularies list them"
    )]
    fn keyword(
        &mut self,
        schema: &'r Node,
        resource: usize,
        kw: &Keyword<'r>,
        v: &Node,
        out: &mut Vec<Violation>,
        seen: &mut Evaluated,
    ) {
        let k = kw.member.key.as_str();
        let s = &kw.member.value;
        match (k, &kw.kind, &v.value) {
            ("$ref", Kind::Ref(r), _) => {
                let e = self.check(r.target, r.resource, v, out);
                seen.merge(e);
            }
            ("$dynamicRef", Kind::Ref(r), _) => {
                let (resource, target) = self.dynamic_target(*r);
                let e = self.check(target, resource, v, out);
                seen.merge(e);
            }
            ("allOf", _, _) => {
                for sub in list(s) {
                    let e = self.check(sub, resource, v, out);
                    seen.merge(e);
                }
            }
            ("anyOf", _, _) => {
                let mut any = false;
                for sub in list(s) {
                    let (ok, e) = self.passes(sub, resource, v);
                    if ok {
                        any = true;
                        seen.merge(e);
                    }
                }
                if !any {
                    self.violation(
                        out,
                        schema,
                        k,
                        v.span,
                        format!("{} matches none of the schemas in `anyOf`", show(v)),
                        "change it to match one of them",
                    );
                }
            }
            ("oneOf", _, _) => {
                let mut matched = Vec::new();
                for sub in list(s) {
                    let (ok, e) = self.passes(sub, resource, v);
                    if ok {
                        matched.push(e);
                    }
                }
                match matched.len() {
                    1 => matched.into_iter().for_each(|e| seen.merge(e)),
                    0 => self.violation(
                        out,
                        schema,
                        k,
                        v.span,
                        format!("{} matches none of the schemas in `oneOf`", show(v)),
                        "change it to match exactly one of them",
                    ),
                    n => self.violation(
                        out,
                        schema,
                        k,
                        v.span,
                        format!(
                            "{} matches {n} of the schemas in `oneOf`, and may match only one",
                            show(v)
                        ),
                        "change it to match exactly one of them",
                    ),
                }
            }
            ("not", _, _) => {
                if self.passes(s, resource, v).0 {
                    self.violation(
                        out,
                        schema,
                        k,
                        v.span,
                        format!("{} matches the schema in `not`", show(v)),
                        "change it so it does not",
                    );
                }
            }
            ("if", _, _) => {
                let (ok, e) = self.passes(s, resource, v);
                let branch = if ok { "then" } else { "else" };
                if ok {
                    seen.merge(e);
                }
                if let Some(next) = schema.get(branch) {
                    let e = self.check(next, resource, v, out);
                    seen.merge(e);
                }
            }
            ("dependentSchemas", Kind::Entries(entries), Value::Object(..)) => {
                for e in *entries {
                    if v.get(&e.key).is_some() {
                        let ev = self.check(&e.value, resource, v, out);
                        seen.merge(ev);
                    }
                }
            }
            ("type", Kind::Type(names), _) => {
                if !names.iter().any(|t| has_type(v, t)) {
                    let expected = names
                        .iter()
                        .map(|t| type_phrase(t))
                        .collect::<Vec<_>>()
                        .join(" or ");
                    self.violation(
                        out,
                        schema,
                        k,
                        v.span,
                        format!("expected {expected}, found {}", kind(v)),
                        format!("write {expected} here"),
                    );
                }
            }
            ("enum", _, _) => {
                let allowed = list(s);
                if !allowed.iter().any(|a| equal(a, v)) {
                    let mut shown: Vec<String> = allowed
                        .iter()
                        .take(10)
                        .map(|a| format!("`{}`", a.raw.chars().take(40).collect::<String>()))
                        .collect();
                    if allowed.len() > 10 {
                        shown.push("…".to_string());
                    }
                    let values = shown.join(", ");
                    self.violation(
                        out,
                        schema,
                        k,
                        v.span,
                        format!(
                            "{} is not one of the values `enum` allows: {values}",
                            show(v)
                        ),
                        "write one of the allowed values",
                    );
                }
            }
            ("const", _, _) => {
                if !equal(s, v) {
                    self.violation(
                        out,
                        schema,
                        k,
                        v.span,
                        format!("expected {}, found {}", show(s), show(v)),
                        format!("write {}", show(s)),
                    );
                }
            }
            (_, Kind::Number(bound), Value::Number(n)) => {
                self.number(schema, kw.member, bound, n, v, out);
            }
            ("maxLength" | "minLength", _, Value::Str(text)) => {
                let len = text.chars().count();
                let bound = count(s);
                let (bad, word) = if k == "maxLength" {
                    (len > bound, "at most")
                } else {
                    (len < bound, "at least")
                };
                if bad {
                    self.violation(
                        out,
                        schema,
                        k,
                        v.span,
                        format!(
                            "the string is {} long, and the schema asks for {word} {bound}",
                            plural(len, "character", "characters")
                        ),
                        format!(
                            "write a string of {word} {}",
                            plural(bound, "character", "characters")
                        ),
                    );
                }
            }
            ("pattern", Kind::Pattern(pattern, regex), Value::Str(text)) => {
                self.pattern(schema, pattern, regex, text, v, out);
            }
            ("prefixItems", _, Value::Array(items, _)) => {
                for (i, (sub, item)) in list(s).iter().zip(items).enumerate() {
                    self.check(sub, resource, item, out);
                    seen.items.insert(i);
                }
            }
            ("items", _, Value::Array(items, _)) => {
                let skip = schema.get("prefixItems").map_or(0, |p| list(p).len());
                for (i, item) in items.iter().enumerate().skip(skip) {
                    if matches!(s.value, Value::Bool(false)) {
                        self.violation(
                            out,
                            schema,
                            k,
                            item.span,
                            format!(
                                "the array may hold {}, and this is item {}",
                                plural(skip, "item", "items"),
                                i.saturating_add(1)
                            ),
                            "delete it",
                        );
                    } else {
                        self.check(s, resource, item, out);
                    }
                }
                seen.all_items = true;
            }
            ("contains", _, Value::Array(items, _)) => {
                let mut matched = 0usize;
                for (i, item) in items.iter().enumerate() {
                    if self.passes(s, resource, item).0 {
                        matched = matched.saturating_add(1);
                        seen.items.insert(i);
                    }
                }
                let min = schema.get("minContains").map_or(1, count);
                let max = schema.get("maxContains").map(count);
                if matched < min {
                    let found = if matched == 0 {
                        "no item matches".to_string()
                    } else {
                        format!("{} match", plural(matched, "item", "items"))
                    };
                    self.violation(
                        out,
                        schema,
                        k,
                        v.span,
                        format!("{found} `contains`, and the schema asks for at least {min}"),
                        "add an item that matches `contains`",
                    );
                }
                if let Some(max) = max.filter(|max| matched > *max) {
                    self.violation(
                        out,
                        schema,
                        "maxContains",
                        v.span,
                        format!(
                            "{} match `contains`, and the schema allows at most {max}",
                            plural(matched, "item", "items")
                        ),
                        "remove the items that match",
                    );
                }
            }
            ("maxItems" | "minItems", _, Value::Array(items, _)) => {
                let bound = count(s);
                let (bad, word) = if k == "maxItems" {
                    (items.len() > bound, "at most")
                } else {
                    (items.len() < bound, "at least")
                };
                if bad {
                    self.violation(
                        out,
                        schema,
                        k,
                        v.span,
                        format!(
                            "the array has {}, and the schema asks for {word} {bound}",
                            plural(items.len(), "item", "items")
                        ),
                        format!("write {word} {}", plural(bound, "item", "items")),
                    );
                }
            }
            ("uniqueItems", _, Value::Array(items, _)) => {
                if matches!(s.value, Value::Bool(true)) {
                    'outer: for (j, b) in items.iter().enumerate() {
                        for (i, a) in items.iter().enumerate().take(j) {
                            if equal(a, b) {
                                self.violation(
                                    out,
                                    schema,
                                    k,
                                    b.span,
                                    format!(
                                        "item {} repeats item {}, and the items must be unique",
                                        j.saturating_add(1),
                                        i.saturating_add(1)
                                    ),
                                    "delete one of them",
                                );
                                break 'outer;
                            }
                        }
                    }
                }
            }
            ("properties", _, Value::Object(members, _)) => {
                for member in members {
                    if let Some(sub) = s.get(&member.key) {
                        self.check(sub, resource, &member.value, out);
                        seen.props.insert(member.key.clone());
                    }
                }
            }
            ("patternProperties", Kind::Entries(patterns), Value::Object(members, _)) => {
                for member in members {
                    for p in *patterns {
                        if self
                            .reg
                            .regexes
                            .get(&p.key)
                            .is_some_and(|r| r.is_match(&member.key) == Ok(true))
                        {
                            self.check(&p.value, resource, &member.value, out);
                            seen.props.insert(member.key.clone());
                        }
                    }
                }
            }
            ("additionalProperties", _, Value::Object(members, _)) => {
                for member in members {
                    let declared = schema
                        .get("properties")
                        .is_some_and(|p| p.get(&member.key).is_some());
                    let patterned = match schema.get("patternProperties").map(|p| &p.value) {
                        Some(Value::Object(patterns, _)) => patterns.iter().any(|p| {
                            self.reg
                                .regexes
                                .get(&p.key)
                                .is_some_and(|r| r.is_match(&member.key) == Ok(true))
                        }),
                        _ => false,
                    };
                    if declared || patterned {
                        continue;
                    }
                    self.extra_property(schema, resource, k, s, member, out);
                }
                seen.all_props = true;
            }
            ("propertyNames", _, Value::Object(members, _)) => {
                for member in members {
                    let key = Node {
                        value: Value::Str(member.key.clone()),
                        span: member.key_span,
                        raw: member.key_raw.clone(),
                        leading: Vec::new(),
                        trailing: Vec::new(),
                        blank_before: false,
                    };
                    self.check(s, resource, &key, out);
                }
            }
            ("maxProperties" | "minProperties", _, Value::Object(members, _)) => {
                let bound = count(s);
                let (bad, word) = if k == "maxProperties" {
                    (members.len() > bound, "at most")
                } else {
                    (members.len() < bound, "at least")
                };
                if bad {
                    self.violation(
                        out,
                        schema,
                        k,
                        v.span,
                        format!(
                            "the object has {}, and the schema asks for {word} {bound}",
                            plural(members.len(), "property", "properties")
                        ),
                        format!("write {word} {}", plural(bound, "property", "properties")),
                    );
                }
            }
            ("required", _, Value::Object(..)) => {
                for name in list(s).iter().filter_map(|n| n.as_str()) {
                    if v.get(name).is_none() {
                        let open = (v.span.0, v.span.0.saturating_add(1));
                        self.violation(
                            out,
                            schema,
                            k,
                            open,
                            format!("the object has no `\"{name}\"`"),
                            format!("add `\"{name}\"`"),
                        );
                    }
                }
            }
            ("dependentRequired", Kind::Entries(entries), Value::Object(members, _)) => {
                for e in *entries {
                    let Some(present) = members.iter().find(|m| m.key == e.key) else {
                        continue;
                    };
                    for name in list(&e.value).iter().filter_map(|n| n.as_str()) {
                        if v.get(name).is_none() {
                            self.violation(
                                out,
                                schema,
                                k,
                                present.key_span,
                                format!("`\"{name}\"` is required when `\"{}\"` is present", e.key),
                                format!("add `\"{name}\"`, or delete `\"{}\"`", e.key),
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn extra_property(
        &mut self,
        schema: &'r Node,
        resource: usize,
        k: &str,
        s: &'r Node,
        member: &Member,
        out: &mut Vec<Violation>,
    ) {
        if matches!(s.value, Value::Bool(false)) {
            self.violation(
                out,
                schema,
                k,
                member.key_span,
                format!("`{}` is not a property the schema declares", member.key_raw),
                "delete it, or declare it in the schema's `properties`",
            );
        } else {
            self.check(s, resource, &member.value, out);
        }
    }

    fn unevaluated_properties(
        &mut self,
        schema: &'r Node,
        resource: usize,
        s: &'r Node,
        v: &Node,
        out: &mut Vec<Violation>,
        seen: &mut Evaluated,
    ) {
        let Value::Object(members, _) = &v.value else {
            return;
        };
        if seen.all_props {
            return;
        }
        for member in members {
            if !seen.props.contains(&member.key) {
                self.extra_property(schema, resource, "unevaluatedProperties", s, member, out);
            }
        }
        seen.all_props = true;
    }

    fn unevaluated_items(
        &mut self,
        schema: &'r Node,
        resource: usize,
        s: &'r Node,
        v: &Node,
        out: &mut Vec<Violation>,
        seen: &mut Evaluated,
    ) {
        let Value::Array(items, _) = &v.value else {
            return;
        };
        if seen.all_items {
            return;
        }
        for (i, item) in items.iter().enumerate() {
            if seen.items.contains(&i) {
                continue;
            }
            if matches!(s.value, Value::Bool(false)) {
                self.violation(
                    out,
                    schema,
                    "unevaluatedItems",
                    item.span,
                    format!(
                        "item {} is not one the schema describes",
                        i.saturating_add(1)
                    ),
                    "delete it, or describe it in the schema",
                );
            } else {
                self.check(s, resource, item, out);
            }
        }
        seen.all_items = true;
    }

    fn number(
        &mut self,
        schema: &Node,
        member: &Member,
        bound: &Decimal,
        n: &super::number::Number,
        v: &Node,
        out: &mut Vec<Violation>,
    ) {
        use super::number::Number;
        let (k, s) = (member.key.as_str(), &member.value);
        let Number::Finite(x) = n else {
            // `Infinity` and `NaN` pass `type: number` and nothing else.
            self.violation(
                out,
                schema,
                k,
                v.span,
                format!("{} is not a finite number, and `{k}` asks for one", show(v)),
                "write a finite number",
            );
            return;
        };
        let (bad, problem) = match k {
            "multipleOf" => (
                !x.is_multiple_of(bound),
                format!("{} is not a multiple of `{}`", show(v), s.raw),
            ),
            "maximum" => (
                x > bound,
                format!("{} is greater than the maximum, `{}`", show(v), s.raw),
            ),
            "exclusiveMaximum" => (
                x >= bound,
                format!(
                    "{} is not less than `{}`, the exclusive maximum",
                    show(v),
                    s.raw
                ),
            ),
            "minimum" => (
                x < bound,
                format!("{} is less than the minimum, `{}`", show(v), s.raw),
            ),
            _ => (
                x <= bound,
                format!(
                    "{} is not greater than `{}`, the exclusive minimum",
                    show(v),
                    s.raw
                ),
            ),
        };
        if bad {
            self.violation(
                out,
                schema,
                k,
                v.span,
                problem,
                format!("write a number `{k}` allows"),
            );
        }
    }

    fn pattern(
        &mut self,
        schema: &Node,
        pattern: &str,
        regex: &Regex,
        text: &str,
        v: &Node,
        out: &mut Vec<Violation>,
    ) {
        let k = "pattern";
        match regex.is_match(text) {
            Ok(true) => {}
            Ok(false) => self.violation(
                out,
                schema,
                k,
                v.span,
                format!("{} does not match the pattern `{pattern}`", show(v)),
                "write a string the pattern matches",
            ),
            Err(TooCostly) => self.violation(
                out,
                schema,
                k,
                v.span,
                format!(
                    "the pattern `{pattern}` is too costly to match against {}",
                    show(v)
                ),
                "simplify the pattern",
            ),
        }
    }

    /// `$dynamicRef`: the outermost resource in scope with a matching
    /// `$dynamicAnchor`, when the reference first lands on one.
    fn dynamic_target(&self, reference: Reference<'r>) -> (usize, &'r Node) {
        let (r, target) = (reference.resource, reference.target);
        let Some(name) = reference.written.split_once('#').map(|(_, f)| f) else {
            return (r, target);
        };
        let dynamic = self
            .reg
            .dynamic_anchors
            .get(&(r, name.to_string()))
            .is_some_and(|n| std::ptr::eq(*n, target));
        if !dynamic {
            return (r, target);
        }
        for scope in &self.dynamic {
            if let Some(n) = self.reg.dynamic_anchors.get(&(*scope, name.to_string())) {
                return (*scope, n);
            }
        }
        (r, target)
    }
}

fn list(v: &Node) -> Vec<&Node> {
    match &v.value {
        Value::Array(items, _) => items.iter().collect(),
        _ => Vec::new(),
    }
}
