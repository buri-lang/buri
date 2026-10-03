//! A repository platform's `js` file: what it exports, whether that is every
//! method it has to implement, and the one module the build writes out of it
//! and the compiled program.
//!
//! The file is JavaScript the toolchain does not compile, so it is read with a
//! small tokenizer rather than a parser. That is enough for the two shapes a
//! production struct is written in, `export const HostKv = { get: ... }` and
//! `export function`, and for finding the file's imports and exports so the
//! bundle can move them. A struct written any other way is reported as a shape
//! the build cannot read, which is the same refusal as a missing method:
//! `host-file-incomplete`.
//!
//! Two modules are the artifact's own rather than files: `buri:program`, the
//! entries the file calls, and `buri:ui`, the reactive graph's `signal` and
//! `write`, for a file whose production struct answers a signal, as `web`'s
//! `HostLocation` does.

/// One lexical token of the file, with where it starts and ends.
#[derive(Clone, Debug, PartialEq)]
enum Token {
    Ident(String),
    Str(String),
    Punct(char),
    /// `=>`, kept whole because an arrow is what marks a function value.
    Arrow,
    /// A template literal, a regular expression or a number: something whose
    /// insides the reader never needs.
    Opaque,
}

#[derive(Clone, Debug)]
struct Lexeme {
    token: Token,
    start: usize,
    end: usize,
}

/// Whether a `/` after this token starts a regular expression rather than a
/// division: it does after anything that cannot end an expression.
fn regex_may_follow(prev: Option<&Token>) -> bool {
    match prev {
        None => true,
        Some(Token::Punct(c)) => !matches!(c, ')' | ']' | '}'),
        Some(Token::Arrow) => true,
        Some(Token::Ident(w)) => {
            matches!(w.as_str(), "return" | "typeof" | "instanceof" | "in" | "of" | "new" | "delete" | "void" | "throw" | "case" | "do" | "else" | "yield" | "await")
        }
        Some(Token::Str(_)) | Some(Token::Opaque) => false,
    }
}

fn lex(src: &str) -> Vec<Lexeme> {
    let bytes = src.as_bytes();
    let mut out: Vec<Lexeme> = Vec::new();
    let mut i = 0usize;
    let at = |i: usize| bytes.get(i).copied().unwrap_or(0);
    while i < bytes.len() {
        let c = at(i);
        let start = i;
        if c.is_ascii_whitespace() {
            i = i.saturating_add(1);
            continue;
        }
        if c == b'/' && at(i.saturating_add(1)) == b'/' {
            while i < bytes.len() && at(i) != b'\n' {
                i = i.saturating_add(1);
            }
            continue;
        }
        if c == b'/' && at(i.saturating_add(1)) == b'*' {
            i = i.saturating_add(2);
            while i < bytes.len() && !(at(i) == b'*' && at(i.saturating_add(1)) == b'/') {
                i = i.saturating_add(1);
            }
            i = (i.saturating_add(2)).min(bytes.len());
            continue;
        }
        if c == b'"' || c == b'\'' {
            i = i.saturating_add(1);
            let mut text = Vec::new();
            while i < bytes.len() && at(i) != c && at(i) != b'\n' {
                if at(i) == b'\\' {
                    i = i.saturating_add(1);
                }
                text.push(at(i));
                i = i.saturating_add(1);
            }
            i = (i.saturating_add(1)).min(bytes.len());
            out.push(Lexeme { token: Token::Str(String::from_utf8_lossy(&text).to_string()), start, end: i });
            continue;
        }
        if c == b'`' {
            // A template literal, with its `${ }` holes skipped by depth.
            i = i.saturating_add(1);
            let mut holes = 0usize;
            while i < bytes.len() {
                match at(i) {
                    b'\\' => i = i.saturating_add(1),
                    b'`' if holes == 0 => break,
                    b'$' if at(i.saturating_add(1)) == b'{' => {
                        holes = holes.saturating_add(1);
                        i = i.saturating_add(1);
                    }
                    b'}' if holes > 0 => holes = holes.saturating_sub(1),
                    _ => {}
                }
                i = i.saturating_add(1);
            }
            i = (i.saturating_add(1)).min(bytes.len());
            out.push(Lexeme { token: Token::Opaque, start, end: i });
            continue;
        }
        if c == b'/' && regex_may_follow(out.last().map(|l| &l.token)) {
            i = i.saturating_add(1);
            let mut class = false;
            while i < bytes.len() && at(i) != b'\n' {
                match at(i) {
                    b'\\' => i = i.saturating_add(1),
                    b'[' => class = true,
                    b']' => class = false,
                    b'/' if !class => break,
                    _ => {}
                }
                i = i.saturating_add(1);
            }
            i = i.saturating_add(1);
            while at(i).is_ascii_alphabetic() {
                i = i.saturating_add(1);
            }
            out.push(Lexeme { token: Token::Opaque, start, end: i.min(bytes.len()) });
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' || c == b'$' || c >= 0x80 {
            while i < bytes.len() && (at(i).is_ascii_alphanumeric() || at(i) == b'_' || at(i) == b'$' || at(i) >= 0x80) {
                i = i.saturating_add(1);
            }
            out.push(Lexeme { token: Token::Ident(src.get(start..i).unwrap_or_default().to_string()), start, end: i });
            continue;
        }
        if c.is_ascii_digit() {
            while i < bytes.len() && (at(i).is_ascii_alphanumeric() || at(i) == b'.' || at(i) == b'_') {
                i = i.saturating_add(1);
            }
            out.push(Lexeme { token: Token::Opaque, start, end: i });
            continue;
        }
        if c == b'=' && at(i.saturating_add(1)) == b'>' {
            i = i.saturating_add(2);
            out.push(Lexeme { token: Token::Arrow, start, end: i });
            continue;
        }
        // Every other byte is punctuation; a multi-byte operator is a run of
        // these, which nothing below needs whole.
        i = i.saturating_add(1);
        out.push(Lexeme { token: Token::Punct(c as char), start, end: i });
    }
    out
}

