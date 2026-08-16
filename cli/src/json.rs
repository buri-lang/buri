//! JSON, in the amount the toolchain needs and no more.
//!
//! There is no serde here for the same reason there is no anything-else: the
//! toolchain is pinned by hash, and a dependency tree is a second thing to
//! pin. What the language server needs is a reader — the protocol arrives as
//! JSON and has to be understood, not just produced — and a writer that agrees
//! with `diag::json_str`, which was already escaping strings for
//! `--error-format=json` before this file existed.
//!
//! Deliberately not implemented: numbers are `f64` and nothing else, so there
//! is no integer/float distinction to get wrong; duplicate object keys keep the
//! last one; and the reader rejects trailing data rather than ignoring it. The
//! protocol this reads is machine-written, so being strict costs nothing and
//! turns a malformed message into a message rather than a guess.

use crate::diag::json_str;
use std::collections::BTreeMap;
use std::fmt::Write as _;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Value>),
    /// Ordered, so writing a value back produces the same bytes every time —
    /// which is what lets a test record a response as a golden.
    Obj(BTreeMap<String, Value>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(m) => m.get(key),
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

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// The protocol's integers arrive as JSON numbers. A position or an id that
    /// is not a whole number is a malformed message, so this returns `None`
    /// rather than truncating.
    pub fn as_u32(&self) -> Option<u32> {
        let n = self.as_f64()?;
        (n.fract() == 0.0 && n >= 0.0 && n <= u32::MAX as f64).then_some(n as u32)
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Arr(v) => Some(v),
            _ => None,
        }
    }

    pub fn obj(pairs: Vec<(&str, Value)>) -> Value {
        Value::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn str(s: impl Into<String>) -> Value {
        Value::Str(s.into())
    }

    pub fn num(n: impl Into<f64>) -> Value {
        Value::Num(n.into())
    }

    pub fn write(&self, out: &mut String) {
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(true) => out.push_str("true"),
            Value::Bool(false) => out.push_str("false"),
            Value::Num(n) => {
                // A whole number prints without a fractional part, because the
                // protocol's line numbers are integers and `5.0` reads as a
                // different thing to anyone looking at the transcript.
                if n.fract() == 0.0 && n.is_finite() && n.abs() < 1e15 {
                    let _ = write!(out, "{}", *n as i64);
                } else {
                    let _ = write!(out, "{n}");
                }
            }
            Value::Str(s) => out.push_str(&json_str(s)),
            Value::Arr(items) => {
                out.push('[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write(out);
                }
                out.push(']');
            }
            Value::Obj(m) => {
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

    pub fn to_string(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }
}

/// Parses one complete value. Trailing data is an error, not a stopping point.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut p = Parser { b: text.as_bytes(), i: 0 };
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
}

impl<'a> Parser<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Result<(), String> {
        if self.i < self.b.len() && self.b[self.i] == c {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected `{}` at byte {}", c as char, self.i))
        }
    }

    fn literal(&mut self, word: &str, v: Value) -> Result<Value, String> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
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
            Some(b'[') => self.array(),
            Some(b'{') => self.object(),
            Some(_) => self.number(),
        }
    }

    fn array(&mut self) -> Result<Value, String> {
        self.eat(b'[')?;
        let mut items = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b']') {
            self.i += 1;
            return Ok(Value::Arr(items));
        }
        loop {
            self.ws();
            items.push(self.value()?);
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Value::Arr(items));
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
            self.i += 1;
            return Ok(Value::Obj(map));
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
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Value::Obj(map));
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
            self.i += 1;
            match c {
                b'"' => return Ok(out),
                b'\\' => {
                    let Some(&e) = self.b.get(self.i) else {
                        return Err("an escape is never completed".into());
                    };
                    self.i += 1;
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
                    let start = self.i - 1;
                    self.i += extra;
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
        if self.b.get(self.i) == Some(&b'\\') && self.b.get(self.i + 1) == Some(&b'u') {
            self.i += 2;
            let lo = self.hex4()?;
            if (0xDC00..0xE000).contains(&lo) {
                let c = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                return Ok(char::from_u32(c).unwrap_or('\u{fffd}'));
            }
        }
        Ok('\u{fffd}')
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let Some(s) = self.b.get(self.i..self.i + 4).and_then(|b| std::str::from_utf8(b).ok())
        else {
            return Err(format!("a \\u escape needs four digits, at byte {}", self.i));
        };
        let n = u32::from_str_radix(s, 16)
            .map_err(|_| format!("`{s}` is not four hex digits, at byte {}", self.i))?;
        self.i += 4;
        Ok(n)
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.i;
        if self.b.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        while self
            .b
            .get(self.i)
            .is_some_and(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'))
        {
            self.i += 1;
        }
        let s = std::str::from_utf8(&self.b[start..self.i]).unwrap_or("");
        s.parse::<f64>().map(Value::Num).map_err(|_| format!("`{s}` is not a number"))
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
        assert_eq!(Value::num(3.0).to_string(), "3");
        assert_eq!(Value::num(-1.0).to_string(), "-1");
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
        assert_eq!(parse("{}").unwrap(), Value::Obj(BTreeMap::new()));
        assert_eq!(parse("[]").unwrap(), Value::Arr(Vec::new()));
    }
}
