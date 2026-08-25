//! JSON, in the amount the toolchain needs and no more.
//!
//! There is no serde here for the same reason there is no anything-else: a
//! dependency tree is a second thing to audit, and this is a compiler. What
//! the language server needs is a reader — the protocol arrives as
//! JSON and has to be understood, not just produced — and a writer that agrees
//! with `diagnostics::json_str`, which was already escaping strings for
//! `--error-format=json` before this file existed.
//!
//! Deliberately not implemented: duplicate object keys keep the
//! last one; and the reader rejects trailing data rather than ignoring it. The
//! protocol this reads is machine-written, so being strict costs nothing and
//! turns a malformed message into a message rather than a guess.

use crate::diagnostics::json_str;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// A `f64` that is neither NaN nor infinite.
///
/// JSON has no spelling for either one. Holding them in a [`Value`] means the
/// writer has to invent something, and what it invented was Rust's — `NaN` and
/// `inf` — which went straight out on the language-server wire and are rejected
/// by every client that reads them. The constructor is the only way in, so the
/// writer has no such case to handle.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Finite(f64);

impl Finite {
    /// `None` for NaN and for either infinity, which JSON cannot express.
    pub fn new(n: f64) -> Option<Finite> {
        n.is_finite().then_some(Finite(n))
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

impl From<Finite> for f64 {
    fn from(n: Finite) -> f64 {
        n.0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// Every number the protocol itself uses is an integer — line, character,
    /// severity, symbol kind, request id. Keeping them a separate variant is
    /// what makes writing one exact rather than a guess about whether a `f64`
    /// that happens to be whole was meant to print as `5` or as `5.0`.
    Int(i64),
    /// A number that is not whole. Never NaN, never infinite.
    Float(Finite),
    Str(String),
    Array(Vec<Value>),
    /// Ordered, so writing a value back produces the same bytes every time —
    /// which is what lets a test record a response as a golden.
    Object(BTreeMap<String, Value>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(m) => m.get(key),
            _ => None,
        }
    }

    /// `a.b.c`, for reaching into a request without a chain of `?`s.
    pub fn at(&self, path: &str) -> Option<&Value> {
        let mut here = self;
        for part in path.split('.') {
            here = here.get(part)?;
        }
        Some(here)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The protocol's integers arrive as JSON numbers. A position or an id that
    /// is not a whole number is a malformed message, so this returns `None`
    /// rather than truncating — and now that whole numbers have their own
    /// variant, "is it whole" is the variant rather than a `fract()` test.
    pub fn as_u32(&self) -> Option<u32> {
        match self {
            Value::Int(n) => u32::try_from(*n).ok(),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(v) => Some(v),
            _ => None,
        }
    }

    pub fn object(pairs: Vec<(&str, Value)>) -> Value {
        Value::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn str(s: impl Into<String>) -> Value {
        Value::Str(s.into())
    }

    pub fn number(n: impl Into<i64>) -> Value {
        Value::Int(n.into())
    }

    /// A non-whole number. `None` for NaN and the infinities, which have no
    /// JSON spelling — a caller that can produce one has to say what it means
    /// by it rather than emitting bytes no client will accept.
    pub fn float(n: f64) -> Option<Value> {
        Finite::new(n).map(Value::Float)
    }

    pub fn write(&self, out: &mut String) {
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(true) => out.push_str("true"),
            Value::Bool(false) => out.push_str("false"),
            // A whole number prints without a fractional part, because the
            // protocol's line numbers are integers and `5.0` reads as a
            // different thing to anyone looking at the transcript. Which of the
            // two it is, is the variant, not a range check on a `f64`.
            Value::Int(n) => {
                let _ = write!(out, "{n}");
            }
            // `{:?}` rather than `{}` so a float always keeps a decimal point
            // and reads back as a float. It cannot print `NaN` or `inf`:
            // `Finite` has no way to hold either.
            Value::Float(n) => {
                let _ = write!(out, "{:?}", n.get());
            }
            Value::Str(s) => out.push_str(&json_str(s)),
            Value::Array(items) => {
                out.push('[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write(out);
                }
                out.push(']');
            }
            Value::Object(m) => {
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&json_str(k));
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }

    /// The value as JSON text.
    ///
    /// Deliberately not `Display`/`ToString`: `Value::Str("x").to_string()` has
    /// to be `"\"x\""` and not `x`, and a `Display` impl that quotes its
    /// argument is a trap for everything that formats a value into a message.
    #[expect(
        clippy::inherent_to_string,
        reason = "see above: a `Display` impl for a JSON value would be quoted where a reader expects the string"
    )]
    pub fn to_string(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }
}

/// How deep an array or object may nest.
///
/// The reader is recursive, so nesting is stack, and the text it reads arrives
/// on a socket: `[[[[[…` a hundred thousand deep is four bytes of typing and a
/// segmentation fault, which is the one failure a diagnostic cannot be made
/// out of. A language-server message is a handful of objects deep, so this is
/// three orders of magnitude of headroom and still nowhere near a stack.
const MAX_DEPTH: u32 = 128;

/// Parses one complete value. Trailing data is an error, not a stopping point.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut p = Parser { b: text.as_bytes(), i: 0, depth: 0 };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i != p.b.len() {
        return Err(format!("trailing data at byte {}", p.i));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
    /// Arrays and objects open so far. See [`MAX_DEPTH`].
    depth: u32,
}

impl<'a> Parser<'a> {
    fn ws(&mut self) {
        while self.b.get(self.i).is_some_and(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r')) {
            self.bump();
        }
    }