/// The shape one export was written in.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// `export const X = { key: (a, b) => ... }`: each key, with how many
    /// parameters its function takes, or `None` where the value is not a
    /// function the reader can count.
    Object(Vec<(String, Option<usize>)>),
    /// `export function X(a, b)`: how many parameters it takes.
    Function(usize),
    /// Anything else: a call, a class, a re-export.
    Other,
}

/// One top-level export of the file.
#[derive(Clone, Debug)]
pub struct Export {
    /// The name it is exported under; `default` for the default export.
    pub name: String,
    /// The name it has inside the file.
    pub local: String,
    pub shape: Shape,
    /// Where its `export` keyword is, for a diagnostic to point at.
    pub at: usize,
}

/// What a bundle does to one span of the file.
#[derive(Clone, Debug)]
enum Edit {
    /// Replace these bytes with this text.
    Replace { start: usize, end: usize, with: String },
}

/// What the reader found.
#[derive(Clone, Debug, Default)]
pub struct Exports {
    pub exports: Vec<Export>,
    /// The import statements other than `buri:program`'s, verbatim: they
    /// stay at the top of the module.
    imports: Vec<String>,
    edits: Vec<Edit>,
    /// Whether the file waits at its top level, so its body runs in an
    /// `async` function.
    waits: bool,
    /// Whether the file imports `buri:ui`, the reactive graph the backend
    /// publishes, so the program has to hand it over.
    pub ui: bool,
}

impl Exports {
    pub fn get(&self, name: &str) -> Option<&Export> {
        self.exports.iter().find(|e| e.name == name)
    }
}

/// How many parameters a parenthesised list starting at `open` holds, and the
/// index just past its `)`.
fn count_params(lexemes: &[Lexeme], open: usize) -> Option<(usize, usize)> {
    if lexemes.get(open)?.token != Token::Punct('(') {
        return None;
    }
    let mut depth = 0usize;
    let mut commas = 0usize;
    let mut any = false;
    let mut i = open;
    loop {
        let l = lexemes.get(i)?;
        match &l.token {
            Token::Punct('(' | '[' | '{') => depth = depth.saturating_add(1),
            Token::Punct(')' | ']' | '}') => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some((if any { commas.saturating_add(1) } else { 0 }, i.saturating_add(1)));
                }
            }
            Token::Punct(',') if depth == 1 => {
                // A trailing comma adds no parameter.
                if !matches!(lexemes.get(i.saturating_add(1)).map(|l| &l.token), Some(Token::Punct(')'))) {
                    commas = commas.saturating_add(1);
                }
            }
            _ if depth >= 1 => any = true,
            _ => {}
        }
        i = i.saturating_add(1);
    }
}

