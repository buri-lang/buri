//! A reader for `.proto` *schemas*.
//!
//! `build/textproto.rs` reads proto *values* — the dialect `BUILD.buri` is
//! written in. This reads the other half of the format: the declarations, so
//! that `from "//proto/person.proto" import { Person };` can become a module.
//! The two share nothing but a family resemblance, which is why they are two
//! files.
//!
//! **Editions only, and one edition.** A schema declares
//! `edition = "2026";` — not `syntax = "proto3"`, not proto2, not an earlier
//! edition. See [`REQUIRED_EDITION`].
//!
//! ```text
//! edition = "2026";
//! package example.v1;
//! import "proto/address.proto";
//!
//! enum Colour { COLOUR_UNSPECIFIED = 0; RED = 1; }
//!
//! message Person {
//!   string name = 1;                                  // presence tracked
//!   int32 age = 2 [features.field_presence = IMPLICIT];  // and here it is not
//!   repeated string emails = 3;
//!   Address home = 4;
//!   oneof contact { string phone = 5; string email = 6; }
//!   message Nickname { string text = 1; }
//! }
//! ```
//!
//! Editions replaced the `optional` and `required` labels with a *feature*:
//! `features.field_presence`, which defaults to `EXPLICIT` and can be set per
//! file, per message, or per field. That is the one change with teeth, because
//! it decides whether a singular field is `Option<T>` or `T` — see
//! `cli/src/docs/build/proto.md`.
//!
//! What is *not* here is refused by name rather than ignored: `service`,
//! `extend`, `extensions`, groups, `map<>`, the two removed labels, and every
//! feature value whose semantics this mapping cannot express. Each would change what a message means on the wire, and a reader
//! that skipped one would decode the file in front of it as a different file.
//! `reserved`, and an `option` that is not a `features.` one, are skipped
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
    /// A singular field. Whether its presence is tracked is
    /// `features.field_presence`, not the label — editions removed `optional`.
    Single,
    Repeated,
}

/// The edition this reader implements, and the only one it accepts.
///
/// **One constant, deliberately.** Every wire- and JSON-affecting feature
/// resolves identically at editions 2023, 2024 and 2026 — protobuf's rule is
/// that a feature takes the default of the closest edition at or before it, and
/// no such default was introduced after 2023 — so moving the requirement
/// forward is this line and the fixtures, and nothing in the mapping.
const REQUIRED_EDITION: &str = "2026";

/// `features.field_presence`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Presence {
    /// The editions default: the field is on the wire only if it was set, and
    /// a field set to its zero value is still on the wire.
    Explicit,
    /// What proto3 called a singular field: no presence, and a value equal to
    /// the type's default is indistinguishable from an absent one.
    Implicit,
}

/// `features.repeated_field_encoding`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RepeatedEncoding {
    Packed,
    Expanded,
}

/// The subset of `FeatureSet` this reader models, unresolved: `None` is
/// "inherited from the enclosing scope, or from the edition".
///
/// Everything else protobuf's `FeatureSet` carries either has no bearing on
/// what a message means — `enforce_naming_style` and
/// `default_symbol_visibility` are source-retention lints — or is refused by
/// name where it is written.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Features {
    pub field_presence: Option<Presence>,
    pub repeated_field_encoding: Option<RepeatedEncoding>,
}

impl Features {
    /// The edition's own defaults, which is where every resolution bottoms out.
    pub fn edition_defaults() -> Features {
        Features {
            field_presence: Some(Presence::Explicit),
            repeated_field_encoding: Some(RepeatedEncoding::Packed),
        }
    }

    /// `self` layered over `outer`: a scope overrides what encloses it.
    pub fn over(self, outer: Features) -> Features {
        Features {
            field_presence: self.field_presence.or(outer.field_presence),
            repeated_field_encoding: self
                .repeated_field_encoding
                .or(outer.repeated_field_encoding),
        }
    }

    pub fn presence(self) -> Presence {
        self.field_presence.unwrap_or(Presence::Explicit)
    }

    pub fn packed(self) -> bool {
        self.repeated_field_encoding.unwrap_or(RepeatedEncoding::Packed) == RepeatedEncoding::Packed
    }
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub number: i64,
    pub label: Label,
    pub ty: TypeRef,
    pub span: Span,
    /// This field's own `features.…`, before resolution against the message
    /// and the file.
    pub features: Features,
}

/// One case of a `oneof`.
///
/// **It has no label, and that is the point.** A `repeated` case is not a thing
/// protobuf has — a oneof is already a choice of exactly one — so rather than
/// diagnosing one and carrying it anyway, the type a case is parsed into cannot
/// hold a label at all. A labelled case is refused at the one place a [`Field`]
/// becomes a `OneofCase`, and nothing downstream has to remember that a
/// diagnostic was issued.
#[derive(Clone, Debug)]
pub struct OneofCase {
    pub name: String,
    pub number: i64,
    pub ty: TypeRef,
    pub span: Span,
    pub features: Features,
}

#[derive(Clone, Debug)]
pub struct Oneof {
    pub name: String,
    pub span: Span,
    pub cases: Vec<OneofCase>,
}

