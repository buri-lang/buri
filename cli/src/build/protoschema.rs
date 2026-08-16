//! A reader for `.proto` *schemas*.
//!
//! `build/textproto.rs` reads proto *values* — the dialect `BUILD.buri` is
//! written in. This reads the other half of the format: the declarations, so
//! that `from "//proto/person.proto" import { Person };` can become a module.
//! The two share nothing but a family resemblance, which is why they are two
//! files.
//!
//! proto3 only, and a deliberately small proto3. What is here is what a message
//! is made of:
//!
//! ```text
//! syntax = "proto3";
//! package example.v1;
//! import "proto/address.proto";
//!
//! enum Colour { COLOUR_UNSPECIFIED = 0; RED = 1; }
//!
//! message Person {
//!   string name = 1;
//!   optional int32 age = 2;
//!   repeated string emails = 3;
//!   Address home = 4;
//!   oneof contact { string phone = 5; string email = 6; }
//!   message Nickname { string text = 1; }
//! }
//! ```
//!
//! What is *not* here is refused by name rather than ignored: `service`,
//! `extend`, `extensions`, groups, `map<>`, `google.protobuf.Any`, and proto2.
//! Every one of them would change what a message means on the wire, and a
//! reader that skipped one would decode the file in front of it as a different
//! file. `option` and `reserved` are the two exceptions, and they are skipped
//! rather than refused: neither says anything about the shape of a message, and
//! `option` in particular is how a schema talks to code generators that are not
//! this one.

use crate::diagnostics::{Diagnostic, FileId, Span};

// ---------------------------------------------------------------------------
// The tree
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scalar {
    Int32,
    Int64,
    Uint32,
    Uint64,
    Sint32,
    Sint64,
    Fixed32,
    Fixed64,
    Sfixed32,
    Sfixed64,
    Bool,
    Str,
    Bytes,
    Double,
    Float,
}

impl Scalar {
    pub fn parse(word: &str) -> Option<Scalar> {
        Some(match word {
            "int32" => Scalar::Int32,
            "int64" => Scalar::Int64,
            "uint32" => Scalar::Uint32,
            "uint64" => Scalar::Uint64,
            "sint32" => Scalar::Sint32,
            "sint64" => Scalar::Sint64,
            "fixed32" => Scalar::Fixed32,
            "fixed64" => Scalar::Fixed64,
            "sfixed32" => Scalar::Sfixed32,
            "sfixed64" => Scalar::Sfixed64,
            "bool" => Scalar::Bool,
            "string" => Scalar::Str,
            "bytes" => Scalar::Bytes,
            "double" => Scalar::Double,
            "float" => Scalar::Float,
            _ => return None,
        })
    }

    pub fn proto_name(self) -> &'static str {
        match self {
            Scalar::Int32 => "int32",
            Scalar::Int64 => "int64",
            Scalar::Uint32 => "uint32",
            Scalar::Uint64 => "uint64",
            Scalar::Sint32 => "sint32",
            Scalar::Sint64 => "sint64",
            Scalar::Fixed32 => "fixed32",
            Scalar::Fixed64 => "fixed64",
            Scalar::Sfixed32 => "sfixed32",
            Scalar::Sfixed64 => "sfixed64",
            Scalar::Bool => "bool",
            Scalar::Str => "string",
            Scalar::Bytes => "bytes",
            Scalar::Double => "double",
            Scalar::Float => "float",
        }
    }
}