/// How many parameters the function value starting at `i` takes:
/// `(a, b) => ...`, `a => ...`, `async (a) => ...`, `function (a) {}`.
fn function_value(lexemes: &[Lexeme], mut i: usize) -> Option<usize> {
    if matches!(lexemes.get(i).map(|l| &l.token), Some(Token::Ident(w)) if w == "async") {
        i = i.saturating_add(1);
    }
    match &lexemes.get(i)?.token {
        Token::Ident(w) if w == "function" => {
            i = i.saturating_add(1);
            if matches!(lexemes.get(i).map(|l| &l.token), Some(Token::Punct('*'))) {
                i = i.saturating_add(1);
            }
            if matches!(lexemes.get(i).map(|l| &l.token), Some(Token::Ident(_))) {
                i = i.saturating_add(1);
            }
            count_params(lexemes, i).map(|(n, _)| n)
        }
        Token::Ident(_) if lexemes.get(i.saturating_add(1))?.token == Token::Arrow => Some(1),
        Token::Punct('(') => {
            let (n, after) = count_params(lexemes, i)?;
            (lexemes.get(after)?.token == Token::Arrow).then_some(n)
        }
        _ => None,
    }
}

/// The keys of an object literal starting at `open`, each with the parameter
/// count of its function value.
fn object_keys(lexemes: &[Lexeme], open: usize) -> Vec<(String, Option<usize>)> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut i = open;
    // Whether the next token at depth one starts a property.
    let mut fresh = true;
    while let Some(l) = lexemes.get(i) {
        match &l.token {
            Token::Punct('(' | '[' | '{') => {
                depth = depth.saturating_add(1);
                if depth == 1 {
                    fresh = true;
                }
            }
            Token::Punct(')' | ']' | '}') => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            Token::Punct(',') if depth == 1 => fresh = true,
            Token::Ident(_) | Token::Str(_) if depth == 1 && fresh => {
                fresh = false;
                let mut k = i;
                // `async get(...) {}` and `get: async (...) => ...` both start
                // with the word `async`; it is a key only when `:` or `(`
                // follows it directly.
                if matches!(&l.token, Token::Ident(w) if w == "async")
                    && !matches!(lexemes.get(i.saturating_add(1)).map(|l| &l.token), Some(Token::Punct(':' | '(' | ',')))
                {
                    k = k.saturating_add(1);
                }
                let key = match lexemes.get(k).map(|l| &l.token) {
                    Some(Token::Ident(w)) | Some(Token::Str(w)) => w.clone(),
                    _ => continue,
                };
                let count = match lexemes.get(k.saturating_add(1)).map(|l| &l.token) {
                    Some(Token::Punct(':')) => function_value(lexemes, k.saturating_add(2)),
                    // A method: `get(self, key) { ... }`.
                    Some(Token::Punct('(')) => count_params(lexemes, k.saturating_add(1)).map(|(n, _)| n),
                    _ => None,
                };
                out.push((key, count));
            }
            _ => {}
        }
        i = i.saturating_add(1);
    }
    out
}

/// The index of the token that ends the statement starting at `i`: the `;`
/// at depth zero, or the last token before the next line's statement.
fn statement_end(src: &str, lexemes: &[Lexeme], i: usize) -> usize {
    let mut depth = 0usize;
    let mut j = i;
    while let Some(l) = lexemes.get(j) {
        match &l.token {
            Token::Punct('(' | '[' | '{') => depth = depth.saturating_add(1),
            Token::Punct(')' | ']' | '}') => depth = depth.saturating_sub(1),
            Token::Punct(';') if depth == 0 => return j,
            _ => {}
        }
        // A statement with no `;` ends at a line break before a token that
        // cannot continue it, which for an import is its source string.
        if depth == 0 && matches!(l.token, Token::Str(_)) {
            let next = lexemes.get(j.saturating_add(1));
            let breaks = next.is_none_or(|n| src.get(l.end..n.start).is_some_and(|gap| gap.contains('\n')));
            let continues = matches!(next.map(|n| &n.token), Some(Token::Punct(';')));
            if breaks && !continues {
                return j;
            }
        }
        j = j.saturating_add(1);
    }
    lexemes.len().saturating_sub(1)
}