    /// One byte forward. Saturating because the alternative is a wrap, and the
    /// input is a message someone else wrote.
    fn bump(&mut self) {
        self.i = self.i.saturating_add(1);
    }

    fn eat(&mut self, c: u8) -> Result<(), String> {
        if self.b.get(self.i) == Some(&c) {
            self.bump();
            Ok(())
        } else {
            Err(format!("expected `{}` at byte {}", c as char, self.i))
        }
    }

    fn literal(&mut self, word: &str, v: Value) -> Result<Value, String> {
        if self.b.get(self.i..).is_some_and(|rest| rest.starts_with(word.as_bytes())) {
            self.i = self.i.saturating_add(word.len());
            Ok(v)
        } else {
            Err(format!("expected `{word}` at byte {}", self.i))
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        match self.b.get(self.i) {
            None => Err("expected a value, found the end of the input".into()),
            Some(b'n') => self.literal("null", Value::Null),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'"') => self.string().map(Value::Str),
            Some(b'[') => self.nested(Parser::array),
            Some(b'{') => self.nested(Parser::object),
            Some(_) => self.number(),
        }
    }

    /// Reads one array or object, counting the nesting so that
    /// [`MAX_DEPTH`] is a refusal rather than a stack overflow.
    fn nested(
        &mut self,
        read: fn(&mut Parser<'a>) -> Result<Value, String>,
    ) -> Result<Value, String> {
        self.depth = self.depth.saturating_add(1);
        if self.depth > MAX_DEPTH {
            return Err(format!("nested more than {MAX_DEPTH} deep, at byte {}", self.i));
        }
        let v = read(self);
        self.depth = self.depth.saturating_sub(1);
        v
    }

    fn array(&mut self) -> Result<Value, String> {
        self.eat(b'[')?;
        let mut items = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b']') {
            self.bump();
            return Ok(Value::Array(items));
        }
        loop {
            self.ws();
            items.push(self.value()?);
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.bump(),
                Some(b']') => {
                    self.bump();
                    return Ok(Value::Array(items));
                }
                _ => return Err(format!("expected `,` or `]` at byte {}", self.i)),
            }
        }
    }

