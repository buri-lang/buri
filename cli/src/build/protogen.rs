//! A `.proto` schema, as a Buri module.
//!
//! The mapping this writes is normative, and it is written down in
//! `cli/src/docs/build/proto.md` rather than left to be read off the output.
//! In summary:
//!
//! ```text
//! message Person            -> struct Person { ... }, derive Eq, Show
//! optional T f              -> Option<T>
//! repeated T f              -> [T]
//! T f (proto3 singular)     -> T, defaulted when absent
//! Message f (singular)      -> Option<Message>, because a message always has
//!                              explicit presence in proto3
//! oneof pick { ... }        -> enum Person_Pick, held as Option<Person_Pick>
//! message Outer { message Inner } -> Outer and Outer_Inner, side by side
//! enum Colour               -> enum Colour, value names verbatim
//! ```
//!
//! Four codecs come with each message, and they are *generated Buri* rather
//! than a descriptor walk in the runtime. `core/proto` says why: the descriptor
//! `$json_of` walks carries field names and variant shapes, and a protobuf
//! message is made of field numbers and wire types, which it does not carry at
//! all.

use std::collections::BTreeMap;

use crate::build::protoschema::{EnumDef, Field, Label, Message, Scalar, Schema, TypeRef};
use crate::diagnostics::{Diagnostic, Span};

/// A generated module.
pub struct Generated {
    pub source: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Message,
    Enum,
}

/// One declared type, wherever it came from.
#[derive(Clone, Debug)]
struct Entry {
    /// The Buri name: nesting flattened with `_`.
    buri: String,
    kind: Kind,
    /// The generated module this type lives in, when it is not this one.
    module: Option<String>,
    /// An enum's zero value, which is its default and what an unrecognised
    /// number decodes to.
    zero: Option<String>,
}

/// Where a field's type landed.
#[derive(Clone, Debug)]
enum FTy {
    Scalar(Scalar),
    Named(Entry),
}

impl FTy {
    fn wire(&self) -> &'static str {
        match self {
            FTy::Scalar(s) => match s {
                Scalar::Fixed64 | Scalar::Sfixed64 | Scalar::Double => "proto.I64",
                Scalar::Fixed32 | Scalar::Sfixed32 | Scalar::Float => "proto.I32",
                Scalar::Str | Scalar::Bytes => "proto.LEN",
                _ => "proto.VARINT",
            },
            FTy::Named(e) => match e.kind {
                Kind::Enum => "proto.VARINT",
                Kind::Message => "proto.LEN",
            },
        }
    }

    /// A repeated field of this type is packed by default. Nothing
    /// length-delimited is: packing strings would be indistinguishable from
    /// one long string.
    fn packable(&self) -> bool {
        self.wire() != "proto.LEN"
    }

    fn buri(&self) -> String {
        match self {
            FTy::Scalar(s) => match s {
                Scalar::Bool => "Bool".into(),
                Scalar::Str => "Str".into(),
                Scalar::Bytes => "[U8]".into(),
                Scalar::Double | Scalar::Float => "Float".into(),
                _ => "Int".into(),
            },
            FTy::Named(e) => e.buri.clone(),
        }
    }
}

/// A field with its type resolved and its two names settled.
#[derive(Clone)]
struct RField {
    /// The Buri field or variant name.
    name: String,
    /// The name proto3 JSON writes. The same string unless it collided with a
    /// Buri keyword.
    json: String,
    /// The name the schema wrote, which proto3 JSON also *accepts* on the way
    /// in. The same string as `json` for a field whose name is already camel.
    proto: String,
    number: i64,
    label: Label,
    ty: FTy,
    /// Whether a repeated field of a packable type is written packed. proto3's
    /// default is `true`, and `[packed = false]` is what turns it off.
    packed: bool,
}

impl RField {
    /// A singular message field is `Option<T>` whatever its label says: in
    /// proto3 a message field always tracks presence, and there is no "default
    /// message" for an absent one to mean.
    fn optional(&self) -> bool {
        self.label == Label::Optional || (self.label == Label::Single && self.is_message())
    }

    fn is_message(&self) -> bool {
        matches!(&self.ty, FTy::Named(e) if e.kind == Kind::Message)
    }

    fn buri_type(&self) -> String {
        let base = self.ty.buri();
        match self.label {
            Label::Repeated => format!("[{base}]"),
            _ if self.optional() => format!("Option<{base}>"),
            _ => base,
        }
    }
}