/// `{ a, b as c }` from `open`: each `(imported, local)` pair.
fn named_list(lexemes: &[Lexeme], open: usize) -> (Vec<(String, String)>, usize) {
    let mut out = Vec::new();
    let mut i = open.saturating_add(1);
    while let Some(l) = lexemes.get(i) {
        match &l.token {
            Token::Punct('}') => return (out, i),
            Token::Ident(first) | Token::Str(first) => {
                let renamed = matches!(lexemes.get(i.saturating_add(1)).map(|l| &l.token), Some(Token::Ident(w)) if w == "as");
                if renamed {
                    if let Some(Token::Ident(second) | Token::Str(second)) = lexemes.get(i.saturating_add(2)).map(|l| &l.token) {
                        out.push((first.clone(), second.clone()));
                    }
                    i = i.saturating_add(3);
                    continue;
                }
                out.push((first.clone(), first.clone()));
            }
            _ => {}
        }
        i = i.saturating_add(1);
    }
    (out, i)
}

/// Reads a `js` file's top level.
pub fn read(src: &str) -> Exports {
    let lexemes = lex(src);
    let mut found = Exports::default();
    let mut depth = 0usize;
    let mut i = 0usize;
    while let Some(l) = lexemes.get(i) {
        match &l.token {
            Token::Punct('(' | '[' | '{') => depth = depth.saturating_add(1),
            Token::Punct(')' | ']' | '}') => depth = depth.saturating_sub(1),
            Token::Ident(w) if depth == 0 && w == "await" => found.waits = true,
            Token::Ident(w) if depth == 0 && w == "import" => {
                // `import(...)` and `import.meta` are expressions.
                if matches!(lexemes.get(i.saturating_add(1)).map(|l| &l.token), Some(Token::Punct('(' | '.'))) {
                    i = i.saturating_add(1);
                    continue;
                }
                let end = statement_end(src, &lexemes, i);
                let last = lexemes.get(end).map_or(src.len(), |l| l.end);
                let text = src.get(l.start..last).unwrap_or_default().to_string();
                let names = |module: &str| {
                    lexemes
                        .get(i..=end)
                        .unwrap_or_default()
                        .iter()
                        .any(|x| x.token == Token::Str(String::from(module)))
                };
                let with = if names("buri:program") {
                    program_import(&lexemes, i, end, crate::compiler::backend::js::crossing::HOSTED_PROGRAM)
                } else if names("buri:ui") {
                    found.ui = true;
                    program_import(&lexemes, i, end, crate::compiler::backend::js::crossing::HOSTED_UI)
                } else {
                    found.imports.push(text);
                    String::new()
                };
                found.edits.push(Edit::Replace { start: l.start, end: last, with });
                i = end.saturating_add(1);
                continue;
            }
            Token::Ident(w) if depth == 0 && w == "export" => {
                i = export(src, &lexemes, i, &mut found);
                continue;
            }
            _ => {}
        }
        i = i.saturating_add(1);
    }
    found
}

/// `import { fetch, other as mine } from "buri:program";` as the statement
/// that takes the same names out of the artifact's binding, `program`:
/// `$buri$program` for `buri:program` and `$buri$ui` for `buri:ui`.
fn program_import(lexemes: &[Lexeme], start: usize, end: usize, program: &str) -> String {
    let mut i = start.saturating_add(1);
    let mut parts: Vec<String> = Vec::new();
    while i <= end {
        match lexemes.get(i).map(|l| &l.token) {
            Some(Token::Punct('{')) => {
                let (names, close) = named_list(lexemes, i);
                let fields: Vec<String> = names
                    .iter()
                    .map(|(a, b)| if a == b { a.clone() } else { format!("{a}: {b}") })
                    .collect();
                parts.push(format!("const {{ {} }} = {program};", fields.join(", ")));
                i = close.saturating_add(1);
            }
            Some(Token::Punct('*')) => {
                if let Some(Token::Ident(name)) = lexemes.get(i.saturating_add(2)).map(|l| &l.token) {
                    parts.push(format!("const {name} = {program};"));
                }
                i = i.saturating_add(3);
            }
            Some(Token::Ident(w)) if w != "from" && w != "as" => {
                parts.push(format!("const {w} = {program}.default;"));
                i = i.saturating_add(1);
            }
            _ => i = i.saturating_add(1),
        }
    }
    parts.join(" ")
}