#[derive(Clone, Debug)]
pub struct EnumValue {
    pub name: String,
    pub number: i64,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Enum {
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
    pub enums: Vec<Enum>,
    /// Declared on the message, and inherited by its fields and its nested
    /// types.
    pub features: Features,
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
    pub enums: Vec<Enum>,
    /// The file's own `option features.…`, layered over the edition's defaults
    /// by [`Features::edition_defaults`] when a field is resolved.
    pub features: Features,
}

pub struct Parsed {
    pub schema: Schema,
    pub errors: Vec<Diagnostic>,
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

pub fn parse(text: &str, file: FileId) -> Parsed {
    let mut p =
        Parser { src: text.as_bytes(), text, position: 0, file, errors: Vec::new(), depth: 0 };
    let schema = p.file();
    Parsed { schema, errors: p.errors }
}

struct Parser<'a> {
    src: &'a [u8],
    text: &'a str,
    position: usize,
    file: FileId,
    errors: Vec<Diagnostic>,
    depth: u32,
}

/// One token. A `.proto` schema needs no more vocabulary than this.
#[derive(Clone, Debug, PartialEq)]
enum Token {
    Word(String),
    Num(i64),
    Str(String),
    Punctuation(char),
    End,
}

impl Token {
    fn describe(&self) -> String {
        match self {
            Token::Word(w) => format!("`{w}`"),
            Token::Num(n) => format!("`{n}`"),
            Token::Str(s) => format!("\"{s}\""),
            Token::Punctuation(c) => format!("`{c}`"),
            Token::End => "the end of the file".to_string(),
        }
    }
}

impl<'a> Parser<'a> {
    /// Every way a schema can be malformed shares one code, so the sentence and
    /// the edit are what each site supplies.
    fn err(&mut self, span: Span, msg: impl Into<String>, fix: impl Into<String>) {
        if self.errors.len() < 32 {
            self.errors.push(
                Diagnostic::templated("proto-schema", span)
                    .with_bind("problem", msg)
                    .with_bind("remedy", fix),
            );
        }
    }

    /// A construct this reader refuses rather than ignores. Named, always,
    /// because "unsupported" without a noun is a message you cannot act on.
    fn unsupported(&mut self, span: Span, what: &str, why: &str, fix: impl Into<String>) {
        if self.errors.len() < 32 {
            self.errors.push(
                Diagnostic::templated("proto-unsupported", span)
                    .with_bind("construct", what)
                    .with_bind("reason", why)
                    .with_bind("remedy", fix),
            );
        }
    }

    fn peek_byte(&self) -> u8 {
        *self.src.get(self.position).unwrap_or(&0)
    }

    fn byte_after(&self) -> Option<&u8> {
        self.src.get(self.position.saturating_add(1))
    }

    /// The text from the cursor on, or `""` if the cursor has run past the end.
    fn rest(&self) -> &'a str {
        self.text.get(self.position..).unwrap_or("")
    }

