//! Just enough JSON to read `llvm-cov export`, which the gate does without a
//! dependency.

/// A JSON value. Numbers are `f64`: counts past 2^53 lose precision, and the
/// gate only asks whether a count is zero.
#[derive(Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    /// The member `key` of an object.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn items(&self) -> &[Value] {
        match self {
            Value::Array(items) => items,
            _ => &[],
        }
    }

    pub fn str(&self) -> &str {
        match self {
            Value::String(s) => s,
            _ => "",
        }
    }

    /// A count or a position: a negative or missing number is 0.
    pub fn u64(&self) -> u64 {
        match self {
            Value::Number(n) if *n > 0.0 => *n as u64,
            Value::Bool(true) => 1,
            _ => 0,
        }
    }
}

pub fn parse(text: &str) -> Result<Value, String> {
    let mut parser = Parser { bytes: text.as_bytes(), at: 0 };
    let value = parser.value()?;
    parser.space();
    if parser.at != parser.bytes.len() {
        return Err(format!("text after the value at byte {}", parser.at));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn space(&mut self) {
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.space();
        self.bytes.get(self.at).copied()
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        if self.peek() == Some(byte) {
            self.at += 1;
            Ok(())
        } else {
            Err(format!("expected `{}` at byte {}", byte as char, self.at))
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, String> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(format!("unexpected text at byte {}", self.at))
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        match self.peek() {
            Some(b'{') => {
                self.at += 1;
                let mut members = Vec::new();
                if self.peek() == Some(b'}') {
                    self.at += 1;
                    return Ok(Value::Object(members));
                }
                loop {
                    let key = self.string()?;
                    self.expect(b':')?;
                    members.push((key, self.value()?));
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        _ => {
                            self.expect(b'}')?;
                            return Ok(Value::Object(members));
                        }
                    }
                }
            }
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                if self.peek() == Some(b']') {
                    self.at += 1;
                    return Ok(Value::Array(items));
                }
                loop {
                    items.push(self.value()?);
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        _ => {
                            self.expect(b']')?;
                            return Ok(Value::Array(items));
                        }
                    }
                }
            }
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(_) => {
                let start = self.at;
                while self.bytes.get(self.at).is_some_and(|b| b"+-.eE0123456789".contains(b)) {
                    self.at += 1;
                }
                let text = std::str::from_utf8(&self.bytes[start..self.at]).unwrap_or("");
                text.parse().map(Value::Number).map_err(|_| format!("not a number at byte {start}"))
            }
            None => Err("unexpected end".to_string()),
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        loop {
            let Some(&byte) = self.bytes.get(self.at) else { return Err("unterminated string".to_string()) };
            self.at += 1;
            match byte {
                b'"' => return String::from_utf8(out).map_err(|e| e.to_string()),
                b'\\' => {
                    let Some(&escaped) = self.bytes.get(self.at) else { return Err("unterminated string".to_string()) };
                    self.at += 1;
                    let plain = match escaped {
                        b'n' => b'\n',
                        b't' => b'\t',
                        b'r' => b'\r',
                        b'b' => 8,
                        b'f' => 12,
                        b'u' => {
                            let hex = self.bytes.get(self.at..self.at + 4).ok_or("short \\u escape")?;
                            self.at += 4;
                            let code = u32::from_str_radix(std::str::from_utf8(hex).unwrap_or(""), 16)
                                .map_err(|e| e.to_string())?;
                            let c = char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER);
                            out.extend_from_slice(c.to_string().as_bytes());
                            continue;
                        }
                        other => other,
                    };
                    out.push(plain);
                }
                other => out.push(other),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llvm_covs_shapes_parse() {
        let value = parse(r#" {"a": [1, -2.5e1, "x\"A\n"], "b": {"c": true, "d": null}, "e": []} "#).unwrap();
        assert_eq!(value.get("a").unwrap().items()[0].u64(), 1);
        assert_eq!(value.get("a").unwrap().items()[1], Value::Number(-25.0));
        assert_eq!(value.get("a").unwrap().items()[2].str(), "x\"A\n");
        assert_eq!(value.get("b").unwrap().get("c"), Some(&Value::Bool(true)));
        assert_eq!(value.get("e").unwrap().items(), &[]);
        assert!(parse("[1,]").is_err());
        assert!(parse("[1] 2").is_err());
    }
}