/// One `export` statement starting at `i`; answers where reading resumes.
fn export(src: &str, lexemes: &[Lexeme], i: usize, found: &mut Exports) -> usize {
    let Some(at) = lexemes.get(i) else { return i.saturating_add(1) };
    let word = |k: usize| match lexemes.get(k).map(|l| &l.token) {
        Some(Token::Ident(w)) => Some(w.clone()),
        _ => None,
    };
    let strip = |found: &mut Exports, upto: usize, with: &str| {
        let end = lexemes.get(upto).map_or(at.end, |l| l.start);
        found.edits.push(Edit::Replace { start: at.start, end, with: with.to_string() });
    };
    match word(i.saturating_add(1)).as_deref() {
        Some("default") => {
            let shape = match lexemes.get(i.saturating_add(2)).map(|l| &l.token) {
                Some(Token::Punct('{')) => Shape::Object(object_keys(lexemes, i.saturating_add(2))),
                _ => Shape::Other,
            };
            strip(found, i.saturating_add(2), "const $buri$default = ");
            found.exports.push(Export {
                name: String::from("default"),
                local: String::from("$buri$default"),
                shape,
                at: at.start,
            });
            i.saturating_add(2)
        }
        Some("const" | "let" | "var") => {
            let Some(name) = word(i.saturating_add(2)) else { return i.saturating_add(1) };
            let assigned = lexemes.get(i.saturating_add(3)).map(|l| &l.token) == Some(&Token::Punct('='));
            let shape = match (assigned, lexemes.get(i.saturating_add(4)).map(|l| &l.token)) {
                (true, Some(Token::Punct('{'))) => Shape::Object(object_keys(lexemes, i.saturating_add(4))),
                (true, _) => function_value(lexemes, i.saturating_add(4)).map_or(Shape::Other, Shape::Function),
                _ => Shape::Other,
            };
            strip(found, i.saturating_add(1), "");
            found.exports.push(Export { name: name.clone(), local: name, shape, at: at.start });
            i.saturating_add(2)
        }
        Some("function" | "async" | "class") => {
            let mut k = i.saturating_add(1);
            if word(k).as_deref() == Some("async") {
                k = k.saturating_add(1);
            }
            let is_class = word(k).as_deref() == Some("class");
            k = k.saturating_add(1);
            if lexemes.get(k).map(|l| &l.token) == Some(&Token::Punct('*')) {
                k = k.saturating_add(1);
            }
            let Some(name) = word(k) else { return i.saturating_add(1) };
            let shape = if is_class {
                Shape::Other
            } else {
                count_params(lexemes, k.saturating_add(1)).map_or(Shape::Other, |(n, _)| Shape::Function(n))
            };
            strip(found, i.saturating_add(1), "");
            found.exports.push(Export { name: name.clone(), local: name, shape, at: at.start });
            i.saturating_add(2)
        }
        _ if lexemes.get(i.saturating_add(1)).map(|l| &l.token) == Some(&Token::Punct('{')) => {
            let (names, close) = named_list(lexemes, i.saturating_add(1));
            let end = statement_end(src, lexemes, close);
            let last = lexemes.get(end).map_or(src.len(), |l| l.end);
            for (local, name) in names {
                found.exports.push(Export { name, local, shape: Shape::Other, at: at.start });
            }
            found.edits.push(Edit::Replace { start: at.start, end: last, with: String::new() });
            end.saturating_add(1)
        }
        _ => i.saturating_add(1),
    }
}

/// One method a `js` file has to implement.
#[derive(Clone, Debug)]
pub struct Method {
    pub name: String,
    /// `self` included.
    pub params: usize,
    /// The effect that declares it.
    pub effect: String,
}

/// One production struct a `js` file has to implement.
#[derive(Clone, Debug)]
pub struct Needed {
    pub name: String,
    pub methods: Vec<Method>,
}

/// One gap: where in the file, and the sentence after the file's name.
#[derive(Clone, Debug)]
pub struct Gap {
    pub at: usize,
    pub gap: String,
    pub note: Option<String>,
}