    fn object(&mut self) -> Result<Value, String> {
        self.eat(b'{')?;
        let mut map = BTreeMap::new();
        self.ws();
        if self.b.get(self.i) == Some(&b'}') {
            self.bump();
            return Ok(Value::Object(map));
        }
        loop {
            self.ws();
            let key = self.string()?;
            self.ws();
            self.eat(b':')?;
            self.ws();
            map.insert(key, self.value()?);
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.bump(),
                Some(b'}') => {
                    self.bump();
                    return Ok(Value::Object(map));
                }
                _ => return Err(format!("expected `,` or `}}` at byte {}", self.i)),
            }
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.eat(b'"')?;
        let mut out = String::new();
        loop {
            let Some(&c) = self.b.get(self.i) else {
                return Err("a string is never closed".into());
            };
            self.bump();
            match c {
                b'"' => return Ok(out),
                b'\\' => {
                    let Some(&e) = self.b.get(self.i) else {
                        return Err("an escape is never completed".into());
                    };
                    self.bump();
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.unicode_escape()?),
                        _ => return Err(format!("unknown escape `\\{}`", e as char)),
                    }
                }
                // Multi-byte UTF-8 arrives as raw bytes; gather the whole
                // sequence rather than pushing each byte as a character.
                c if c < 0x80 => out.push(c as char),
                c => {
                    let extra = match c {
                        0xC0..=0xDF => 1,
                        0xE0..=0xEF => 2,
                        _ => 3,
                    };
                    let start = self.i.saturating_sub(1);
                    self.i = self.i.saturating_add(extra);
                    let Some(s) = self.b.get(start..self.i).and_then(|b| std::str::from_utf8(b).ok())
                    else {
                        return Err(format!("invalid UTF-8 at byte {start}"));
                    };
                    out.push_str(s);
                }
            }
        }
    }

    /// `\uXXXX`, including the surrogate pair a character above the basic plane
    /// arrives as. An unpaired surrogate becomes the replacement character
    /// rather than an error: the message is still readable, and refusing it
    /// would drop a whole request over one bad character.
    fn unicode_escape(&mut self) -> Result<char, String> {
        let hi = self.hex4()?;
        if !(0xD800..0xDC00).contains(&hi) {
            return Ok(char::from_u32(hi).unwrap_or('\u{fffd}'));
        }
        if self.b.get(self.i) == Some(&b'\\')
            && self.b.get(self.i.saturating_add(1)) == Some(&b'u')
        {
            self.i = self.i.saturating_add(2);
            let lo = self.hex4()?;
            if (0xDC00..0xE000).contains(&lo) {
                // `hi` is in D800..DC00 and `lo` in DC00..E000, checked just
                // above, so this is the surrogate-pair formula on operands it
                // is defined for and cannot leave the scalar range.
                let c = 0x10000u32
                    .saturating_add(hi.saturating_sub(0xD800) << 10)
                    .saturating_add(lo.saturating_sub(0xDC00));
                return Ok(char::from_u32(c).unwrap_or('\u{fffd}'));
            }
        }
        Ok('\u{fffd}')
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let end = self.i.saturating_add(4);
        let Some(s) = self.b.get(self.i..end).and_then(|b| std::str::from_utf8(b).ok()) else {
            return Err(format!("a \\u escape needs four digits, at byte {}", self.i));
        };
        let n = u32::from_str_radix(s, 16)
            .map_err(|_| format!("`{s}` is not four hex digits, at byte {}", self.i))?;
        self.i = end;
        Ok(n)
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.i;
        if self.b.get(self.i) == Some(&b'-') {
            self.bump();
        }
        while self
            .b
            .get(self.i)
            .is_some_and(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'))
        {
            self.bump();
        }
        let s = self.b.get(start..self.i).and_then(|b| std::str::from_utf8(b).ok()).unwrap_or("");
        // A number spelled without a point or an exponent is an integer, which
        // is what every number in this protocol is. Anything else is a float,
        // and one that overflows `f64` to infinity — `1e999` — is rejected here
        // rather than becoming a value that cannot be written back out.
        if !s.contains(['.', 'e', 'E']) {
            if let Ok(n) = s.parse::<i64>() {
                return Ok(Value::Int(n));
            }
        }
        let n = s.parse::<f64>().map_err(|_| format!("`{s}` is not a number"))?;
        Value::float(n).ok_or_else(|| format!("`{s}` is not a finite number"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_the_shapes_the_protocol_uses() {
        let text = r#"{"id":1,"method":"textDocument/hover","params":{"position":{"line":3,"character":7}}}"#;
        let v = parse(text).expect("parses");
        assert_eq!(v.at("params.position.line").and_then(|v| v.as_u32()), Some(3));
        assert_eq!(v.get("method").and_then(|v| v.as_str()), Some("textDocument/hover"));
        // Objects are ordered, so writing is deterministic and recordable.
        assert_eq!(parse(&v.to_string()).unwrap(), v);
    }

    #[test]
    fn a_whole_number_prints_without_a_fraction() {
        assert_eq!(Value::number(3).to_string(), "3");
        assert_eq!(Value::number(-1).to_string(), "-1");
    }

    /// `NaN` and `inf` are not JSON. They used to be representable, and
    /// `Value::write` emitted them verbatim onto the language-server wire,
    /// where a real client rejects the whole message. There is now no `Value`
    /// that can hold one.
    #[test]
    fn a_number_that_json_cannot_express_cannot_be_built() {
        assert_eq!(Value::float(f64::NAN), None);
        assert_eq!(Value::float(f64::INFINITY), None);
        assert_eq!(Value::float(f64::NEG_INFINITY), None);
        assert_eq!(Value::float(1.5).map(|v| v.to_string()), Some("1.5".to_string()));
        // And a literal that overflows to infinity is refused at the reader.
        assert!(parse("1e999").is_err());
    }

    #[test]
    fn integers_and_floats_round_trip_as_themselves() {
        for text in ["0", "-7", "9007199254740993", "1.5", "-0.25", "2.5e3"] {
            let v = parse(text).expect(text);
            assert_eq!(parse(&v.to_string()).unwrap(), v, "{text}");
        }
        assert_eq!(parse("42").unwrap(), Value::Int(42));
        assert!(matches!(parse("4.25").unwrap(), Value::Float(_)));
        // A float is never mistaken for an integer offset.
        assert_eq!(parse("3.0").unwrap().as_u32(), None);
        assert_eq!(parse("3").unwrap().as_u32(), Some(3));
    }

    #[test]
    fn escapes_survive_the_round_trip() {
        let v = Value::str("a\"b\\c\nd\te\u{1}f");
        assert_eq!(parse(&v.to_string()).unwrap(), v);
    }

    #[test]
    fn reads_non_ascii_and_surrogate_pairs() {
        assert_eq!(parse(r#""héllo""#).unwrap(), Value::str("héllo"));
        assert_eq!(parse(r#""😀""#).unwrap(), Value::str("😀"));
        // A raw multi-byte character is one character, not four.
        assert_eq!(parse(r#""😀""#).unwrap(), Value::str("😀"));
    }

    #[test]
    fn trailing_data_is_an_error() {
        assert!(parse("{} {}").is_err());
        assert!(parse("[1,2]junk").is_err());
    }

    #[test]
    fn malformed_input_is_reported_rather_than_guessed_at() {
        for bad in ["{", "[1,", r#"{"a"}"#, r#""unclosed"#, "tru", "{,}"] {
            assert!(parse(bad).is_err(), "`{bad}` should not parse");
        }
    }

    #[test]
    fn empty_containers() {
        assert_eq!(parse("{}").unwrap(), Value::Object(BTreeMap::new()));
        assert_eq!(parse("[]").unwrap(), Value::Array(Vec::new()));
    }

    /// The reader is recursive and its input arrives on a socket, so nesting is
    /// stack. Two hundred thousand brackets is four bytes of typing and used to
    /// be a segmentation fault — the one failure a diagnostic cannot be made
    /// out of.
    #[test]
    fn nesting_past_the_limit_is_an_error_rather_than_a_stack_overflow() {
        for text in [
            format!("{}1{}", "[".repeat(200_000), "]".repeat(200_000)),
            format!("{}1{}", "{\"a\":".repeat(200_000), "}".repeat(200_000)),
            "[".repeat(200_000),
        ] {
            let err = parse(&text).expect_err("nesting this deep is refused");
            assert!(err.contains("nested more than"), "{err}");
        }
    }

    /// Right at the edge, so the limit is the limit rather than approximately
    /// it.
    #[test]
    fn nesting_up_to_the_limit_is_read() {
        let n = MAX_DEPTH as usize;
        let ok = format!("{}1{}", "[".repeat(n), "]".repeat(n));
        assert!(parse(&ok).is_ok(), "{n} deep should read");
        let one_more = format!("{}1{}", "[".repeat(n + 1), "]".repeat(n + 1));
        assert!(parse(&one_more).is_err(), "{} deep should not", n + 1);
    }
}