#[derive(Clone, Debug)]
pub enum TypeRef {
    Scalar(Scalar),
    /// A message or enum, by the name the schema wrote. Resolved against the
    /// file's own declarations and its imports' when the module is generated.
    Named(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Label {
    /// proto3 implicit presence: always there, defaulted when absent.
    Single,
    /// `optional`: presence is tracked.
    Optional,
    Repeated,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub number: i64,
    pub label: Label,
    pub ty: TypeRef,
    pub span: Span,
    /// The oneof this field belongs to, by name, or `None`.
    pub oneof: Option<String>,
    /// `[packed = ...]`, when the field says. proto3 packs a repeated numeric
    /// field by default, and this is the only field option that changes what
    /// goes on the wire — so it is the only one this reader records.
    pub packed: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct Oneof {
    pub name: String,
    pub span: Span,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug)]
pub struct EnumValue {
    pub name: String,
    pub number: i64,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct EnumDef {
    pub name: String,
    pub span: Span,
    pub values: Vec<EnumValue>,
}

#[derive(Clone, Debug)]
pub struct Message {
    pub name: String,
    pub span: Span,
    /// Fields not inside a `oneof`, in declaration order.
    pub fields: Vec<Field>,
    pub oneofs: Vec<Oneof>,
    pub messages: Vec<Message>,
    pub enums: Vec<EnumDef>,
}

#[derive(Clone, Debug)]
pub struct Import {
    pub path: String,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct Schema {
    pub package: Option<String>,
    pub imports: Vec<Import>,
    pub messages: Vec<Message>,
    pub enums: Vec<EnumDef>,
}

pub struct Parsed {
    pub schema: Schema,
    pub errors: Vec<Diagnostic>,
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

pub fn parse(text: &str, file: FileId) -> Parsed {
    let mut p = Parser { src: text.as_bytes(), text, pos: 0, file, errors: Vec::new(), depth: 0 };
    let schema = p.file();
    Parsed { schema, errors: p.errors }
}

struct Parser<'a> {
    src: &'a [u8],
    text: &'a str,
    pos: usize,
    file: FileId,
    errors: Vec<Diagnostic>,
    depth: u32,
}

/// One token. A `.proto` schema needs no more vocabulary than this.
#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Word(String),
    Num(i64),
    Str(String),
    Punct(char),
    End,
}

impl Tok {
    fn describe(&self) -> String {
        match self {
            Tok::Word(w) => format!("`{w}`"),
            Tok::Num(n) => format!("`{n}`"),
            Tok::Str(s) => format!("\"{s}\""),
            Tok::Punct(c) => format!("`{c}`"),
            Tok::End => "the end of the file".to_string(),
        }
    }
}

impl<'a> Parser<'a> {
    fn err(&mut self, span: Span, msg: impl Into<String>, fix: impl Into<String>) {
        if self.errors.len() < 32 {
            self.errors.push(
                Diagnostic::error(span, msg).with_code("proto-schema").with_fix(fix),
            );
        }
    }

    /// A construct this reader refuses rather than ignores. Named, always,
    /// because "unsupported" without a noun is a message you cannot act on.
    fn unsupported(&mut self, span: Span, what: &str, why: &str, fix: impl Into<String>) {
        if self.errors.len() < 32 {
            self.errors.push(
                Diagnostic::error(span, format!("{what} is not supported"))
                    .with_code("proto-unsupported")
                    .with_note(why)
                    .with_fix(fix),
            );
        }
    }

    fn peek_byte(&self) -> u8 {
        *self.src.get(self.pos).unwrap_or(&0)
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek_byte() {
                b' ' | b'\t' | b'\r' | b'\n' => self.pos += 1,
                b'/' if self.src.get(self.pos + 1) == Some(&b'/') => {
                    while self.pos < self.src.len() && self.peek_byte() != b'\n' {
                        self.pos += 1;
                    }
                }
                b'/' if self.src.get(self.pos + 1) == Some(&b'*') => {
                    let start = self.pos;
                    self.pos += 2;
                    loop {
                        if self.pos + 1 >= self.src.len() {
                            self.pos = self.src.len();
                            let span = Span::new(self.file, start, self.pos);
                            self.err(span, "unterminated block comment", "close it with `*/`");
                            return;
                        }
                        if self.peek_byte() == b'*' && self.src[self.pos + 1] == b'/' {
                            self.pos += 2;
                            break;
                        }
                        self.pos += 1;
                    }
                }
                _ => return,
            }
        }
    }

    /// The next token, consuming it. The span of what was read is
    /// `start..self.pos` for whatever `start` the caller recorded first.
    fn next(&mut self) -> Tok {
        self.skip_trivia();
        if self.pos >= self.src.len() {
            return Tok::End;
        }
        let c = self.peek_byte();
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = self.pos;
            while self.peek_byte().is_ascii_alphanumeric()
                || self.peek_byte() == b'_'
                || self.peek_byte() == b'.'
            {
                self.pos += 1;
            }
            return Tok::Word(self.text[start..self.pos].to_string());
        }
        if c.is_ascii_digit() || (c == b'-' && self.src.get(self.pos + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = self.pos;
            if c == b'-' {
                self.pos += 1;
            }
            while self.peek_byte().is_ascii_alphanumeric() {
                self.pos += 1;
            }
            let raw = &self.text[start..self.pos];
            let parsed = if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
                i64::from_str_radix(hex, 16).ok()
            } else {
                raw.parse::<i64>().ok()
            };
            return match parsed {
                Some(n) => Tok::Num(n),
                None => {
                    let span = Span::new(self.file, start, self.pos);
                    let raw = raw.to_string();
                    self.err(span, format!("`{raw}` is not a number"), "write a decimal or `0x` field number");
                    Tok::Num(0)
                }
            };
        }
        if c == b'"' || c == b'\'' {
            let quote = c;
            let start = self.pos;
            self.pos += 1;
            let mut s = String::new();
            loop {
                if self.pos >= self.src.len() || self.peek_byte() == b'\n' {
                    let span = Span::new(self.file, start, self.pos);
                    self.err(span, "unterminated string", "close it with a quote");
                    return Tok::Str(s);
                }
                let ch = self.peek_byte();
                if ch == quote {
                    self.pos += 1;
                    return Tok::Str(s);
                }
                if ch == b'\\' {
                    self.pos += 1;
                    let e = self.peek_byte();
                    self.pos += 1;
                    s.push(match e {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        other => other as char,
                    });
                    continue;
                }
                let ch = self.text[self.pos..].chars().next().unwrap();
                self.pos += ch.len_utf8();
                s.push(ch);
            }
        }
        self.pos += 1;
        Tok::Punct(c as char)
    }

    fn peek(&mut self) -> Tok {
        let save = self.pos;
        let t = self.next();
        self.pos = save;
        t
    }

    /// Consumes a specific punctuation token, reporting when it is not there.
    fn expect(&mut self, c: char, context: &str) -> bool {
        let start = {
            self.skip_trivia();
            self.pos
        };
        let t = self.next();
        if t == Tok::Punct(c) {
            return true;
        }
        let span = Span::new(self.file, start, self.pos.max(start + 1));
        let found = t.describe();
        self.err(
            span,
            format!("expected `{c}` after {context}, found {found}"),
            format!("write `{c}` here"),
        );
        false
    }

    fn word(&mut self, what: &str) -> Option<(String, Span)> {
        self.skip_trivia();
        let start = self.pos;
        match self.next() {
            Tok::Word(w) => Some((w, Span::new(self.file, start, self.pos))),
            other => {
                let span = Span::new(self.file, start, self.pos.max(start + 1));
                let found = other.describe();
                self.err(
                    span,
                    format!("expected {what}, found {found}"),
                    format!("write {what} here"),
                );
                None
            }
        }
    }

    /// Everything up to and including the next `;`, for the statements this
    /// reader records nothing from.
    fn skip_statement(&mut self) {
        loop {
            match self.next() {
                Tok::Punct(';') | Tok::End => return,
                // An option value may be a block: `option (x) = { a: 1 };`.
                Tok::Punct('{') => self.skip_braces(),
                _ => {}
            }
        }
    }

    fn skip_braces(&mut self) {
        let mut depth = 1;
        loop {
            match self.next() {
                Tok::Punct('{') => depth += 1,
                Tok::Punct('}') => {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                Tok::End => return,
                _ => {}
            }
        }
    }

    // -- the file ----------------------------------------------------------

    fn file(&mut self) -> Schema {
        let mut schema = Schema::default();
        let mut seen_syntax = false;
        loop {
            self.skip_trivia();
            let start = self.pos;
            let t = self.next();
            match t {
                Tok::End => break,
                Tok::Punct(';') => {}
                Tok::Word(w) => match w.as_str() {
                    "syntax" => {
                        seen_syntax = true;
                        self.expect('=', "`syntax`");
                        self.skip_trivia();
                        let at = self.pos;
                        let value = self.next();
                        if value != Tok::Str("proto3".to_string()) {
                            let span = Span::new(self.file, at, self.pos);
                            let found = value.describe();
                            self.unsupported(
                                span,
                                &format!("syntax {found}"),
                                "the reader implements proto3, whose field presence and default \
                                 rules are what the Buri mapping is written against",
                                "write `syntax = \"proto3\";`",
                            );
                        }
                        self.expect(';', "the syntax declaration");
                    }
                    "package" => {
                        if let Some((name, _)) = self.word("a package name") {
                            schema.package = Some(name);
                        }
                        self.expect(';', "the package declaration");
                    }
                    "import" => {
                        self.skip_trivia();
                        let at = self.pos;
                        match self.next() {
                            Tok::Str(path) => schema
                                .imports
                                .push(Import { path, span: Span::new(self.file, at, self.pos) }),
                            Tok::Word(w) if w == "public" || w == "weak" => {
                                let span = Span::new(self.file, at, self.pos);
                                self.unsupported(
                                    span,
                                    &format!("`import {w}`"),
                                    "a public import re-exports another file's declarations, which \
                                     would make one module's surface depend on a second file's",
                                    "write a plain `import \"...\";`, and import the second file \
                                     where its types are used",
                                );
                                self.skip_statement();
                                continue;
                            }
                            other => {
                                let span = Span::new(self.file, at, self.pos.max(at + 1));
                                let found = other.describe();
                                self.err(
                                    span,
                                    format!("expected a quoted path after `import`, found {found}"),
                                    "write `import \"path/from/the/repository/root.proto\";`",
                                );
                                self.skip_statement();
                                continue;
                            }
                        }
                        self.expect(';', "the import");
                    }
                    "option" | "reserved" => self.skip_statement(),
                    "message" => {
                        if let Some(m) = self.message() {
                            schema.messages.push(m);
                        }
                    }
                    "enum" => {
                        if let Some(e) = self.enum_def() {
                            schema.enums.push(e);
                        }
                    }
                    "service" | "extend" | "extensions" => {
                        let span = Span::new(self.file, start, self.pos);
                        let (why, fix): (&str, &str) = match w.as_str() {
                            "service" => (
                                "this reader turns a schema into data types; it has no RPC \
                                 transport to generate a stub against",
                                "delete the `service` block, or keep it in a schema this \
                                 repository does not import",
                            ),
                            _ => (
                                "an extension adds fields to a message from outside it, so the \
                                 generated type would not be the whole of the message",
                                "put the fields in the message itself",
                            ),
                        };
                        self.unsupported(span, &format!("`{w}`"), why, fix);
                        // Both forms carry a block; step over it so the rest of
                        // the file is still read.
                        self.recover_block();
                    }
                    other => {
                        let span = Span::new(self.file, start, self.pos);
                        let other = other.to_string();
                        self.err(
                            span,
                            format!("`{other}` does not begin a declaration"),
                            "a schema holds `syntax`, `package`, `import`, `option`, `message` \
                             and `enum` declarations",
                        );
                        self.skip_statement();
                    }
                },
                other => {
                    let span = Span::new(self.file, start, self.pos.max(start + 1));
                    let found = other.describe();
                    self.err(
                        span,
                        format!("expected a declaration, found {found}"),
                        "a schema holds `syntax`, `package`, `import`, `option`, `message` and \
                         `enum` declarations",
                    );
                }
            }
        }
        if !seen_syntax {
            self.errors.push(
                Diagnostic::error(Span::point(self.file, 0), "the file does not declare its syntax")
                    .with_code("proto-schema")
                    .with_note(
                        "without the line, protoc reads a file as proto2, whose presence rules \
                         are not the ones this mapping is written against",
                    )
                    .with_fix("add `syntax = \"proto3\";` as the first line"),
            );
        }
        schema
    }

    /// After an unsupported block-shaped declaration: step over the block if
    /// there is one, so one refusal does not become twenty.
    fn recover_block(&mut self) {
        loop {
            match self.peek() {
                Tok::Punct('{') => {
                    self.next();
                    self.skip_braces();
                    return;
                }
                Tok::Punct(';') | Tok::End => {
                    self.next();
                    return;
                }
                _ => {
                    self.next();
                }
            }
        }
    }

    // -- message -----------------------------------------------------------

    fn message(&mut self) -> Option<Message> {
        let (name, span) = self.word("a message name")?;
        if !self.expect('{', "a message name") {
            return None;
        }
        self.depth += 1;
        if self.depth > 16 {
            self.err(span, "messages nest too deeply", "flatten them; sixteen levels is the limit");
            self.depth -= 1;
            self.skip_braces();
            return None;
        }
        let mut m = Message {
            name,
            span,
            fields: Vec::new(),
            oneofs: Vec::new(),
            messages: Vec::new(),
            enums: Vec::new(),
        };
        loop {
            self.skip_trivia();
            let start = self.pos;
            match self.peek() {
                Tok::Punct('}') => {
                    self.next();
                    break;
                }
                Tok::End => {
                    self.err(
                        Span::new(self.file, start, start + 1),
                        format!("`{}` is not closed", m.name),
                        "close the message with `}`",
                    );
                    break;
                }
                Tok::Punct(';') => {
                    self.next();
                }
                Tok::Word(w) => match w.as_str() {
                    "message" => {
                        self.next();
                        if let Some(inner) = self.message() {
                            m.messages.push(inner);
                        }
                    }
                    "enum" => {
                        self.next();
                        if let Some(e) = self.enum_def() {
                            m.enums.push(e);
                        }
                    }
                    "oneof" => {
                        self.next();
                        if let Some(o) = self.oneof() {
                            m.oneofs.push(o);
                        }
                    }
                    "option" | "reserved" => {
                        self.next();
                        self.skip_statement();
                    }
                    "extensions" | "extend" => {
                        self.next();
                        let span = Span::new(self.file, start, self.pos);
                        self.unsupported(
                            span,
                            &format!("`{w}`"),
                            "an extension adds fields to a message from outside it, so the \
                             generated type would not be the whole of the message",
                            "put the fields in the message itself",
                        );
                        self.recover_block();
                    }
                    "group" => {
                        self.next();
                        let span = Span::new(self.file, start, self.pos);
                        self.unsupported(
                            span,
                            "`group`",
                            "groups are proto2's inline nesting, and their wire encoding was \
                             removed from proto3",
                            "declare a nested `message` and a field of that type",
                        );
                        self.recover_block();
                    }
                    "required" => {
                        self.next();
                        let span = Span::new(self.file, start, self.pos);
                        self.unsupported(
                            span,
                            "`required`",
                            "proto3 has no required fields; a field that must be there is a \
                             promise the format cannot keep across versions",
                            "drop `required`, or write `optional` if presence has to be visible",
                        );
                        self.skip_statement();
                    }
                    _ => match self.field(None) {
                        Some(f) => m.fields.push(f),
                        None => self.skip_statement(),
                    },
                },
                _ => match self.field(None) {
                    Some(f) => m.fields.push(f),
                    None => self.skip_statement(),
                },
            }
        }
        self.depth -= 1;
        Some(m)
    }

    fn oneof(&mut self) -> Option<Oneof> {
        let (name, span) = self.word("a oneof name")?;
        if !self.expect('{', "a oneof name") {
            return None;
        }
        let mut o = Oneof { name: name.clone(), span, fields: Vec::new() };
        loop {
            self.skip_trivia();
            let start = self.pos;
            match self.peek() {
                Tok::Punct('}') => {
                    self.next();
                    break;
                }
                Tok::End => {
                    self.err(
                        Span::new(self.file, start, start + 1),
                        format!("`oneof {}` is not closed", o.name),
                        "close it with `}`",
                    );
                    break;
                }
                Tok::Punct(';') => {
                    self.next();
                }
                Tok::Word(w) if w == "option" => {
                    self.next();
                    self.skip_statement();
                }
                _ => match self.field(Some(name.clone())) {
                    Some(f) => {
                        if f.label != Label::Single {
                            self.err(
                                f.span,
                                "a oneof case takes no label",
                                "remove `optional` or `repeated`; a oneof is already a choice of \
                                 exactly one case",
                            );
                        }
                        o.fields.push(f)
                    }
                    None => self.skip_statement(),
                },
            }
        }
        Some(o)
    }

    /// `[repeated|optional] Type name = N [options];`
    fn field(&mut self, oneof: Option<String>) -> Option<Field> {
        self.skip_trivia();
        let start = self.pos;
        let (first, first_span) = self.word("a field type")?;
        let (label, (ty_word, ty_span)) = match first.as_str() {
            "repeated" => (Label::Repeated, self.word("a field type")?),
            "optional" => (Label::Optional, self.word("a field type")?),
            _ => (Label::Single, (first, first_span)),
        };

        if ty_word == "map" || self.peek() == Tok::Punct('<') {
            self.unsupported(
                ty_span,
                "`map<>`",
                "a map field is sugar for a repeated entry message with its own wire layout, and \
                 Buri's `Map` is not ordered the way a decoded map would have to be",
                "declare `repeated Entry entries = N;` with an explicit `message Entry { ... }`",
            );
            return None;
        }
        if ty_word.ends_with("Any") && ty_word.contains("protobuf") {
            self.unsupported(
                ty_span,
                "`google.protobuf.Any`",
                "an `Any` holds a message whose type is known only at runtime, and a generated \
                 Buri type has to know its fields at compile time",
                "declare a `oneof` over the message types the field can actually hold",
            );
            return None;
        }

        let ty = match Scalar::parse(&ty_word) {
            Some(s) => TypeRef::Scalar(s),
            None => TypeRef::Named(ty_word),
        };
        let (name, _) = self.word("a field name")?;
        if !self.expect('=', "a field name") {
            return None;
        }
        self.skip_trivia();
        let num_at = self.pos;
        let number = match self.next() {
            Tok::Num(n) => n,
            other => {
                let span = Span::new(self.file, num_at, self.pos.max(num_at + 1));
                let found = other.describe();
                self.err(
                    span,
                    format!("expected a field number, found {found}"),
                    "every field carries a number, and the number is what goes on the wire",
                );
                return None;
            }
        };
        if number < 1 || number > 536870911 {
            self.err(
                Span::new(self.file, num_at, self.pos),
                format!("{number} is not a field number"),
                "field numbers run from 1 to 536870911",
            );
            return None;
        }
        if (19000..=19999).contains(&number) {
            self.err(
                Span::new(self.file, num_at, self.pos),
                format!("{number} is in the range protobuf reserves for itself"),
                "use a number outside 19000..19999",
            );
            return None;
        }
        // `[packed = ...]` is the one field option that changes what goes on
        // the wire, so it is the one this reader keeps. The rest — `ctype`,
        // `deprecated`, `json_name` — are read past.
        let mut packed = None;
        if self.peek() == Tok::Punct('[') {
            self.next();
            let mut last_word = String::new();
            loop {
                match self.next() {
                    Tok::Punct(']') | Tok::End => break,
                    Tok::Word(w) => {
                        if last_word == "packed" {
                            packed = match w.as_str() {
                                "true" => Some(true),
                                "false" => Some(false),
                                _ => packed,
                            };
                        }
                        last_word = w;
                    }
                    _ => {}
                }
            }
        }
        self.expect(';', "a field");
        Some(Field {
            name,
            number,
            label,
            ty,
            span: Span::new(self.file, start, self.pos),
            oneof,
            packed,
        })
    }

    fn enum_def(&mut self) -> Option<EnumDef> {
        let (name, span) = self.word("an enum name")?;
        if !self.expect('{', "an enum name") {
            return None;
        }
        let mut e = EnumDef { name, span, values: Vec::new() };
        loop {
            self.skip_trivia();
            let start = self.pos;
            match self.peek() {
                Tok::Punct('}') => {
                    self.next();
                    break;
                }
                Tok::End => {
                    self.err(
                        Span::new(self.file, start, start + 1),
                        format!("`enum {}` is not closed", e.name),
                        "close it with `}`",
                    );
                    break;
                }
                Tok::Punct(';') => {
                    self.next();
                }
                Tok::Word(w) if w == "option" || w == "reserved" => {
                    self.next();
                    self.skip_statement();
                }
                _ => {
                    let Some((value_name, value_span)) = self.word("an enum value name") else {
                        self.skip_statement();
                        continue;
                    };
                    if !self.expect('=', "an enum value name") {
                        self.skip_statement();
                        continue;
                    }
                    self.skip_trivia();
                    let num_at = self.pos;
                    let number = match self.next() {
                        Tok::Num(n) => n,
                        other => {
                            let span = Span::new(self.file, num_at, self.pos.max(num_at + 1));
                            let found = other.describe();
                            self.err(
                                span,
                                format!("expected an enum value number, found {found}"),
                                "every enum value carries the number that goes on the wire",
                            );
                            self.skip_statement();
                            continue;
                        }
                    };
                    if self.peek() == Tok::Punct('[') {
                        self.next();
                        loop {
                            match self.next() {
                                Tok::Punct(']') | Tok::End => break,
                                _ => {}
                            }
                        }
                    }
                    self.expect(';', "an enum value");
                    e.values.push(EnumValue { name: value_name, number, span: value_span });
                }
            }
        }
        // proto3 requires the first value to be zero, and the mapping leans on
        // it: an unrecognised number decodes to it.
        match e.values.first() {
            Some(v) if v.number != 0 => {
                let span = v.span;
                let name = v.name.clone();
                let enum_name = e.name.clone();
                self.err(
                    span,
                    format!("the first value of `{enum_name}` is not zero"),
                    format!("give `{name}` the number 0, or put a zero value first"),
                );
            }
            None => {
                let span = e.span;
                let enum_name = e.name.clone();
                self.err(
                    span,
                    format!("`{enum_name}` has no values"),
                    "an enum needs a zero value, which is what an unset field decodes to",
                );
            }
            _ => {}
        }
        Some(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> Schema {
        let r = parse(src, FileId(0));
        assert!(r.errors.is_empty(), "{:#?}", r.errors);
        r.schema
    }

    fn errors(src: &str) -> Vec<Diagnostic> {
        parse(src, FileId(0)).errors
    }

    const HEAD: &str = "syntax = \"proto3\";\n";

    #[test]
    fn reads_a_message_with_every_label() {
        let s = ok(&format!(
            "{HEAD}package a.b;\nmessage M {{\n  string a = 1;\n  optional int32 b = 2;\n  \
             repeated bytes c = 3;\n}}\n"
        ));
        assert_eq!(s.package.as_deref(), Some("a.b"));
        let m = &s.messages[0];
        assert_eq!(m.fields.len(), 3);
        assert_eq!(m.fields[0].label, Label::Single);
        assert_eq!(m.fields[1].label, Label::Optional);
        assert_eq!(m.fields[2].label, Label::Repeated);
        assert_eq!(m.fields[2].number, 3);
    }

    #[test]
    fn reads_nested_types_and_a_oneof() {
        let s = ok(&format!(
            "{HEAD}message Outer {{\n  message Inner {{ string t = 1; }}\n  enum E {{ E_ZERO = 0; \
             E_ONE = 1; }}\n  oneof pick {{ string a = 1; Inner b = 2; }}\n}}\n"
        ));
        let m = &s.messages[0];
        assert_eq!(m.messages[0].name, "Inner");
        assert_eq!(m.enums[0].values.len(), 2);
        assert_eq!(m.oneofs[0].name, "pick");
        assert_eq!(m.oneofs[0].fields.len(), 2);
        assert_eq!(m.oneofs[0].fields[1].oneof.as_deref(), Some("pick"));
    }

    #[test]
    fn comments_of_both_kinds_are_trivia() {
        let s = ok(&format!(
            "{HEAD}// line\n/* block\n   spanning */\nmessage M {{ int64 x = 1; /* here */ }}\n"
        ));
        assert_eq!(s.messages[0].fields.len(), 1);
    }

    #[test]
    fn imports_are_recorded_in_order() {
        let s = ok(&format!("{HEAD}import \"a/b.proto\";\nimport \"c.proto\";\n"));
        assert_eq!(s.imports.len(), 2);
        assert_eq!(s.imports[1].path, "c.proto");
    }

    /// Each unsupported construct is refused *by name*, because a message that
    /// says only "unsupported" is one nobody can act on.
    #[test]
    fn unsupported_constructs_are_named() {
        for (src, needle) in [
            ("service S { rpc Go (A) returns (B); }", "`service`"),
            ("extend M { int32 x = 100; }", "`extend`"),
            ("message M { extensions 100 to 200; }", "`extensions`"),
            ("message M { map<string, int32> m = 1; }", "`map<>`"),
            ("message M { group G = 1 { int32 x = 2; } }", "`group`"),
            ("message M { required int32 x = 1; }", "`required`"),
            ("message M { google.protobuf.Any a = 1; }", "`google.protobuf.Any`"),
            ("import public \"x.proto\";", "`import public`"),
        ] {
            let es = errors(&format!("{HEAD}{src}\n"));
            assert!(
                es.iter().any(|e| e.message.contains(needle)),
                "{src}\nwanted {needle}, got {es:#?}"
            );
            assert!(es.iter().all(|e| e.fix.is_some()), "{src}: a refusal with no fix");
        }
    }

    #[test]
    fn proto2_is_refused_and_a_missing_syntax_line_is_too() {
        assert!(errors("syntax = \"proto2\";\n")[0].message.contains("not supported"));
        assert!(errors("message M { int32 x = 1; }\n")
            .iter()
            .any(|e| e.message.contains("does not declare its syntax")));
    }

    #[test]
    fn a_field_number_outside_the_range_is_rejected() {
        assert!(errors(&format!("{HEAD}message M {{ int32 x = 0; }}"))[0]
            .message
            .contains("is not a field number"));
        assert!(errors(&format!("{HEAD}message M {{ int32 x = 19500; }}"))[0]
            .message
            .contains("reserves"));
    }

    #[test]
    fn an_enum_must_start_at_zero() {
        assert!(errors(&format!("{HEAD}enum E {{ E_ONE = 1; }}"))[0]
            .message
            .contains("is not zero"));
        assert!(errors(&format!("{HEAD}enum E {{ }}"))[0].message.contains("no values"));
    }

    /// `option` and `reserved` say nothing about the shape of a message, so
    /// they are skipped rather than refused — including the block form.
    #[test]
    fn options_and_reserved_are_skipped() {
        let s = ok(&format!(
            "{HEAD}option java_package = \"com.x\";\nmessage M {{\n  option (my.opt) = {{ a: 1 }};\n  \
             reserved 2, 15 to 20;\n  reserved \"old\";\n  int32 x = 1 [deprecated = true];\n}}\n"
        ));
        assert_eq!(s.messages[0].fields.len(), 1);
        assert_eq!(s.messages[0].fields[0].name, "x");
    }

    /// A malformed field does not swallow the rest of the file.
    #[test]
    fn recovery_keeps_reading() {
        let r = parse(&format!("{HEAD}message M {{ int32 = 1; string ok = 2; }}"), FileId(0));
        assert!(!r.errors.is_empty());
        assert_eq!(r.schema.messages[0].fields.len(), 1);
        assert_eq!(r.schema.messages[0].fields[0].name, "ok");
    }
}