/// Every method `needed` names that `exports` lacks, or exports with the
/// wrong number of parameters.
pub fn gaps(exports: &Exports, needed: &[Needed]) -> Vec<Gap> {
    let mut out = Vec::new();
    for s in needed {
        let Some(export) = exports.get(&s.name) else {
            let effects: Vec<String> = {
                let mut e: Vec<String> = s.methods.iter().map(|m| format!("`{}`", m.effect)).collect();
                e.dedup();
                e
            };
            out.push(Gap {
                at: 0,
                gap: format!("does not export `{}`, which implements {}", s.name, effects.join(" and ")),
                note: None,
            });
            continue;
        };
        match &export.shape {
            Shape::Object(keys) => {
                for m in &s.methods {
                    match keys.iter().find(|(k, _)| *k == m.name) {
                        None => out.push(Gap {
                            at: export.at,
                            gap: format!(
                                "exports `{}` without `{}`, which `{}` declares with {} parameters",
                                s.name, m.name, m.effect, m.params
                            ),
                            note: None,
                        }),
                        Some((_, Some(n))) if *n != m.params => out.push(Gap {
                            at: export.at,
                            gap: format!(
                                "exports `{}` whose `{}` takes {n} parameters, where `{}` declares {}",
                                s.name, m.name, m.effect, m.params
                            ),
                            note: Some(String::from("`self` is the first parameter, as in Buri")),
                        }),
                        Some(_) => {}
                    }
                }
            }
            _ => out.push(Gap {
                at: export.at,
                gap: format!("exports `{}` in a shape the build cannot read", s.name),
                note: Some(format!(
                    "write it `export const {} = {{ {}: (self, ...) => ... }}`, an object with one \
                     function per method",
                    s.name,
                    s.methods.first().map_or("method", |m| m.name.as_str())
                )),
            }),
        }
    }
    out
}

/// The module the build writes for an entry with a `js` file: the compiled
/// program, then the file, with the file's exports the module's own.
///
/// The two halves keep separate scopes. The program is the module's top level,
/// as it is for every other artifact, so a name it reaches for — `fetch`,
/// `Response` — is still the global one. The file runs inside a function
/// below it, so its own names cannot shadow those, and it reaches the program
/// through `$buri$program` where it wrote `import ... from "buri:program"`.
/// Each export of the file becomes the module's, except a production struct's,
/// which the program reaches through `$buri$host` instead.
pub fn bundle(program: &str, file: &str, exports: &Exports, structs: &[String]) -> String {
    let mut body = String::new();
    let mut cursor = 0usize;
    let mut edits = exports.edits.clone();
    edits.sort_by_key(|Edit::Replace { start, .. }| *start);
    for Edit::Replace { start, end, with } in &edits {
        if *start < cursor {
            continue;
        }
        body.push_str(file.get(cursor..*start).unwrap_or_default());
        body.push_str(with);
        cursor = *end;
    }
    body.push_str(file.get(cursor..).unwrap_or_default());

    let host = crate::compiler::backend::js::crossing::HOST_LOOKUP;
    let implemented: Vec<String> = structs
        .iter()
        .filter_map(|s| exports.get(s).map(|e| format!("{s}: {}", e.local)))
        .collect();
    let own: Vec<&Export> = exports.exports.iter().filter(|e| !structs.contains(&e.name)).collect();
    let returned: Vec<String> =
        exports.exports.iter().map(|e| format!("{}: {}", quoted_key(&e.name), e.local)).collect();

    let mut out = String::new();
    for import in &exports.imports {
        out.push_str(import);
        out.push('\n');
    }
    out.push_str(program);
    if !program.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!("var {host};\n"));
    let (open, close) = if exports.waits {
        ("const $buri$platform = await (async () => {\n", "})();\n")
    } else {
        ("const $buri$platform = (() => {\n", "})();\n")
    };
    out.push_str(open);
    out.push_str(&format!("{host} = () => ({{ {} }});\n", implemented.join(", ")));
    out.push_str(&body);
    if !body.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!("return {{ {} }};\n", returned.join(", ")));
    out.push_str(close);
    for (n, e) in own.iter().enumerate() {
        if e.name == "default" {
            out.push_str("export default $buri$platform.default;\n");
        } else {
            out.push_str(&format!(
                "const $buri$e{n} = $buri$platform[{}];\nexport {{ $buri$e{n} as {} }};\n",
                js_string(&e.name),
                e.name
            ));
        }
    }
    out
}

fn quoted_key(name: &str) -> String {
    js_string(name)
}