/// A `oneof`, resolved.
struct ROneof {
    /// The Buri field on the enclosing struct.
    field: String,
    /// The Buri enum the cases became.
    enum_name: String,
    cases: Vec<RField>,
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

/// Every word a Buri identifier may not be: the keywords, and the words
/// reserved for a future version. A field called `type` becomes `type_`; its
/// JSON name is untouched, because the document is not ours to rename.
const TAKEN: &[&str] = &[
    "as", "const", "context", "ctx", "derive", "effect", "else", "enum", "export", "false", "fn",
    "for", "from", "if", "impl", "import", "let", "match", "self", "Self", "struct", "test",
    "trait", "true", "type", "async", "await", "break", "continue", "do", "in", "is", "loop",
    "module", "mut", "opaque", "panic", "pub", "return", "unreachable", "use", "when", "where",
    "while", "with", "yield",
];

/// protoc's rule, exactly: drop each `_` and capitalise what follows it.
/// Nothing else changes case, so a schema's `URL` stays `URL` and the JSON
/// name this produces is the one protoc would put in the descriptor.
pub fn camel_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut up = false;
    for c in name.chars() {
        if c == '_' {
            up = true;
            continue;
        }
        if up {
            out.extend(c.to_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn escape(name: &str) -> String {
    if TAKEN.contains(&name) {
        format!("{name}_")
    } else {
        name.to_string()
    }
}

/// `pick` -> `Pick`, for the enum a `oneof` becomes and for its cases.
fn upper_first(name: &str) -> String {
    let mut cs = name.chars();
    match cs.next() {
        Some(c) => c.to_uppercase().collect::<String>() + cs.as_str(),
        None => name.to_string(),
    }
}

// ---------------------------------------------------------------------------
// The type table
// ---------------------------------------------------------------------------

/// Every message and enum a set of schemas declares, under every name a field
/// could reasonably use to reach it.
#[derive(Default)]
struct Table {
    by_name: BTreeMap<String, Entry>,
}

impl Table {
    fn add(&mut self, key: String, entry: Entry) {
        self.by_name.entry(key).or_insert(entry);
    }

    fn collect(&mut self, schema: &Schema, module: Option<&str>) {
        let pkg = schema.package.clone().unwrap_or_default();
        for e in &schema.enums {
            self.add_enum(e, &[], &pkg, module);
        }
        for m in &schema.messages {
            self.add_message(m, &[], &pkg, module);
        }
    }

    fn keys(scope: &[String], name: &str, pkg: &str) -> Vec<String> {
        let mut path = scope.to_vec();
        path.push(name.to_string());
        let relative = path.join(".");
        let mut out = vec![relative.clone(), name.to_string()];
        if !pkg.is_empty() {
            out.insert(0, format!("{pkg}.{relative}"));
        }
        out
    }

    fn add_enum(&mut self, e: &EnumDef, scope: &[String], pkg: &str, module: Option<&str>) {
        let mut path = scope.to_vec();
        path.push(e.name.clone());
        let entry = Entry {
            buri: path.join("_"),
            kind: Kind::Enum,
            module: module.map(str::to_string),
            zero: zero_value(e),
        };
        for key in Table::keys(scope, &e.name, pkg) {
            self.add(key, entry.clone());
        }
    }

    fn add_message(&mut self, m: &Message, scope: &[String], pkg: &str, module: Option<&str>) {
        let mut path = scope.to_vec();
        path.push(m.name.clone());
        let entry = Entry {
            buri: path.join("_"),
            kind: Kind::Message,
            module: module.map(str::to_string),
            zero: None,
        };
        for key in Table::keys(scope, &m.name, pkg) {
            self.add(key, entry.clone());
        }
        for e in &m.enums {
            self.add_enum(e, &path, pkg, module);
        }
        for inner in &m.messages {
            self.add_message(inner, &path, pkg, module);
        }
    }

    /// proto's name resolution, narrowed to what a schema this small needs: a
    /// leading `.` is absolute, and everything else is tried from the innermost
    /// enclosing message outward.
    fn resolve(&self, name: &str, scope: &[String], pkg: &str) -> Option<Entry> {
        if let Some(absolute) = name.strip_prefix('.') {
            return self.by_name.get(absolute).cloned();
        }
        for i in (0..=scope.len()).rev() {
            let mut path: Vec<String> = scope[..i].to_vec();
            path.push(name.to_string());
            let joined = path.join(".");
            if let Some(e) = self.by_name.get(&joined) {
                return Some(e.clone());
            }
            if !pkg.is_empty() {
                if let Some(e) = self.by_name.get(&format!("{pkg}.{joined}")) {
                    return Some(e.clone());
                }
            }
        }
        self.by_name.get(name).cloned()
    }
}

fn zero_value(e: &EnumDef) -> Option<String> {
    e.values
        .iter()
        .find(|v| v.number == 0)
        .or_else(|| e.values.first())
        .map(|v| escape(&v.name))
}

// ---------------------------------------------------------------------------
// Generating
// ---------------------------------------------------------------------------

/// Turns one schema into one module.
///
/// `deps` is the schema of every file this one imports, with the module path
/// its generated form will have — the loader has already read them, because a
/// field's type may live in any of them.
pub fn generate(
    origin: &str,
    schema: &Schema,
    deps: &[(String, Schema)],
    diags: &mut Vec<Diagnostic>,
) -> Generated {
    let mut table = Table::default();
    table.collect(schema, None);
    for (module, dep) in deps {
        table.collect(dep, Some(module));
    }

    let mut g = Gen {
        table,
        pkg: schema.package.clone().unwrap_or_default(),
        out: String::new(),
        used: BTreeMap::new(),
        diags,
    };

    for e in &schema.enums {
        g.enum_type(e, &[]);
    }
    for m in &schema.messages {
        g.message(m, &[]);
    }
    let body = std::mem::take(&mut g.out);

    let mut source = String::new();
    source.push_str(&format!(
        "//! Generated from {origin}. Not a file to edit: the schema is the source,\n\
         //! and this module is rebuilt from it whenever the schema changes.\n\
         //!\n\
         //! What the mapping is, and why: `buri docs build/proto`.\n\n"
    ));
    source.push_str("from \"core/cap\" import { Alloc };\n");
    source.push_str("from \"core/bytes\" import * as bytes;\n");
    source.push_str("from \"core/json\" import { Json };\n");
    source.push_str("from \"core/list\" import * as list;\n");
    source.push_str("from \"core/proto\" import { ProtoError };\n");
    source.push_str("from \"core/proto\" import * as proto;\n");
    // One import line per foreign module, naming the types used and the codecs
    // that come with them.
    let mut foreign: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in g.used.values() {
        let Some(module) = &entry.module else { continue };
        let names = foreign.entry(module.clone()).or_default();
        names.push(entry.buri.clone());
        names.extend(codec_names(entry));
    }
    for (module, mut names) in foreign {
        names.sort();
        names.dedup();
        source.push_str(&format!("from \"{module}\" import {{ {} }};\n", names.join(", ")));
    }
    source.push('\n');
    source.push_str(&body);
    Generated { source }
}

fn codec_names(entry: &Entry) -> Vec<String> {
    let n = &entry.buri;
    match entry.kind {
        Kind::Message => vec![
            format!("encode{n}"),
            format!("decode{n}"),
            format!("merge{n}"),
            format!("encode{n}Json"),
            format!("decode{n}JsonAt"),
        ],
        Kind::Enum => vec![
            format!("encode{n}"),
            format!("decode{n}"),
            format!("encode{n}Json"),
            format!("decode{n}Json"),
        ],
    }
}

struct Gen<'a> {
    table: Table,
    pkg: String,
    out: String,
    /// Every type actually referenced, so the import lines name exactly those.
    used: BTreeMap<String, Entry>,
    diags: &'a mut Vec<Diagnostic>,
}

/// An `if`/`else if`/`else` chain, written once so no generator has to get the
/// braces right twice.
fn if_chain(indent: &str, arms: &[(String, Vec<String>)], otherwise: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if arms.is_empty() {
        for l in otherwise {
            out.push(format!("{indent}{l}"));
        }
        return out;
    }
    for (i, (test, body)) in arms.iter().enumerate() {
        let head = if i == 0 { format!("{indent}if ({test}) {{") } else { format!("{indent}}} else if ({test}) {{") };
        out.push(head);
        for l in body {
            out.push(format!("{indent}  {l}"));
        }
    }
    out.push(format!("{indent}}} else {{"));
    for l in otherwise {
        out.push(format!("{indent}  {l}"));
    }
    out.push(format!("{indent}}}"));
    out
}

impl<'a> Gen<'a> {
    fn line(&mut self, s: &str) {
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn lines(&mut self, ls: &[String]) {
        for l in ls {
            self.line(l);
        }
    }

    fn resolve(&mut self, f: &Field, scope: &[String]) -> Option<FTy> {
        match &f.ty {
            TypeRef::Scalar(s) => Some(FTy::Scalar(*s)),
            TypeRef::Named(name) => match self.table.resolve(name, scope, &self.pkg) {
                Some(e) => {
                    self.used.insert(e.buri.clone(), e.clone());
                    Some(FTy::Named(e))
                }
                None => {
                    let name = name.clone();
                    self.diags.push(
                        Diagnostic::error(f.span, format!("`{name}` names no message or enum"))
                            .with_code("proto-unknown-type")
                            .with_fix(
                                "declare it in this file, or `import` the file that declares it — \
                                 an import path is written from the repository root",
                            ),
                    );
                    None
                }
            },
        }
    }

    fn rfield(&mut self, f: &Field, ty: FTy) -> RField {
        let json = camel_case(&f.name);
        RField {
            name: escape(&json),
            json: json.clone(),
            proto: f.name.clone(),
            number: f.number,
            label: f.label,
            ty,
            packed: f.packed.unwrap_or(true),
        }
    }

    fn resolved(&mut self, m: &Message, scope: &[String], name: &str) -> (Vec<RField>, Vec<ROneof>) {
        let mut fields = Vec::new();
        for f in &m.fields {
            if let Some(ty) = self.resolve(f, scope) {
                fields.push(self.rfield(f, ty));
            }
        }
        let mut oneofs = Vec::new();
        for o in &m.oneofs {
            let mut cases = Vec::new();
            for f in &o.fields {
                if let Some(ty) = self.resolve(f, scope) {
                    cases.push(self.rfield(f, ty));
                }
            }
            // A oneof with no cases would be an uninhabited enum, which the
            // language rejects and which says nothing anyway.
            if cases.is_empty() {
                continue;
            }
            oneofs.push(ROneof {
                field: escape(&camel_case(&o.name)),
                enum_name: format!("{name}_{}", upper_first(&camel_case(&o.name))),
                cases,
            });
        }
        (fields, oneofs)
    }

    // -- declarations ------------------------------------------------------

    fn message(&mut self, m: &Message, scope: &[String]) {
        let mut path = scope.to_vec();
        path.push(m.name.clone());
        let name = path.join("_");
        let (fields, oneofs) = self.resolved(m, &path, &name);

        for o in &oneofs {
            self.line(&format!("/// The cases of `{name}`'s `{}`.", o.field));
            self.line(&format!("export enum {} {{", o.enum_name));
            for c in &o.cases {
                self.line(&format!("  export {}({}),", upper_first(&c.name), c.ty.buri()));
            }
            self.line("}");
            self.line("");
            self.line(&format!("derive Eq, Show for {};", o.enum_name));
            self.line("");
        }

        self.line(&format!("export struct {name} {{"));
        for f in &fields {
            self.line(&format!("  export {}: {},", f.name, f.buri_type()));
        }
        for o in &oneofs {
            self.line(&format!("  export {}: Option<{}>,", o.field, o.enum_name));
        }
        self.line("}");
        self.line("");
        self.line(&format!("derive Eq, Show for {name};"));
        self.line("");

        self.default_fn(&name, &fields, &oneofs);
        self.encode_fn(&name, &fields, &oneofs);
        self.decode_fn(&name, &fields, &oneofs);
        self.json_encode_fn(&name, &fields, &oneofs);
        self.json_decode_fn(&name, &fields, &oneofs);

        for e in &m.enums {
            self.enum_type(e, &path);
        }
        for inner in &m.messages {
            self.message(inner, &path);
        }
    }

    fn enum_type(&mut self, e: &EnumDef, scope: &[String]) {
        let mut path = scope.to_vec();
        path.push(e.name.clone());
        let name = path.join("_");
        let zero = zero_value(e).unwrap_or_else(|| "UNSPECIFIED".to_string());
        // A schema whose enum has no values is already an error; emitting one
        // value keeps the module parseable so the real diagnostic is the one a
        // reader sees.
        let values: Vec<(String, String, i64)> = if e.values.is_empty() {
            vec![(zero.clone(), zero.clone(), 0)]
        } else {
            e.values.iter().map(|v| (escape(&v.name), v.name.clone(), v.number)).collect()
        };

        self.line(&format!("export enum {name} {{"));
        for (buri, _, _) in &values {
            self.line(&format!("  export {buri},"));
        }
        self.line("}");
        self.line("");
        self.line(&format!("derive Eq, Show for {name};"));
        self.line("");

        self.line(&format!("/// The number `{name}` goes on the wire as."));
        self.line(&format!("export fn encode{name}(value: {name}): Int {{"));
        self.line("  match (value) {");
        for (buri, _, number) in &values {
            self.line(&format!("    .{buri} => {number},"));
        }
        self.line("  }");
        self.line("}");
        self.line("");

        self.line(&format!(
            "/// A number this schema does not know decodes to `{zero}`, the zero value."
        ));
        self.line("///");
        self.line("/// A deliberate loss. proto3 asks a reader to *keep* an unrecognised number;");
        self.line("/// a Buri enum has nowhere to keep one. Failing instead would mean that");
        self.line("/// adding a value to an enum broke every reader built before it — which is");
        self.line("/// the thing proto3's rule exists to prevent.");
        self.line(&format!("export fn decode{name}(number: Int): {name} {{"));
        // An alias — two names for one number — decodes to the first of them.
        let mut seen: Vec<i64> = Vec::new();
        let mut arms: Vec<(String, Vec<String>)> = Vec::new();
        for (buri, _, number) in &values {
            if seen.contains(number) {
                continue;
            }
            seen.push(*number);
            arms.push((format!("number == {number}"), vec![format!("{name}.{buri}")]));
        }
        let chain = if_chain("  ", &arms, &[format!("{name}.{zero}")]);
        self.lines(&chain);
        self.line("}");
        self.line("");

        self.line("/// proto3 JSON writes an enum as the *name* of its value.");
        self.line(&format!("export fn encode{name}Json(value: {name}): Json {{"));
        self.line("  match (value) {");
        for (buri, proto_name, _) in &values {
            self.line(&format!("    .{buri} => Json.Str(\"{proto_name}\"),"));
        }
        self.line("  }");
        self.line("}");
        self.line("");

        self.line("/// A name, or — as proto3 JSON also allows — the number.");
        self.line(&format!(
            "export fn decode{name}Json(doc: Json, path: Str): Result<{name}, ProtoError> {{"
        ));
        self.line(&format!("  match (doc) {{"));
        self.line("    .Str(s) => {");
        let mut arms: Vec<(String, Vec<String>)> = Vec::new();
        for (buri, proto_name, _) in &values {
            arms.push((
                format!("s == \"{proto_name}\""),
                vec![format!(".Ok({name}.{buri})")],
            ));
        }
        let chain = if_chain(
            "      ",
            &arms,
            &[format!(".Err(.BadJson {{ path: path, wanted: \"a value of {name}\" }})")],
        );
        self.lines(&chain);
        self.line("    },");
        self.line(&format!("    .Num(x) => .Ok(decode{name}(x.wrapToI64())),"));
        self.line(&format!(
            "    _ => .Err(.BadJson {{ path: path, wanted: \"a value of {name}\" }}),"
        ));
        self.line("  }");
        self.line("}");
        self.line("");
    }

    // -- defaults ----------------------------------------------------------

    fn default_of(&self, f: &RField) -> String {
        if f.label == Label::Repeated {
            return format!("list.empty::<{}>()", f.ty.buri());
        }
        if f.optional() {
            return ".None".into();
        }
        match &f.ty {
            FTy::Scalar(Scalar::Bool) => "false".into(),
            FTy::Scalar(Scalar::Str) => "\"\"".into(),
            FTy::Scalar(Scalar::Bytes) => "list.empty::<U8>()".into(),
            FTy::Scalar(Scalar::Double | Scalar::Float) => "0.0".into(),
            FTy::Scalar(_) => "0".into(),
            FTy::Named(e) => {
                format!("{}.{}", e.buri, e.zero.clone().unwrap_or_else(|| "UNSPECIFIED".into()))
            }
        }
    }

    fn default_fn(&mut self, name: &str, fields: &[RField], oneofs: &[ROneof]) {
        self.line("/// Every field at its proto3 default: what a message of no bytes decodes to.");
        self.line(&format!("export fn default{name}(): {name} {{"));
        if fields.is_empty() && oneofs.is_empty() {
            self.line(&format!("  {name} {{}}"));
        } else {
            self.line(&format!("  {name} {{"));
            for f in fields {
                let d = self.default_of(f);
                self.line(&format!("    {}: {d},", f.name));
            }
            for o in oneofs {
                self.line(&format!("    {}: .None,", o.field));
            }
            self.line("  }");
        }
        self.line("}");
        self.line("");
    }

    // -- the binary encoder -------------------------------------------------

    /// The bytes of one whole field — header included — given an expression
    /// holding its value.
    fn write_one(&self, f: &RField, value: &str, ctx: &str) -> String {
        let n = f.number;
        match &f.ty {
            FTy::Scalar(s) => match s {
                Scalar::Sint32 | Scalar::Sint64 => {
                    format!("proto.varintField({ctx}, {n}, bytes.zigzag({value}))")
                }
                Scalar::Bool => {
                    format!("proto.varintField({ctx}, {n}, if ({value}) {{ 1 }} else {{ 0 }})")
                }
                Scalar::Fixed32 | Scalar::Sfixed32 => {
                    format!("proto.fixed32Field({ctx}, {n}, {value})")
                }
                Scalar::Fixed64 | Scalar::Sfixed64 => {
                    format!("proto.fixed64Field({ctx}, {n}, {value})")
                }
                Scalar::Double => format!("proto.f64Field({ctx}, {n}, {value})"),
                Scalar::Float => format!("proto.f32Field({ctx}, {n}, {value})"),
                Scalar::Str => format!("proto.strField({ctx}, {n}, {value})"),
                Scalar::Bytes => format!("proto.bytesField({ctx}, {n}, {value})"),
                _ => format!("proto.varintField({ctx}, {n}, {value})"),
            },
            FTy::Named(e) => match e.kind {
                Kind::Enum => format!("proto.varintField({ctx}, {n}, encode{}({value}))", e.buri),
                Kind::Message => {
                    format!("proto.bytesField({ctx}, {n}, encode{}({ctx}, {value}))", e.buri)
                }
            },
        }
    }

    /// One element of a packed repeated field: the value with no header.
    fn write_raw(&self, f: &RField, value: &str, ctx: &str) -> String {
        match &f.ty {
            FTy::Scalar(s) => match s {
                Scalar::Sint32 | Scalar::Sint64 => {
                    format!("bytes.toVarint({ctx}, bytes.zigzag({value}))")
                }
                Scalar::Bool => {
                    format!("bytes.toVarint({ctx}, if ({value}) {{ 1 }} else {{ 0 }})")
                }
                Scalar::Fixed32 | Scalar::Sfixed32 => format!("proto.le32({value})"),
                Scalar::Fixed64 | Scalar::Sfixed64 => format!("proto.le64({ctx}, {value})"),
                Scalar::Double => format!("bytes.f64ToBytes({ctx}, {value})"),
                Scalar::Float => format!("bytes.f32ToBytes({ctx}, {value})"),
                _ => format!("bytes.toVarint({ctx}, {value})"),
            },
            FTy::Named(e) => format!("bytes.toVarint({ctx}, encode{}({value}))", e.buri),
        }
    }

    /// The test that decides whether a singular field is written at all.
    /// proto3 omits a field holding its default, and that is what makes an
    /// encoding canonical rather than merely correct.
    fn is_default(&self, f: &RField, value: &str) -> String {
        match &f.ty {
            FTy::Scalar(Scalar::Bool) => format!("{value} == false"),
            FTy::Scalar(Scalar::Str) => format!("{value} == \"\""),
            FTy::Scalar(Scalar::Bytes) => format!("{value}.len() == 0"),
            FTy::Scalar(Scalar::Double | Scalar::Float) => format!("{value} == 0.0"),
            FTy::Scalar(_) => format!("{value} == 0"),
            FTy::Named(e) => format!("encode{}({value}) == 0", e.buri),
        }
    }

    fn encode_fn(&mut self, name: &str, fields: &[RField], oneofs: &[ROneof]) {
        self.line("/// The protobuf wire encoding, fields in schema order.");
        self.line("///");
        self.line("/// A singular field holding its default is omitted, which is what proto3");
        self.line("/// calls the canonical encoding; an `optional` field that is set is written");
        self.line("/// whatever it holds, because that is the difference `optional` buys.");
        self.line(&format!("export fn encode{name}<C: Alloc>(ctx: C, value: {name}): [U8] {{"));
        if fields.is_empty() && oneofs.is_empty() {
            self.line("  let _ = value;");
            self.line("  let _ = ctx;");
            self.line("  list.empty::<U8>()");
            self.line("}");
            self.line("");
            return;
        }
        self.line("  [");
        for f in fields {
            let v = format!("value.{}", f.name);
            let piece = match f.label {
                Label::Repeated => {
                    if f.ty.packable() && f.packed {
                        let raw = self.write_raw(f, "x", "c");
                        format!(
                            "if ({v}.len() == 0) {{ list.empty::<U8>() }} else {{ proto.bytesField(ctx, {}, {v}.mapCtx(ctx, fn(c, x) => {raw}).flatten(ctx)) }}",
                            f.number
                        )
                    } else {
                        let one = self.write_one(f, "x", "c");
                        format!("{v}.mapCtx(ctx, fn(c, x) => {one}).flatten(ctx)")
                    }
                }
                _ if f.optional() => {
                    let one = self.write_one(f, "x", "ctx");
                    format!("match ({v}) {{ .Some(x) => {one}, .None => list.empty::<U8>() }}")
                }
                _ => {
                    let test = self.is_default(f, &v);
                    let one = self.write_one(f, &v, "ctx");
                    format!("if ({test}) {{ list.empty::<U8>() }} else {{ {one} }}")
                }
            };
            self.line(&format!("    {piece},"));
        }
        for o in oneofs {
            let arms: Vec<String> = o
                .cases
                .iter()
                .map(|c| format!(".{}(x) => {}", upper_first(&c.name), self.write_one(c, "x", "ctx")))
                .collect();
            self.line(&format!(
                "    match (value.{}) {{ .Some(k) => match (k) {{ {} }}, .None => list.empty::<U8>() }},",
                o.field,
                arms.join(", ")
            ));
        }
        self.line("  ].flatten(ctx)");
        self.line("}");
        self.line("");
    }

    // -- the binary decoder -------------------------------------------------

    /// What to read at `next`: a `Result` of the raw value and the index just
    /// past it.
    fn read_one(&self, f: &RField) -> String {
        match &f.ty {
            FTy::Scalar(s) => match s {
                Scalar::Str => "proto.readStr(ctx, b, next)?".into(),
                Scalar::Bytes => "proto.readBytes(ctx, b, next)?".into(),
                Scalar::Double => "proto.readF64(b, next)?".into(),
                Scalar::Float => "proto.readF32(b, next)?".into(),
                Scalar::Fixed32 | Scalar::Sfixed32 => "proto.readFixed32(b, next)?".into(),
                Scalar::Fixed64 | Scalar::Sfixed64 => "proto.readFixed64(b, next)?".into(),
                // A 32-bit field is the varint's low 32 bits, read as such
                // rather than truncated afterwards — see `bytes.readVarint32`.
                Scalar::Int32 | Scalar::Uint32 | Scalar::Sint32 => {
                    "proto.readVarint32(b, next)?".into()
                }
                _ => "proto.readVarint(b, next)?".into(),
            },
            FTy::Named(e) => match e.kind {
                // An enum value is an int32.
                Kind::Enum => "proto.readVarint32(b, next)?".into(),
                Kind::Message => "proto.readBytes(ctx, b, next)?".into(),
            },
        }
    }

    /// The raw reading turned into the field's own type.
    ///
    /// A 32-bit field truncates. The wire format has one integer encoding, so a
    /// `uint32` field can arrive carrying 2^33; protobuf reads the low 32 bits
    /// and every implementation agrees on that, so a decoder that kept the
    /// whole number would disagree with all of them.
    fn adapt(&self, f: &RField, raw: &str) -> String {
        match &f.ty {
            // `raw` is already the low 32 bits, unsigned; the signed types
            // reinterpret it and the zigzag one undoes its transform.
            FTy::Scalar(Scalar::Int32) => format!("proto.signed32({raw})"),
            FTy::Scalar(Scalar::Uint32) => raw.to_string(),
            FTy::Scalar(Scalar::Sint32) => format!("bytes.unzigzag({raw})"),
            FTy::Scalar(Scalar::Sint64) => format!("bytes.unzigzag({raw})"),
            FTy::Scalar(Scalar::Bool) => format!("{raw} != 0"),
            FTy::Scalar(Scalar::Sfixed32) => format!("proto.signed32({raw})"),
            FTy::Named(e) => match e.kind {
                Kind::Enum => format!("decode{}(proto.signed32({raw}))", e.buri),
                Kind::Message => format!("decode{}(ctx, {raw})?", e.buri),
            },
            _ => raw.to_string(),
        }
    }

    /// The same, for one element of a packed field, where the values arrive as
    /// a list rather than one at a time.
    fn adapt_packed(&self, f: &RField) -> String {
        match &f.ty {
            FTy::Scalar(Scalar::Bool) => "values.map(ctx, fn(v) => v != 0)".into(),
            FTy::Scalar(Scalar::Int32) => "values.map(ctx, fn(v) => proto.signed32(v))".into(),
            FTy::Scalar(Scalar::Uint32) => "values".into(),
            FTy::Scalar(Scalar::Sint32 | Scalar::Sint64) => {
                "values.map(ctx, fn(v) => bytes.unzigzag(v))".into()
            }
            FTy::Scalar(Scalar::Sfixed32) => "values.map(ctx, fn(v) => proto.signed32(v))".into(),
            FTy::Named(e) => {
                format!("values.map(ctx, fn(v) => decode{}(proto.signed32(v)))", e.buri)
            }
            _ => "values".into(),
        }
    }

    fn packed_reader(&self, f: &RField) -> String {
        match &f.ty {
            FTy::Scalar(Scalar::Int32 | Scalar::Uint32 | Scalar::Sint32) => {
                "proto.packedVarints32(ctx, raw)?".into()
            }
            FTy::Named(e) if e.kind == Kind::Enum => "proto.packedVarints32(ctx, raw)?".into(),
            FTy::Scalar(Scalar::Double) => "proto.packedF64(ctx, raw)?".into(),
            FTy::Scalar(Scalar::Float) => "proto.packedF32(ctx, raw)?".into(),
            FTy::Scalar(Scalar::Fixed32 | Scalar::Sfixed32) => {
                "proto.packedFixed32(ctx, raw)?".into()
            }
            FTy::Scalar(Scalar::Fixed64 | Scalar::Sfixed64) => {
                "proto.packedFixed64(ctx, raw)?".into()
            }
            _ => "proto.packedVarints(ctx, raw)?".into(),
        }
    }

    fn decode_fn(&mut self, name: &str, fields: &[RField], oneofs: &[ROneof]) {
        self.line("/// Reads one message. A field this schema does not know is skipped, which is");
        self.line("/// proto3's forward compatibility and the whole of it.");
        self.line(&format!(
            "export fn decode{name}<C: Alloc>(ctx: C, b: [U8]): Result<{name}, ProtoError> {{"
        ));
        self.line(&format!("  read{name}(ctx, b, 0, default{name}())"));
        self.line("}");
        self.line("");
        self.line("/// Reads one message *on top of* another.");
        self.line("///");
        self.line("/// A singular message field that arrives twice is merged rather than");
        self.line("/// replaced — the specification says so, and it is what makes a message");
        self.line("/// splittable across two encodings of itself. Every other kind of field");
        self.line("/// takes the later value, which merging also does.");
        self.line(&format!(
            "export fn merge{name}<C: Alloc>(\n  ctx: C,\n  b: [U8],\n  into: {name},\n): Result<{name}, ProtoError> {{"
        ));
        self.line(&format!("  read{name}(ctx, b, 0, into)"));
        self.line("}");
        self.line("");
        self.line(&format!("fn read{name}<C: Alloc>("));
        self.line("  ctx: C,");
        self.line("  b: [U8],");
        self.line("  at: Int,");
        self.line(&format!("  acc: {name},"));
        self.line(&format!("): Result<{name}, ProtoError> {{"));
        self.line("  if (at >= b.len()) {");
        self.line("    .Ok(acc)");
        self.line("  } else {");
        self.line("    let (field, wire, next) = proto.readTag(b, at)?;");

        // Every case a header can name: the message's own fields, then each
        // oneof's, which are ordinary fields with a different place to land.
        let mut all: Vec<(RField, Option<&ROneof>)> = Vec::new();
        for f in fields {
            all.push((f.clone(), None));
        }
        for o in oneofs {
            for c in &o.cases {
                all.push((c.clone(), Some(o)));
            }
        }

        let mut arms: Vec<(String, Vec<String>)> = Vec::new();
        for (f, in_oneof) in &all {
            // Packed and unpacked both, for a repeated field of a packable
            // type: a writer may send either, so a reader takes either.
            if f.label == Label::Repeated && f.ty.packable() {
                let reader = self.packed_reader(f);
                let mapped = self.adapt_packed(f);
                arms.push((
                    format!("field == {} && wire == proto.LEN", f.number),
                    vec![
                        "let (raw, after) = proto.readBytes(ctx, b, next)?;".into(),
                        format!("let values = {reader};"),
                        format!(
                            "read{name}(ctx, b, after, {name} {{ ..acc, {}: acc.{}.concat(ctx, {mapped}) }})",
                            f.name, f.name
                        ),
                    ],
                ));
            }

            let read = self.read_one(f);
            // A singular message field merges into whatever is already there.
            // The seed is the accumulator's own value, so two encodings of one
            // message read as the message they describe together.
            let adapted = match (&f.ty, f.label, in_oneof) {
                (FTy::Named(e), label, seat) if e.kind == Kind::Message && label != Label::Repeated => {
                    let seed = match seat {
                        Some(o) => format!(
                            "match (acc.{}) {{ .Some(.{}(prev)) => prev, _ => default{}() }}",
                            o.field,
                            upper_first(&f.name),
                            e.buri
                        ),
                        None => format!(
                            "match (acc.{}) {{ .Some(prev) => prev, .None => default{}() }}",
                            f.name, e.buri
                        ),
                    };
                    format!("merge{}(ctx, raw, {seed})?", e.buri)
                }
                _ => self.adapt(f, "raw"),
            };
            let assign = match in_oneof {
                Some(o) => format!(
                    "{}: .Some({}.{}(value))",
                    o.field,
                    o.enum_name,
                    upper_first(&f.name)
                ),
                None if f.label == Label::Repeated => {
                    format!("{}: acc.{}.push(ctx, value)", f.name, f.name)
                }
                None if f.optional() => format!("{}: .Some(value)", f.name),
                None => format!("{}: value", f.name),
            };
            arms.push((
                format!("field == {} && wire == {}", f.number, f.ty.wire()),
                vec![
                    format!("let (raw, after) = {read};"),
                    format!("let value = {adapted};"),
                    format!("read{name}(ctx, b, after, {name} {{ ..acc, {assign} }})"),
                ],
            ));
        }

        let chain = if_chain(
            "    ",
            &arms,
            &[
                "let after = proto.skip(b, next, wire)?;".into(),
                format!("read{name}(ctx, b, after, acc)"),
            ],
        );
        self.lines(&chain);
        self.line("  }");
        self.line("}");
        self.line("");
    }

    // -- JSON ---------------------------------------------------------------

    fn json_of(&self, f: &RField, value: &str, ctx: &str) -> String {
        match &f.ty {
            FTy::Scalar(s) => match s {
                Scalar::Int64 | Scalar::Uint64 | Scalar::Sint64 | Scalar::Fixed64
                | Scalar::Sfixed64 => format!("proto.jsonInt64({ctx}, {value})"),
                Scalar::Bool => format!("Json.Bool({value})"),
                Scalar::Str => format!("Json.Str({value})"),
                Scalar::Bytes => format!("proto.jsonBytes({ctx}, {value})"),
                Scalar::Double | Scalar::Float => format!("proto.jsonFloat({value})"),
                _ => format!("proto.jsonInt32({value})"),
            },
            FTy::Named(e) => match e.kind {
                Kind::Enum => format!("encode{}Json({value})", e.buri),
                Kind::Message => format!("encode{}Json({ctx}, {value})", e.buri),
            },
        }
    }

    fn json_encode_fn(&mut self, name: &str, fields: &[RField], oneofs: &[ROneof]) {
        self.line("/// The proto3 JSON mapping — which is *not* `derive ToJson`'s. A 64-bit");
        self.line("/// integer is a string, `bytes` is base64, an enum is its value's name, and");
        self.line("/// a oneof's case is an ordinary member of this object.");
        self.line(&format!(
            "export fn encode{name}Json<C: Alloc>(ctx: C, value: {name}): Json {{"
        ));
        if fields.is_empty() && oneofs.is_empty() {
            self.line("  let _ = value;");
            self.line("  let _ = ctx;");
            self.line("  Json.Object(list.empty::<(Str, Json)>())");
            self.line("}");
            self.line("");
            return;
        }
        self.line("  Json.Object([");
        for f in fields {
            let v = format!("value.{}", f.name);
            let piece = match f.label {
                Label::Repeated => {
                    let one = self.json_of(f, "x", "c");
                    format!(
                        "if ({v}.len() == 0) {{ list.empty::<(Str, Json)>() }} else {{ [(\"{}\", Json.Array({v}.mapCtx(ctx, fn(c, x) => {one})))] }}",
                        f.json
                    )
                }
                _ if f.optional() => {
                    let one = self.json_of(f, "x", "ctx");
                    format!(
                        "match ({v}) {{ .Some(x) => [(\"{}\", {one})], .None => list.empty::<(Str, Json)>() }}",
                        f.json
                    )
                }
                _ => {
                    let test = self.is_default(f, &v);
                    let one = self.json_of(f, &v, "ctx");
                    format!(
                        "if ({test}) {{ list.empty::<(Str, Json)>() }} else {{ [(\"{}\", {one})] }}",
                        f.json
                    )
                }
            };
            self.line(&format!("    {piece},"));
        }
        for o in oneofs {
            let arms: Vec<String> = o
                .cases
                .iter()
                .map(|c| {
                    format!(
                        ".{}(x) => [(\"{}\", {})]",
                        upper_first(&c.name),
                        c.json,
                        self.json_of(c, "x", "ctx")
                    )
                })
                .collect();
            self.line(&format!(
                "    match (value.{}) {{ .Some(k) => match (k) {{ {} }}, .None => list.empty::<(Str, Json)>() }},",
                o.field,
                arms.join(", ")
            ));
        }
        self.line("  ].flatten(ctx))");
        self.line("}");
        self.line("");
    }

    /// Looking a field up in an object.
    ///
    /// proto3 JSON *writes* the camelCase name and *accepts* either it or the
    /// name the schema wrote. A field whose schema name is already camel needs
    /// only the one lookup, and gets it.
    fn member_call(&self, f: &RField) -> String {
        if f.json == f.proto {
            format!("proto.member(doc, \"{}\")", f.json)
        } else {
            format!("proto.memberEither(doc, \"{}\", \"{}\")", f.json, f.proto)
        }
    }

    /// Reading one value out of a document, as a `Result` expression.
    ///
    /// `ctx` is named rather than assumed: inside a fold over an array the
    /// context is the lambda's parameter, and a lambda may not capture the
    /// enclosing one (SPEC 10.6).
    fn json_read(&self, f: &RField, doc: &str, path: &str, ctx: &str) -> String {
        match &f.ty {
            FTy::Scalar(s) => match s {
                Scalar::Bool => format!("proto.asBool({doc}, {path})"),
                Scalar::Str => format!("proto.asStr({doc}, {path})"),
                Scalar::Bytes => format!("proto.asBytes({ctx}, {doc}, {path})"),
                Scalar::Double => format!("proto.asFloat({doc}, {path})"),
                Scalar::Float => format!("proto.asFloat32({doc}, {path})"),
                // proto3 JSON rejects a value the field cannot hold rather
                // than truncating it, which is what the binary format does —
                // so the field's own range has to travel to the reader.
                _ => {
                    let (lo, hi) = json_range(*s);
                    format!("proto.asInt({doc}, {path}, {lo}, {hi})")
                }
            },
            FTy::Named(e) => match e.kind {
                Kind::Enum => format!("decode{}Json({doc}, {path})", e.buri),
                Kind::Message => format!("decode{}JsonAt({ctx}, {doc}, {path})", e.buri),
            },
        }
    }

    /// A oneof's cases, read in order: the first member present wins, which is
    /// what a document holding two of them means to every other implementation.
    fn oneof_json_fn(&mut self, name: &str, o: &ROneof) {
        self.line(&format!(
            "fn read{}<C: Alloc>(\n  ctx: C,\n  doc: Json,\n  path: Str,\n): Result<Option<{}>, ProtoError> {{",
            o.enum_name, o.enum_name
        ));
        let mut body: Vec<String> = vec![".Ok(.None)".to_string()];
        for c in o.cases.iter().rev() {
            let read = self.json_read(c, "v", &format!("proto.at(ctx, path, \"{}\")", c.json), "ctx");
            let mut next = vec![format!("match ({}) {{", self.member_call(c))];
            next.push(format!(
                "  .Some(v) => .Ok(.Some({}.{}({read}?))),",
                o.enum_name,
                upper_first(&c.name)
            ));
            next.push("  .None => ".to_string());
            for (i, l) in body.iter().enumerate() {
                next.push(format!("    {l}{}", if i + 1 == body.len() { "," } else { "" }));
            }
            next.push("}".to_string());
            body = next;
        }
        for l in &body {
            let l = l.clone();
            self.line(&format!("  {l}"));
        }
        self.line("}");
        self.line("");
        let _ = name;
    }

    fn json_decode_fn(&mut self, name: &str, fields: &[RField], oneofs: &[ROneof]) {
        for o in oneofs {
            self.oneof_json_fn(name, o);
        }
        self.line("/// Reads a proto3 JSON document. An absent member and a `null` one mean the");
        self.line("/// same thing, which is what the mapping says.");
        self.line(&format!(
            "export fn decode{name}Json<C: Alloc>(ctx: C, doc: Json): Result<{name}, ProtoError> {{"
        ));
        self.line(&format!("  decode{name}JsonAt(ctx, doc, \"$\")"));
        self.line("}");
        self.line("");
        self.line("/// The same, from somewhere inside a larger document, so that an error names");
        self.line("/// a place a reader can find.");
        self.line(&format!("export fn decode{name}JsonAt<C: Alloc>("));
        self.line("  ctx: C,");
        self.line("  doc: Json,");
        self.line("  path: Str,");
        self.line(&format!("): Result<{name}, ProtoError> {{"));
        self.line("  match (doc) {");
        self.line("    .Object(_entries) => {");
        for f in fields {
            let member = self.member_call(f);
            let field_path = format!("proto.at(ctx, path, \"{}\")", f.json);
            match f.label {
                Label::Repeated => {
                    let element = self.json_read(f, "item", &format!("p_{}", f.name), "c");
                    let base = f.ty.buri();
                    self.line(&format!("      let p_{}: Str = {field_path};", f.name));
                    // Every binding is annotated: a `.None` arm has no type to
                    // infer from on its own, and the annotation is what a
                    // reader of the generated module wants anyway.
                    self.line(&format!(
                        "      let {}: {} = match ({member}) {{",
                        f.name,
                        f.buri_type()
                    ));
                    self.line(&format!("        .None => list.empty::<{base}>(),"));
                    self.line(&format!(
                        "        .Some(v) => proto.asArray(v, p_{})?.foldResultCtx(",
                        f.name
                    ));
                    self.line("          ctx,");
                    self.line(&format!("          fn(c, acc: [{base}], item) => {{"));
                    self.line(&format!("            let one = {element}?;"));
                    self.line("            .Ok(acc.push(c, one))");
                    self.line("          },");
                    self.line(&format!("          list.empty::<{base}>(),"));
                    self.line("        )?,");
                    self.line("      };");
                }
                _ if f.optional() => {
                    let read = self.json_read(f, "v", &field_path, "ctx");
                    self.line(&format!(
                        "      let {}: {} = match ({member}) {{",
                        f.name,
                        f.buri_type()
                    ));
                    self.line("        .None => .None,");
                    self.line(&format!("        .Some(v) => .Some({read}?),"));
                    self.line("      };");
                }
                _ => {
                    let read = self.json_read(f, "v", &field_path, "ctx");
                    let d = self.default_of(f);
                    self.line(&format!(
                        "      let {}: {} = match ({member}) {{",
                        f.name,
                        f.buri_type()
                    ));
                    self.line(&format!("        .None => {d},"));
                    self.line(&format!("        .Some(v) => {read}?,"));
                    self.line("      };");
                }
            }
        }
        for o in oneofs {
            self.line(&format!(
                "      let {} = read{}(ctx, doc, path)?;",
                o.field, o.enum_name
            ));
        }
        if fields.is_empty() && oneofs.is_empty() {
            self.line(&format!("      .Ok({name} {{}})"));
        } else {
            self.line(&format!("      .Ok({name} {{"));
            for f in fields {
                self.line(&format!("        {}: {},", f.name, f.name));
            }
            for o in oneofs {
                self.line(&format!("        {}: {},", o.field, o.field));
            }
            self.line("      })");
        }
        self.line("    },");
        self.line("    _ => .Err(.BadJson { path: path, wanted: \"an object\" }),");
        self.line("  }");
        self.line("}");
        self.line("");
    }
}

/// The range a JSON reader holds an integer field to.
///
/// The 64-bit bounds are `Int`'s own, which is `I64`'s: a `uint64` above 2^63
/// has no `Int` to be, and the reader says so rather than wrapping.
fn json_range(s: Scalar) -> (&'static str, &'static str) {
    match s {
        Scalar::Int32 | Scalar::Sint32 | Scalar::Sfixed32 => ("-2147483648", "2147483647"),
        Scalar::Uint32 | Scalar::Fixed32 => ("0", "4294967295"),
        Scalar::Uint64 | Scalar::Fixed64 => ("0", "9223372036854775807"),
        _ => ("-9223372036854775808", "9223372036854775807"),
    }
}

/// The name a `.proto` module path implies for a schema import: an import is
/// written from the repository root, exactly as protoc's `-I.` would resolve
/// it, so `import "proto/address.proto";` is the module `//proto/address.proto`.
pub fn import_module_path(import: &str) -> String {
    format!("//{}", import.trim_start_matches("./").trim_start_matches('/'))
}

/// True when a module path names a generated `.proto` module.
pub fn is_proto_path(path: &str) -> bool {
    path.ends_with(".proto")
}

/// A `.proto` import that names nothing.
pub fn unresolved_import(span: Span, path: &str) -> Diagnostic {
    Diagnostic::error(span, format!("\"{path}\" names no schema in this repository"))
        .with_code("proto-import-not-found")
        .with_note(
            "an import inside a schema is written from the repository root, the way protoc \
             resolves one against `-I.`",
        )
        .with_fix("write the path from the repository root, as in `import \"proto/address.proto\";`")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::protoschema;
    use crate::diagnostics::FileId;

    fn gen(src: &str) -> String {
        let parsed = protoschema::parse(src, FileId(0));
        assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
        let mut diags = Vec::new();
        let out = generate("x.proto", &parsed.schema, &[], &mut diags);
        assert!(diags.is_empty(), "{diags:#?}");
        out.source
    }

    #[test]
    fn camel_case_follows_protoc() {
        assert_eq!(camel_case("user_name"), "userName");
        assert_eq!(camel_case("id"), "id");
        assert_eq!(camel_case("a_b_c"), "aBC");
        // protoc changes no case but the letter after an underscore, so a
        // schema's `URL` keeps its shape and the JSON name matches protoc's.
        assert_eq!(camel_case("URL"), "URL");
    }

    #[test]
    fn a_keyword_field_name_is_escaped_and_its_json_name_is_not() {
        let s = gen("syntax = \"proto3\";\nmessage M { string type = 1; }\n");
        assert!(s.contains("export type_: Str,"), "{s}");
        assert!(s.contains("\"type\""), "the JSON name is the schema's: {s}");
    }

    #[test]
    fn an_import_resolves_from_the_repository_root() {
        assert_eq!(import_module_path("proto/address.proto"), "//proto/address.proto");
        assert_eq!(import_module_path("./a.proto"), "//a.proto");
    }

    /// The three labels, and the one field whose Buri type does not follow its
    /// label: a singular message.
    #[test]
    fn the_label_mapping() {
        let s = gen(
            "syntax = \"proto3\";\nmessage Inner { int32 a = 1; }\nmessage M {\n  int32 plain = 1;\n  \
             optional int32 maybe = 2;\n  repeated int32 many = 3;\n  Inner one = 4;\n}\n",
        );
        assert!(s.contains("export plain: Int,"), "{s}");
        assert!(s.contains("export maybe: Option<Int>,"), "{s}");
        assert!(s.contains("export many: [Int],"), "{s}");
        assert!(s.contains("export one: Option<Inner>,"), "{s}");
    }

    #[test]
    fn nesting_flattens_with_an_underscore() {
        let s = gen(
            "syntax = \"proto3\";\nmessage Outer {\n  message Inner { int32 a = 1; }\n  Inner in = 1;\n}\n",
        );
        assert!(s.contains("export struct Outer_Inner {"), "{s}");
        assert!(s.contains("export in_: Option<Outer_Inner>,"), "{s}");
    }

    #[test]
    fn a_oneof_becomes_an_enum_held_as_an_option() {
        let s = gen(
            "syntax = \"proto3\";\nmessage M { oneof pick { string a = 1; int32 b = 2; } }\n",
        );
        assert!(s.contains("export enum M_Pick {"), "{s}");
        assert!(s.contains("export A(Str),"), "{s}");
        assert!(s.contains("export pick: Option<M_Pick>,"), "{s}");
    }
}