    fn slice(&self, start: usize, end: usize) -> &'a str {
        self.text.get(start..end).unwrap_or("")
    }

    /// Steps over one whole character.
    ///
    /// Recovery steps by a character rather than a byte, because a cursor left
    /// *inside* a multi-byte character turns every later slice of the text into
    /// a panic — and a schema is free to contain one anywhere.
    fn bump_char(&mut self) {
        let step = self.rest().chars().next().map_or(1, char::len_utf8);
        self.position = self.position.saturating_add(step);
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek_byte() {
                b' ' | b'\t' | b'\r' | b'\n' => self.position = self.position.saturating_add(1),
                b'/' if self.byte_after() == Some(&b'/') => {
                    while self.position < self.src.len() && self.peek_byte() != b'\n' {
                        self.bump_char();
                    }
                }
                b'/' if self.byte_after() == Some(&b'*') => {
                    let start = self.position;
                    self.position = self.position.saturating_add(2);
                    loop {
                        let Some(&next) = self.byte_after() else {
                            self.position = self.src.len();
                            let span = Span::new(self.file, start, self.position);
                            self.err(span, "unterminated block comment", "close it with `*/`");
                            return;
                        };
                        if self.peek_byte() == b'*' && next == b'/' {
                            self.position = self.position.saturating_add(2);
                            break;
                        }
                        self.bump_char();
                    }
                }
                _ => return,
            }
        }
    }

    /// The next token, consuming it. The span of what was read is
    /// `start..self.pos` for whatever `start` the caller recorded first.
    fn next(&mut self) -> Token {
        self.skip_trivia();
        if self.position >= self.src.len() {
            return Token::End;
        }
        let c = self.peek_byte();
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = self.position;
            while self.peek_byte().is_ascii_alphanumeric()
                || self.peek_byte() == b'_'
                || self.peek_byte() == b'.'
            {
                self.position = self.position.saturating_add(1);
            }
            return Token::Word(self.slice(start, self.position).to_string());
        }
        if c.is_ascii_digit() || (c == b'-' && self.byte_after().is_some_and(|d| d.is_ascii_digit())) {
            let start = self.position;
            if c == b'-' {
                self.position = self.position.saturating_add(1);
            }
            while self.peek_byte().is_ascii_alphanumeric() {
                self.position = self.position.saturating_add(1);
            }
            let raw = self.slice(start, self.position);
            let parsed = if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
                i64::from_str_radix(hex, 16).ok()
            } else {
                raw.parse::<i64>().ok()
            };
            return match parsed {
                Some(n) => Token::Num(n),
                None => {
                    let span = Span::new(self.file, start, self.position);
                    let raw = raw.to_string();
                    self.err(span, format!("`{raw}` is not a number"), "write a decimal or `0x` field number");
                    Token::Num(0)
                }
            };
        }
        if c == b'"' || c == b'\'' {
            let quote = c;
            let start = self.position;
            self.position = self.position.saturating_add(1);
            let mut s = String::new();
            loop {
                if self.position >= self.src.len() || self.peek_byte() == b'\n' {
                    let span = Span::new(self.file, start, self.position);
                    self.err(span, "unterminated string", "close it with a quote");
                    return Token::Str(s);
                }
                let ch = self.peek_byte();
                if ch == quote {
                    self.position = self.position.saturating_add(1);
                    return Token::Str(s);
                }
                if ch == b'\\' {
                    self.position = self.position.saturating_add(1);
                    // An escape names a character, not a byte: `\é` has to step
                    // over the whole `é`, or the cursor lands inside it. A
                    // backslash at the end of the input leaves the cursor
                    // there, and the loop head reports the unterminated string.
                    if let Some(e) = self.rest().chars().next() {
                        self.position = self.position.saturating_add(e.len_utf8());
                        s.push(match e {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            other => other,
                        });
                    }
                    continue;
                }
                match self.rest().chars().next() {
                    Some(ch) => {
                        self.position = self.position.saturating_add(ch.len_utf8());
                        s.push(ch);
                    }
                    // The loop head established there is input left, so this
                    // says the text is not valid UTF-8 from here; step off the
                    // byte rather than spin on it.
                    None => self.position = self.position.saturating_add(1),
                }
            }
        }
        // A byte that begins no token is one punctuation character. Stepping by
        // the whole character keeps the cursor on a boundary when the schema
        // holds a stray `é` or an emoji.
        let punctuation = self.rest().chars().next().unwrap_or(c as char);
        self.bump_char();
        Token::Punctuation(punctuation)
    }

    fn peek(&mut self) -> Token {
        let save = self.position;
        let t = self.next();
        self.position = save;
        t
    }

    /// Consumes a specific punctuation token, reporting when it is not there.
    fn expect(&mut self, c: char, context: &str) -> bool {
        let start = {
            self.skip_trivia();
            self.position
        };
        let t = self.next();
        if t == Token::Punctuation(c) {
            return true;
        }
        let span = Span::new(self.file, start, self.position.max(start.saturating_add(1)));
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
        let start = self.position;
        match self.next() {
            Token::Word(w) => Some((w, Span::new(self.file, start, self.position))),
            other => {
                let span = Span::new(self.file, start, self.position.max(start.saturating_add(1)));
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

    /// An `option` statement, after the word `option`.
    ///
    /// A `features.…` one is read; everything else is skipped, because an
    /// ordinary option is how a schema talks to code generators that are not
    /// this one and says nothing about the shape of a message.
    fn feature_option(&mut self) -> Option<Features> {
        self.skip_trivia();
        let start = self.position;
        let name = match self.next() {
            Token::Word(w) => w,
            _ => {
                self.position = start;
                self.skip_statement();
                return None;
            }
        };
        // `option features = { ... };` sets several at once. Refused rather
        // than read: the block form nests, and one spelling of a thing is
        // enough for a reader this size to have to be right about.
        if name == "features" && self.peek() == Token::Punctuation('=') {
            self.next();
            if self.peek() == Token::Punctuation('{') {
                let span = Span::new(self.file, start, self.position);
                self.unsupported(
                    span,
                    "`option features = { ... }`",
                    "the block form sets several features at once, and this reader takes one \
                     spelling of a thing rather than two",
                    "write them one at a time: `option features.field_presence = IMPLICIT;`",
                );
            }
            self.skip_statement();
            return None;
        }
        let Some(feature) = name.strip_prefix("features.") else {
            self.position = start;
            self.skip_statement();
            return None;
        };
        let feature = feature.to_string();
        self.expect('=', "an option name");
        self.skip_trivia();
        let value_at = self.position;
        let value = match self.next() {
            Token::Word(v) => v,
            other => {
                let span =
                    Span::new(self.file, value_at, self.position.max(value_at.saturating_add(1)));
                let found = other.describe();
                self.err(
                    span,
                    format!("expected a feature value, found {found}"),
                    "a feature's value is a bare word, as in `IMPLICIT`",
                );
                self.skip_statement();
                return None;
            }
        };
        let span = Span::new(self.file, start, self.position);
        let out = self.feature(&feature, &value, span);
        self.expect(';', "an option");
        out
    }

    /// One `features.<name> = <value>`, wherever it was written.
    ///
    /// Every value protobuf's `FeatureSet` admits is named here, and each one
    /// is either honoured or refused *by name*. A feature silently ignored is a
    /// schema that means something other than what it says.
    fn feature(&mut self, name: &str, value: &str, span: Span) -> Option<Features> {
        let mut out = Features::default();
        match (name, value) {
            ("field_presence", "EXPLICIT") => out.field_presence = Some(Presence::Explicit),
            ("field_presence", "IMPLICIT") => out.field_presence = Some(Presence::Implicit),
            ("field_presence", "LEGACY_REQUIRED") => self.unsupported(
                span,
                "`features.field_presence = LEGACY_REQUIRED`",
                "a required field is a promise the format cannot keep across versions, which is \
                 why editions carries it only to describe proto2 files",
                "leave the presence at its default, which is EXPLICIT",
            ),
            ("repeated_field_encoding", "PACKED") => {
                out.repeated_field_encoding = Some(RepeatedEncoding::Packed)
            }
            ("repeated_field_encoding", "EXPANDED") => {
                out.repeated_field_encoding = Some(RepeatedEncoding::Expanded)
            }
            // Honoured by being the default and the only thing implemented: an
            // enum is open, and a value the schema does not know survives a
            // round trip as `Unrecognized`.
            ("enum_type", "OPEN") => {}
            ("enum_type", "CLOSED") => self.unsupported(
                span,
                "`features.enum_type = CLOSED`",
                "a closed enum makes an unrecognised value an unknown *field*, which a generated \
                 struct has nowhere to keep; open enums keep it in the value itself",
                "leave the enum open, which is edition 2026's default",
            ),
            ("message_encoding", "LENGTH_PREFIXED") => {}
            ("message_encoding", "DELIMITED") => self.unsupported(
                span,
                "`features.message_encoding = DELIMITED`",
                "delimited is the old group encoding, and a reader has to understand a group to \
                 get past one — so treating it as an unknown field would read the rest of the \
                 message at the wrong offset",
                "leave the encoding length-prefixed, which is edition 2026's default",
            ),
            // `Str` is text or it is not one, so a string field is validated
            // whatever this says; turning validation off would mean a `Str`
            // holding bytes that are not one.
            ("utf8_validation", "VERIFY") => {}
            ("utf8_validation", "NONE") => self.unsupported(
                span,
                "`features.utf8_validation = NONE`",
                "a `string` field becomes a `Str`, and a `Str` is text — there is no unvalidated \
                 one for the bytes to become",
                "declare the field `bytes` if it carries octets rather than text",
            ),
            ("json_format", "ALLOW") => {}
            ("json_format", "LEGACY_BEST_EFFORT") => self.unsupported(
                span,
                "`features.json_format = LEGACY_BEST_EFFORT`",
                "it exists to describe what proto2 files did to JSON, and this reader writes the \
                 one JSON mapping editions defines",
                "leave the format at ALLOW, which is edition 2026's default",
            ),
            // Source-retention lints. They say nothing about what a message
            // means, so they are read past rather than refused.
            ("enforce_naming_style" | "default_symbol_visibility", _) => {}
            (other, v) => {
                let known = [
                    "field_presence",
                    "enum_type",
                    "repeated_field_encoding",
                    "utf8_validation",
                    "message_encoding",
                    "json_format",
                ];
                let mut d = Diagnostic::templated("proto-unknown-feature", span)
                    .with_bind("feature", other)
                    .with_bind("value", v)
                    .with_bind("known_features", known.join(", "));
                // A near miss replaces the page's list, which is the answer when
                // there is no name close enough to guess at.
                if let Some(near) = crate::build::buildfile::nearest(other, &known) {
                    d = d.with_fix(format!("did you mean `features.{near}`?"));
                }
                if self.errors.len() < 32 {
                    self.errors.push(d);
                }
            }
        }
        Some(out)
    }

    /// Everything up to and including the next `;`, for the statements this
    /// reader records nothing from.
    fn skip_statement(&mut self) {
        loop {
            match self.next() {
                Token::Punctuation(';') | Token::End => return,
                // An option value may be a block: `option (x) = { a: 1 };`.
                Token::Punctuation('{') => self.skip_braces(),
                _ => {}
            }
        }
    }

    fn skip_braces(&mut self) {
        let mut depth: u32 = 1;
        loop {
            match self.next() {
                Token::Punctuation('{') => depth = depth.saturating_add(1),
                Token::Punctuation('}') => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return;
                    }
                }
                Token::End => return,
                _ => {}
            }
        }
    }

    // -- the file ----------------------------------------------------------

    fn file(&mut self) -> Schema {
        let mut schema = Schema::default();
        let mut seen_edition = false;
        loop {
            self.skip_trivia();
            let start = self.position;
            let t = self.next();
            match t {
                Token::End => break,
                Token::Punctuation(';') => {}
                Token::Word(w) => match w.as_str() {
                    // `syntax` is what a proto2 or proto3 file declares, and
                    // neither is accepted. The diagnostic is about migrating
                    // rather than about the word, because the word is not the
                    // problem: presence, defaults and enum openness all differ,
                    // and a file that says `proto3` means something this
                    // mapping deliberately no longer implements.
                    "syntax" => {
                        seen_edition = true;
                        self.expect('=', "`syntax`");
                        self.skip_trivia();
                        let at = self.position;
                        let value = self.next();
                        let named = match &value {
                            Token::Str(v) => format!("`syntax = \"{v}\"`"),
                            other => format!("syntax {}", other.describe()),
                        };
                        let span =
                            Span::new(self.file, start, self.position.max(at.saturating_add(1)));
                        self.errors.push(
                            Diagnostic::templated("proto-syntax-declaration", span)
                                .with_bind("declaration", named)
                                .with_bind("edition", REQUIRED_EDITION),
                        );
                        self.expect(';', "the syntax declaration");
                    }
                    "edition" => {
                        seen_edition = true;
                        self.expect('=', "`edition`");
                        self.skip_trivia();
                        let at = self.position;
                        let value = self.next();
                        if value != Token::Str(REQUIRED_EDITION.to_string()) {
                            let span =
                                Span::new(self.file, at, self.position.max(at.saturating_add(1)));
                            let named = match &value {
                                Token::Str(v) => format!("edition {v}"),
                                other => format!("edition {}", other.describe()),
                            };
                            self.errors.push(
                                Diagnostic::templated("proto-edition", span)
                                    .with_bind("declaration", named)
                                    .with_bind("edition", REQUIRED_EDITION),
                            );
                        }
                        self.expect(';', "the edition declaration");
                    }
                    "package" => {
                        if let Some((name, _)) = self.word("a package name") {
                            schema.package = Some(name);
                        }
                        self.expect(';', "the package declaration");
                    }
                    "import" => {
                        self.skip_trivia();
                        let at = self.position;
                        match self.next() {
                            Token::Str(path) => {
                                let span = Span::new(self.file, at, self.position);
                                schema.imports.push(Import { path, span })
                            }
                            Token::Word(w) if w == "public" || w == "weak" => {
                                let span = Span::new(self.file, at, self.position);
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
                                let end = self.position.max(at.saturating_add(1));
                                let span = Span::new(self.file, at, end);
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
                    "option" => {
                        if let Some(f) = self.feature_option() {
                            schema.features = f.over(schema.features);
                        }
                    }
                    "reserved" => self.skip_statement(),
                    "message" => {
                        if let Some(m) = self.message() {
                            schema.messages.push(m);
                        }
                    }
                    "enum" => {
                        if let Some(e) = self.enum_declaration() {
                            schema.enums.push(e);
                        }
                    }
                    "service" | "extend" | "extensions" => {
                        let span = Span::new(self.file, start, self.position);
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
                        let span = Span::new(self.file, start, self.position);
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
                    let span =
                        Span::new(self.file, start, self.position.max(start.saturating_add(1)));
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
        if !seen_edition {
            self.errors.push(
                Diagnostic::templated("proto-edition-missing", Span::point(self.file, 0))
                    .with_bind("edition", REQUIRED_EDITION),
            );
        }
        schema
    }

    /// After an unsupported block-shaped declaration: step over the block if
    /// there is one, so one refusal does not become twenty.
    fn recover_block(&mut self) {
        loop {
            match self.peek() {
                Token::Punctuation('{') => {
                    self.next();
                    self.skip_braces();
                    return;
                }
                Token::Punctuation(';') | Token::End => {
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
        self.depth = self.depth.saturating_add(1);
        if self.depth > 16 {
            self.err(span, "messages nest too deeply", "flatten them; sixteen levels is the limit");
            self.depth = self.depth.saturating_sub(1);
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
            features: Features::default(),
        };
        loop {
            self.skip_trivia();
            let start = self.position;
            match self.peek() {
                Token::Punctuation('}') => {
                    self.next();
                    break;
                }
                Token::End => {
                    self.err(
                        Span::new(self.file, start, start.saturating_add(1)),
                        format!("`{}` is not closed", m.name),
                        "close the message with `}`",
                    );
                    break;
                }
                Token::Punctuation(';') => {
                    self.next();
                }
                Token::Word(w) => match w.as_str() {
                    "message" => {
                        self.next();
                        if let Some(inner) = self.message() {
                            m.messages.push(inner);
                        }
                    }
                    "enum" => {
                        self.next();
                        if let Some(e) = self.enum_declaration() {
                            m.enums.push(e);
                        }
                    }
                    "oneof" => {
                        self.next();
                        if let Some(o) = self.oneof() {
                            m.oneofs.push(o);
                        }
                    }
                    "option" => {
                        self.next();
                        if let Some(f) = self.feature_option() {
                            m.features = f.over(m.features);
                        }
                    }
                    "reserved" => {
                        self.next();
                        self.skip_statement();
                    }
                    "extensions" | "extend" => {
                        self.next();
                        let span = Span::new(self.file, start, self.position);
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
                        let span = Span::new(self.file, start, self.position);
                        self.unsupported(
                            span,
                            "`group`",
                            "groups are proto2's inline nesting, and their wire encoding was \
                             removed from proto3",
                            "declare a nested `message` and a field of that type",
                        );
                        self.recover_block();
                    }
                    // Editions removed both labels: presence is a feature
                    // now, and protoc refuses these in exactly the same way.
                    "required" => {
                        self.next();
                        let span = Span::new(self.file, start, self.position);
                        self.unsupported(
                            span,
                            "the `required` label",
                            "editions replaced the labels with `features.field_presence`, and the \
                             value that corresponds to `required` is LEGACY_REQUIRED — which this \
                             reader refuses in its own right",
                            "drop the label; a singular field already has presence",
                        );
                        self.skip_statement();
                    }
                    "optional" => {
                        self.next();
                        let span = Span::new(self.file, start, self.position);
                        self.unsupported(
                            span,
                            "the `optional` label",
                            "editions replaced the labels with `features.field_presence`, whose \
                             default is EXPLICIT — so a singular field already tracks presence \
                             and `optional` would say it twice",
                            "drop the label; write `[features.field_presence = IMPLICIT]` on the \
                             fields that should *not* track presence",
                        );
                        self.skip_statement();
                    }
                    _ => match self.field() {
                        Some(f) => m.fields.push(f),
                        None => self.skip_statement(),
                    },
                },
                _ => match self.field() {
                    Some(f) => m.fields.push(f),
                    None => self.skip_statement(),
                },
            }
        }
        self.depth = self.depth.saturating_sub(1);
        Some(m)
    }

    fn oneof(&mut self) -> Option<Oneof> {
        let (name, span) = self.word("a oneof name")?;
        if !self.expect('{', "a oneof name") {
            return None;
        }
        let mut o = Oneof { name: name.clone(), span, cases: Vec::new() };
        loop {
            self.skip_trivia();
            let start = self.position;
            match self.peek() {
                Token::Punctuation('}') => {
                    self.next();
                    break;
                }
                Token::End => {
                    self.err(
                        Span::new(self.file, start, start.saturating_add(1)),
                        format!("`oneof {}` is not closed", o.name),
                        "close it with `}`",
                    );
                    break;
                }
                Token::Punctuation(';') => {
                    self.next();
                }
                Token::Word(w) if w == "option" => {
                    self.next();
                    self.feature_option();
                }
                _ => match self.field() {
                    // The one place a field becomes a case, and the only place
                    // a label can be refused — past here a case has nowhere to
                    // keep one.
                    Some(f) => {
                        let c = self.case(f);
                        o.cases.push(c);
                    }
                    None => self.skip_statement(),
                },
            }
        }
        Some(o)
    }

    /// A [`Field`] as one case of a `oneof`.
    ///
    /// This is the only place a label can be observed on something headed for a
    /// oneof, so it is the only place one can be refused — and past here there
    /// is nothing to refuse, because [`OneofCase`] has no field to hold a label
    /// in. The case is still *kept*: the error already fails the build, and
    /// dropping it would take a second round of diagnostics about the same
    /// schema away from whoever has to fix it.
    fn case(&mut self, f: Field) -> OneofCase {
        if f.label != Label::Single {
            self.err(
                f.span,
                "a oneof case takes no label",
                "remove `repeated`; a oneof is already a choice of exactly one case, so a \
                 repeated one has no meaning to give it",
            );
        }
        OneofCase {
            name: f.name,
            number: f.number,
            ty: f.ty,
            span: f.span,
            features: f.features,
        }
    }

    /// `[repeated] Type name = N [options];`
    fn field(&mut self) -> Option<Field> {
        self.skip_trivia();
        let start = self.position;
        let (first, first_span) = self.word("a field type")?;
        let (label, (ty_word, ty_span)) = match first.as_str() {
            "repeated" => (Label::Repeated, self.word("a field type")?),
            // Both removed labels are refused where they are written, so that
            // a migrated file is told once per field rather than once per file.
            "optional" | "required" => {
                let span = Span::new(self.file, start, self.position);
                let which = first.clone();
                self.unsupported(
                    span,
                    &format!("the `{which}` label"),
                    "editions replaced the labels with `features.field_presence`, whose default \
                     is EXPLICIT",
                    if which == "optional" {
                        "drop the label; a singular field already tracks presence"
                    } else {
                        "drop the label; a field that must be there is a promise the format \
                         cannot keep across versions"
                    },
                );
                (Label::Single, self.word("a field type")?)
            }
            _ => (Label::Single, (first, first_span)),
        };

        if ty_word == "map" || self.peek() == Token::Punctuation('<') {
            self.unsupported(
                ty_span,
                "`map<>`",
                "a map field is sugar for a repeated entry message with its own wire layout, and \
                 Buri's `Map` is not ordered the way a decoded map would have to be",
                "declare `repeated Entry entries = N;` with an explicit `message Entry { ... }`",
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
        let num_at = self.position;
        let number = match self.next() {
            Token::Num(n) => n,
            other => {
                let span =
                    Span::new(self.file, num_at, self.position.max(num_at.saturating_add(1)));
                let found = other.describe();
                self.err(
                    span,
                    format!("expected a field number, found {found}"),
                    "every field carries a number, and the number is what goes on the wire",
                );
                return None;
            }
        };
        if !(1..=536870911).contains(&number) {
            self.err(
                Span::new(self.file, num_at, self.position),
                format!("{number} is not a field number"),
                "field numbers run from 1 to 536870911",
            );
            return None;
        }
        if (19000..=19999).contains(&number) {
            self.err(
                Span::new(self.file, num_at, self.position),
                format!("{number} is in the range protobuf reserves for itself"),
                "use a number outside 19000..19999",
            );
            return None;
        }
        // The field's own options. `features.…` is read and everything else —
        // `ctype`, `deprecated`, `json_name` — is stepped over.
        let mut features = Features::default();
        if self.peek() == Token::Punctuation('[') {
            self.next();
            loop {
                self.skip_trivia();
                let at = self.position;
                match self.next() {
                    Token::Punctuation(']') | Token::End => break,
                    Token::Punctuation(',') => {}
                    Token::Word(w) => {
                        let feature = w.strip_prefix("features.").map(str::to_string);
                        // `name = value`, whatever the name was.
                        if self.peek() == Token::Punctuation('=') {
                            self.next();
                            self.skip_trivia();
                            let value = self.next();
                            if let Some(feature) = feature {
                                let span = Span::new(self.file, at, self.position);
                                let value = match value {
                                    Token::Word(v) => v,
                                    other => other.describe(),
                                };
                                if let Some(f) = self.feature(&feature, &value, span) {
                                    features = f.over(features);
                                }
                            }
                        }
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
            span: Span::new(self.file, start, self.position),
            features,
        })
    }

    fn enum_declaration(&mut self) -> Option<Enum> {
        let (name, span) = self.word("an enum name")?;
        if !self.expect('{', "an enum name") {
            return None;
        }
        let mut e = Enum { name, span, values: Vec::new() };
        loop {
            self.skip_trivia();
            let start = self.position;
            match self.peek() {
                Token::Punctuation('}') => {
                    self.next();
                    break;
                }
                Token::End => {
                    self.err(
                        Span::new(self.file, start, start.saturating_add(1)),
                        format!("`enum {}` is not closed", e.name),
                        "close it with `}`",
                    );
                    break;
                }
                Token::Punctuation(';') => {
                    self.next();
                }
                Token::Word(w) if w == "option" => {
                    self.next();
                    // Read for its diagnostics: only `field_presence` and
                    // `repeated_field_encoding` are modelled and neither
                    // targets an enum, so there is nothing here to keep.
                    self.feature_option();
                }
                Token::Word(w) if w == "reserved" => {
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
                    let num_at = self.position;
                    let number = match self.next() {
                        Token::Num(n) => n,
                        other => {
                            let end = self.position.max(num_at.saturating_add(1));
                            let span = Span::new(self.file, num_at, end);
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
                    if self.peek() == Token::Punctuation('[') {
                        self.next();
                        loop {
                            match self.next() {
                                Token::Punctuation(']') | Token::End => break,
                                _ => {}
                            }
                        }
                    }
                    self.expect(';', "an enum value");
                    e.values.push(EnumValue { name: value_name, number, span: value_span });
                }
            }
        }
        // An open enum's first value must be zero, and the mapping leans on it:
        // it is what an unset field decodes to.
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

    const HEAD: &str = "edition = \"2026\";\n";

    #[test]
    fn reads_a_message_with_every_label() {
        let s = ok(&format!(
            "{HEAD}package a.b;\nmessage M {{\n  string a = 1;\n  \
             int32 b = 2 [features.field_presence = IMPLICIT];\n  repeated bytes c = 3;\n}}\n"
        ));
        assert_eq!(s.package.as_deref(), Some("a.b"));
        let m = &s.messages[0];
        assert_eq!(m.fields.len(), 3);
        assert_eq!(m.fields[0].label, Label::Single);
        assert_eq!(m.fields[0].features.field_presence, None, "unset means inherited");
        assert_eq!(m.fields[1].features.field_presence, Some(Presence::Implicit));
        assert_eq!(m.fields[2].label, Label::Repeated);
        assert_eq!(m.fields[2].number, 3);
    }

    /// A `.proto` is user input, so no schema may crash the reader. The escape
    /// case once did: the lexer stepped one *byte* past a backslash, which left
    /// the cursor inside `é` and made the next slice of the text a panic.
    #[test]
    fn no_schema_can_crash_the_reader() {
        let cases = [
            "edition = \"\\é\";",                       // an escape wider than a byte
            "edition = \"2026\";\nmessage M { string \\é = 1; }",
            "é",                                        // a stray character where a word belongs
            "message é {",
            "edition = \"2026\";\nmessage M { int32 x = 99999999999999999999; }",
            "/*",                                       // an unterminated block comment
            "/* é",
            "\"",                                       // an unterminated string
            "edition = \"2026\";\nmessage M {",         // an unclosed message
            "edition = \"2026\";\nenum E {",
            "edition = \"2026\";\nmessage M { oneof o {",
            "edition = \"2026\";\noption features.",
        ];
        for src in cases {
            let r = parse(src, FileId(0));
            assert!(!r.errors.is_empty(), "{src:?} was accepted");
            assert!(r.errors.iter().all(|e| e.fix.is_some()), "{src:?}: an error with no fix");
        }
    }

    /// Nested messages are read by recursion, so the nesting is bounded: a file
    /// that is nothing but `message M {` would otherwise exhaust the stack.
    #[test]
    fn pathological_nesting_is_a_diagnostic_rather_than_a_stack_overflow() {
        let deep = format!("{HEAD}{}", "message M {".repeat(200_000));
        let r = parse(&deep, FileId(0));
        assert!(r.errors.iter().any(|e| e.message.contains("nest too deeply")), "{:#?}", r.errors);
    }

    /// The edition is the one thing every file has to get right, so each way of
    /// getting it wrong is refused separately and says what to do.
    #[test]
    fn only_the_required_edition_is_accepted() {
        assert_eq!(REQUIRED_EDITION, "2026");
        // Three refusals rather than one: a `syntax` file wants the migration,
        // an older edition wants one line changed, and a file with no
        // declaration wants a line added.
        let cases = [
            (
                "syntax = \"proto3\";\n",
                "`syntax = \"proto3\"` is not accepted",
                "proto-syntax-declaration",
            ),
            (
                "syntax = \"proto2\";\n",
                "`syntax = \"proto2\"` is not accepted",
                "proto-syntax-declaration",
            ),
            ("edition = \"2023\";\n", "edition 2023 is not accepted", "proto-edition"),
            ("edition = \"2024\";\n", "edition 2024 is not accepted", "proto-edition"),
            (
                "message M { int32 x = 1; }\n",
                "does not declare its edition",
                "proto-edition-missing",
            ),
        ];
        for (src, want, code) in cases {
            let es = errors(src);
            assert!(
                es.iter().any(|e| e.message.contains(want)),
                "{src}: wanted {want}, got {es:#?}"
            );
            assert!(es.iter().all(|e| e.fix.is_some()), "{src}: a refusal with no fix");
            assert!(
                es.iter().any(|e| e.code.as_deref() == Some(code)),
                "{src}: not filed under {code}"
            );
        }
        assert!(errors(&format!("{HEAD}message M {{ int32 x = 1; }}\n")).is_empty());
    }

    /// Editions removed both labels. protoc refuses them; so does this.
    #[test]
    fn the_removed_labels_are_refused_by_name() {
        for label in ["optional", "required"] {
            let es = errors(&format!("{HEAD}message M {{ {label} int32 x = 1; }}\n"));
            assert!(
                es.iter().any(|e| e.message.contains(&format!("the `{label}` label"))),
                "{label}: {es:#?}"
            );
        }
    }

    /// A feature is honoured or refused by name, at whichever scope it was
    /// written. One silently ignored would be a schema that means something
    /// other than what it says.
    #[test]
    fn features_resolve_from_the_file_inwards() {
        let s = ok(&format!(
            "{HEAD}option features.field_presence = IMPLICIT;\n\
             message M {{\n  option features.repeated_field_encoding = EXPANDED;\n  \
             int32 a = 1;\n  int32 b = 2 [features.field_presence = EXPLICIT];\n}}\n"
        ));
        assert_eq!(s.features.field_presence, Some(Presence::Implicit));
        let m = &s.messages[0];
        assert_eq!(m.features.repeated_field_encoding, Some(RepeatedEncoding::Expanded));
        // The file's value reaches a field that says nothing...
        assert_eq!(m.fields[0].features.over(m.features.over(s.features)).presence(), Presence::Implicit);
        // ...and the field's own wins where it does.
        assert_eq!(m.fields[1].features.over(m.features.over(s.features)).presence(), Presence::Explicit);
    }

    #[test]
    fn the_edition_defaults_are_the_ones_protobuf_resolves() {
        let d = Features::edition_defaults();
        assert_eq!(d.presence(), Presence::Explicit);
        assert!(d.packed());
    }

    #[test]
    fn unimplementable_feature_values_are_refused_by_name() {
        for (feature, needle) in [
            ("features.field_presence = LEGACY_REQUIRED", "LEGACY_REQUIRED"),
            ("features.enum_type = CLOSED", "CLOSED"),
            ("features.message_encoding = DELIMITED", "DELIMITED"),
            ("features.utf8_validation = NONE", "NONE"),
            ("features.json_format = LEGACY_BEST_EFFORT", "LEGACY_BEST_EFFORT"),
        ] {
            let es = errors(&format!("{HEAD}option {feature};\n"));
            assert!(es.iter().any(|e| e.message.contains(needle)), "{feature}: {es:#?}");
            assert!(es.iter().all(|e| e.fix.is_some()), "{feature}: a refusal with no fix");
        }
        // And one nobody has heard of, with a near miss offered.
        let es = errors(&format!("{HEAD}option features.field_presense = EXPLICIT;\n"));
        assert!(es[0].message.contains("not a feature this reader knows"), "{es:#?}");
        assert!(es[0].fix.as_deref().is_some_and(|f| f.contains("field_presence")));
        // The block form is one spelling too many.
        let es = errors(&format!("{HEAD}option features = {{ field_presence: IMPLICIT }};\n"));
        assert!(es.iter().any(|e| e.message.contains("option features = { ... }")), "{es:#?}");
    }

    /// Source-retention lints say nothing about what a message means, so they
    /// are read past rather than refused — which is what lets a schema opt out
    /// of the naming style protoc enforces from 2024 on.
    #[test]
    fn source_only_features_are_read_past() {
        let s = ok(&format!(
            "{HEAD}option features.enforce_naming_style = STYLE_LEGACY;\n\
             message M {{ int32 FieldName8 = 1; }}\n"
        ));
        assert_eq!(s.messages[0].fields.len(), 1);
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
        assert_eq!(m.oneofs[0].cases.len(), 2);
        assert_eq!(m.oneofs[0].cases[1].name, "b");
    }

    /// A `repeated` oneof case is refused — and the case that comes back has
    /// nowhere to *hold* the label, so no code downstream can act on one. The
    /// case itself is kept, because the error already fails the build and
    /// dropping it would hide whatever else is wrong with the same field.
    #[test]
    fn a_oneof_case_cannot_carry_a_label() {
        let r = parse(
            &format!("{HEAD}message M {{ oneof pick {{ repeated string a = 1; string b = 2; }} }}"),
            FileId(0),
        );
        assert!(
            r.errors.iter().any(|e| e.message.contains("a oneof case takes no label")),
            "{:#?}",
            r.errors
        );
        let cases = &r.schema.messages[0].oneofs[0].cases;
        assert_eq!(cases.len(), 2, "the case is kept so the rest of it is still checked");
        assert_eq!(cases[0].name, "a");
        // `OneofCase` has no `label` field at all: this is a type-level claim,
        // and it is checked by this file compiling.
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

    /// `google.protobuf.Any` is a two-field message and is read as one. The
    /// refusal used to fire on the spelling, before any name was resolved.
    #[test]
    fn a_well_known_any_is_an_ordinary_named_type() {
        let s = ok(&format!("{HEAD}message M {{ google.protobuf.Any payload = 1; }}\n"));
        let field = &s.messages[0].fields[0];
        assert!(
            matches!(&field.ty, TypeRef::Named(n) if n == "google.protobuf.Any"),
            "{:?}",
            field.ty
        );
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