fn js_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FETCH: &str = r#"// platform/cloudflare_worker/fetch.mjs
import { fetch } from "buri:program";
let bindings = {};

export default { fetch(request, env) { bindings = env; return fetch(request); } };
export const HostKv = {
  get: async (self, namespace, key) => (await bindings[namespace].get(key)) ?? undefined,
  put: (self, namespace, key, value) => bindings[namespace].put(key, value),
};
"#;

    #[test]
    fn the_walkthrough_s_file_is_read() {
        let read = read(FETCH);
        let names: Vec<&str> = read.exports.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["default", "HostKv"]);
        assert_eq!(
            read.get("HostKv").map(|e| e.shape.clone()),
            Some(Shape::Object(vec![(String::from("get"), Some(3)), (String::from("put"), Some(4))]))
        );
        assert!(!read.waits);
        assert!(read.imports.is_empty());
    }

    #[test]
    fn a_missing_method_is_a_gap_and_a_wrong_count_is_another() {
        let read = read(FETCH);
        let needed = [Needed {
            name: String::from("HostKv"),
            methods: vec![
                Method { name: String::from("get"), params: 3, effect: String::from("Kv") },
                Method { name: String::from("put"), params: 3, effect: String::from("Kv") },
                Method { name: String::from("delete"), params: 3, effect: String::from("Kv") },
            ],
        }];
        let found: Vec<String> = gaps(&read, &needed).into_iter().map(|g| g.gap).collect();
        assert_eq!(
            found,
            [
                "exports `HostKv` whose `put` takes 4 parameters, where `Kv` declares 3",
                "exports `HostKv` without `delete`, which `Kv` declares with 3 parameters",
            ]
        );
    }

    #[test]
    fn every_shape_of_function_is_counted() {
        let read = read(
            "export const S = { a(self) {}, async b(self, x) {}, c: function (self, x, y) {}, d: self => 1, \
             e: async function named(self, { x, y }, [z]) {}, f: (self, x = (1, 2)) => x };\n\
             export function free(a, b,) {}\nexport async function later() {}\n",
        );
        assert_eq!(
            read.get("S").map(|e| e.shape.clone()),
            Some(Shape::Object(vec![
                (String::from("a"), Some(1)),
                (String::from("b"), Some(2)),
                (String::from("c"), Some(3)),
                (String::from("d"), Some(1)),
                (String::from("e"), Some(3)),
                (String::from("f"), Some(2)),
            ]))
        );
        assert_eq!(read.get("free").map(|e| e.shape.clone()), Some(Shape::Function(2)));
        assert_eq!(read.get("later").map(|e| e.shape.clone()), Some(Shape::Function(0)));
    }

    #[test]
    fn another_shape_is_a_gap_with_a_note() {
        let read = read("const made = make();\nexport { made as HostKv };\n");
        let needed = [Needed {
            name: String::from("HostKv"),
            methods: vec![Method { name: String::from("get"), params: 3, effect: String::from("Kv") }],
        }];
        let found = gaps(&read, &needed);
        assert_eq!(found.len(), 1);
        assert!(found[0].gap.contains("a shape the build cannot read"), "{:?}", found[0]);
        assert!(found[0].note.is_some());
    }

    #[test]
    fn the_bundle_keeps_the_file_s_scope_its_own() {
        let read = read(FETCH);
        let out = bundle("function main(){}\n", FETCH, &read, &[String::from("HostKv")]);
        assert!(out.contains("const { fetch } = $buri$program;"), "{out}");
        assert!(out.contains("$buri$host = () => ({ HostKv: HostKv });"), "{out}");
        assert!(out.contains("const $buri$default = { fetch(request, env)"), "{out}");
        assert!(out.contains("export default $buri$platform.default;"), "{out}");
        assert!(!out.contains("export const HostKv"), "{out}");
        assert!(!out.contains("from \"buri:program\""), "{out}");
    }

    #[test]
    fn an_import_moves_to_the_top_and_a_regex_is_not_a_comment() {
        let src = "import { connect } from \"cloudflare:sockets\"\nconst r = /a\\/b/g;\nexport const n = (a) => a;\n";
        let read = read(src);
        assert_eq!(read.imports, ["import { connect } from \"cloudflare:sockets\""]);
        assert_eq!(read.get("n").map(|e| e.shape.clone()), Some(Shape::Function(1)));
    }
}
