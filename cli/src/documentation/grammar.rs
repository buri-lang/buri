//! `grammar.ebnf`, read — and `editors/tree-sitter-buri/grammar.js`, written.
//!
//! The normative grammar used to be transliterated by hand into a tree-sitter
//! grammar, which made two files that say the same thing and nothing that
//! noticed when they stopped agreeing. This reads the EBNF and emits the
//! tree-sitter grammar from it, so there is one declarative source and the
//! editor artifact is a build product with a test holding it in place
//! (`cli/tests/corpus.rs::the_tree_sitter_grammar_is_generated`).
//!
//! ## What the EBNF has to carry beyond a context-free grammar
//!
//! A CFG does not say what a syntax tree should look like, and tree-sitter
//! needs that: node names, which rules are hidden, what a capture is called,
//! which terminals the external scanner produces, and — because it is a GLR
//! parser rather than a recursive-descent one — the precedence and
//! associativity of every operator. All of it is written in the EBNF as
//! comments beginning with `@`, so the file stays readable as EBNF and
//! `buri docs grammar` serves it unchanged.
//!
//! ```text
//! (* @hidden *)
//! Type            ::= FnType | PrimaryType
//!
//! FnDecl          ::= "fn" name=IDENT GenericParams? "(" Params? ")"
//!                     ":" return_type=Type (body=Block | ";")
//! ```
//!
//! The directives are documented in the EBNF's own header, which is the one
//! place a reader looks. This module is their implementation.
//!
//! ## What is derived rather than declared
//!
//! Everything that can be. A node's name comes from the production's name and
//! the `@words` table (`FnDecl` is `function_declaration`); an operator's
//! precedence number comes from its position in the `@cascade` list; its
//! associativity comes from the shape of the production (`X ::= X op Y | Y` is
//! left, `X ::= Y op X | Y` is right, `X ::= Y op Y | Y` is neither); and a
//! token's pattern is compiled out of the lexical grammar's own productions.
//! Nothing that the EBNF already says is said twice.

use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// The EBNF, as a tree
// ---------------------------------------------------------------------------

/// One term of a production's right-hand side.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// A literal token, written `"..."`.
    Lit(String),
    /// A production or token class named on the right-hand side.
    Ref(String),
    /// `"a".."z"`, legal only inside the lexical grammar.
    Range(char, char),
    Seq(Vec<Node>),
    Choice(Vec<Node>),
    Opt(Box<Node>),
    Star(Box<Node>),
    Plus(Box<Node>),
    /// `name=X` — the capture name a syntax tree gives `X`.
    Field(String, Box<Node>),
}

/// How the generator is told to treat a production. Everything here is
/// information a context-free grammar cannot carry; see the EBNF's header.
#[derive(Debug, Default, Clone)]
pub struct Annotations {
    /// `@node` — the tree-sitter node name, when the derived one is wrong.
    pub node: Option<String>,
    /// `@hidden` — the node is inlined into its parent and never appears.
    pub hidden: bool,
    /// `@inline` — no rule at all; the body is substituted at each use.
    pub inline: bool,
    /// `@external` — the terminal comes from `src/scanner.c`.
    pub external: bool,
    /// `@token` — one lexical token, compiled to a single pattern.
    pub token: bool,
    /// `@regex` — the pattern, where the lexical grammar states it in prose.
    pub regex: Option<String>,
    /// `@prose` — the body is documentation and is not read as grammar.
    pub prose: bool,
    /// `@as` — represented in the syntax tree by another production's rule.
    pub same_as: Option<String>,
    /// `@prec` — a disambiguation tree-sitter needs and LR(1) does not.
    pub prec: Option<Prec>,
    /// `@raw` — the escape hatch: this rule's body, in tree-sitter's own DSL.
    pub raw: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Prec {
    None(i32),
    Left(Option<i32>),
    Right(Option<i32>),
}

#[derive(Debug, Clone)]
pub struct Production {
    pub name: String,
    pub body: Node,
    pub ann: Annotations,
    /// For a diagnostic that can say where.
    pub line: usize,
}

/// One level of the operator cascade: a production, and the node its
/// alternatives fold into (several levels may share one).
#[derive(Debug, Clone)]
pub struct Level {
    pub production: String,
    pub fold: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Cascade {
    /// The rule every operand of every level refers to.
    pub operand: String,
    pub levels: Vec<Level>,
}

#[derive(Debug, Default)]
pub struct Ebnf {
    pub productions: Vec<Production>,
    /// `@grammar` — tree-sitter's name for this language.
    pub grammar_name: String,
    /// `@word` — the token keyword extraction works against.
    pub word: Option<String>,
    /// `@extras` — what may appear between any two tokens.
    pub extras: Vec<String>,
    /// `@externals` — in the order `src/scanner.c` enumerates them.
    pub externals: Vec<String>,
    /// `@words` — how a production's name becomes a node's name.
    pub words: Vec<(String, String)>,
    /// `@cascade` — the precedence ladder.
    pub cascade: Option<Cascade>,
    /// `@conflicts` — GLR conflicts, if the grammar ever needs one.
    pub conflicts: Vec<Vec<String>>,
}

impl Ebnf {
    pub fn find(&self, name: &str) -> Option<&Production> {
        self.productions.iter().find(|p| p.name == name)
    }
}

// ---------------------------------------------------------------------------
// Lexing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    /// `::=`
    Define,
    Bar,
    LParen,
    RParen,
    Question,
    Star,
    Plus,
    Eq,
    DotDot,
    Comment(String),
    /// A character that means nothing to this notation. It is legal only in a
    /// body the directives have already declared to be prose.
    Other(char),
}

struct Lexed {
    toks: Vec<Tok>,
    lines: Vec<usize>,
}

fn lex(src: &str) -> Result<Lexed, String> {
    let b: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut line = 1;
    let mut toks = Vec::new();
    let mut lines = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let at = line;
        // A comment, nestable so that a directive may quote one.
        if c == '(' && i + 1 < b.len() && b[i + 1] == '*' {
            let mut depth = 0;
            let start = i;
            while i < b.len() {
                if b[i] == '(' && i + 1 < b.len() && b[i + 1] == '*' {
                    depth += 1;
                    i += 2;
                } else if b[i] == '*' && i + 1 < b.len() && b[i + 1] == ')' {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    if b[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
            }
            if depth != 0 {
                return Err(format!("line {at}: a comment is never closed"));
            }
            let text: String = b[start + 2..i.saturating_sub(2)].iter().collect();
            toks.push(Tok::Comment(text));
            lines.push(at);
            continue;
        }
        let tok = if c == ':' && b.get(i + 1) == Some(&':') && b.get(i + 2) == Some(&'=') {
            i += 3;
            Tok::Define
        } else if c == '.' && b.get(i + 1) == Some(&'.') {
            i += 2;
            Tok::DotDot
        } else if c == '"' || c == '\'' {
            let quote = c;
            i += 1;
            let mut s = String::new();
            loop {
                let Some(&ch) = b.get(i) else {
                    return Err(format!("line {at}: a terminal is never closed"));
                };
                if ch == quote {
                    i += 1;
                    break;
                }
                if ch == '\\' {
                    let Some(&esc) = b.get(i + 1) else {
                        return Err(format!("line {at}: a terminal ends in a backslash"));
                    };
                    s.push(match esc {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        '0' => '\0',
                        other => other,
                    });
                    i += 2;
                    continue;
                }
                s.push(ch);
                i += 1;
            }
            Tok::Str(s)
        } else if c.is_alphanumeric() || c == '_' {
            let start = i;
            while i < b.len() && (b[i].is_alphanumeric() || b[i] == '_') {
                i += 1;
            }
            Tok::Ident(b[start..i].iter().collect())
        } else {
            i += 1;
            match c {
                '|' => Tok::Bar,
                '(' => Tok::LParen,
                ')' => Tok::RParen,
                '?' => Tok::Question,
                '*' => Tok::Star,
                '+' => Tok::Plus,
                '=' => Tok::Eq,
                other => Tok::Other(other),
            }
        };
        toks.push(tok);
        lines.push(at);
    }
    Ok(Lexed { toks, lines })
}

// ---------------------------------------------------------------------------
// Directives
// ---------------------------------------------------------------------------

/// The directives a comment carries.
///
/// A directive begins at an `@` that opens a line: the rest of that line may
/// hold several (`@node integer_literal @token`), and the lines below it
/// continue the last one until a blank line ends it. Prose is anything else,
/// which is why the header's list of the directives writes each in backticks
/// — a line that opens with an `@` is a directive, not a description of one.
fn directives(comment: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut open: Option<(String, String)> = None;
    for raw in comment.lines() {
        let line = raw.trim();
        if !line.starts_with('@') {
            match (line.is_empty(), open.as_mut()) {
                (true, _) => {
                    if let Some(d) = open.take() {
                        out.push(d);
                    }
                }
                (false, Some((_, value))) => {
                    if !value.is_empty() {
                        value.push(' ');
                    }
                    value.push_str(line);
                }
                (false, None) => {}
            }
            continue;
        }
        if let Some(d) = open.take() {
            out.push(d);
        }
        let mut found: Vec<(String, String)> = Vec::new();
        let mut rest = line;
        while let Some(after) = rest.strip_prefix('@') {
            let (name, tail) = match after.find(char::is_whitespace) {
                Some(k) => (&after[..k], after[k..].trim_start()),
                None => (after, ""),
            };
            // A pattern is taken whole: it is one word to a reader, and
            // splitting it on `@` would be a surprise waiting to happen.
            if name == "regex" || name == "raw" {
                found.push((name.to_string(), tail.trim_end().to_string()));
                break;
            }
            let mut value = String::new();
            let mut cursor = tail;
            loop {
                let word = cursor.trim_start();
                if word.is_empty() || word.starts_with('@') {
                    cursor = word;
                    break;
                }
                let k = word.find(char::is_whitespace).unwrap_or(word.len());
                if !value.is_empty() {
                    value.push(' ');
                }
                value.push_str(&word[..k]);
                cursor = &word[k..];
            }
            found.push((name.to_string(), value));
            rest = cursor;
        }
        open = found.pop();
        out.extend(found);
    }
    if let Some(d) = open {
        out.push(d);
    }
    out
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

pub fn parse(src: &str) -> Result<Ebnf, String> {
    let Lexed { toks, lines } = lex(src)?;
    let mut out = Ebnf { grammar_name: String::new(), ..Default::default() };
    let mut pending = Annotations::default();
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            Tok::Comment(text) => {
                take_directives(text, &mut out, &mut pending, lines[i])?;
                i += 1;
            }
            Tok::Ident(name) if toks.get(i + 1) == Some(&Tok::Define) => {
                let line = lines[i];
                let name = name.clone();
                let start = i + 2;
                let mut end = start;
                while end < toks.len() {
                    if matches!(&toks[end], Tok::Ident(_)) && toks.get(end + 1) == Some(&Tok::Define)
                    {
                        break;
                    }
                    end += 1;
                }
                let ann = std::mem::take(&mut pending);
                // A comment inside the body belongs to whatever comes next.
                let mut body_toks = Vec::new();
                let mut trailing = Vec::new();
                for k in start..end {
                    match &toks[k] {
                        Tok::Comment(text) => trailing.push((text.clone(), lines[k])),
                        t => body_toks.push(t.clone()),
                    }
                }
                let body = if ann.prose || ann.regex.is_some() || ann.raw.is_some() {
                    // The body is documentation: `any character except "\n"`
                    // is prose, and reading it as grammar would invent three
                    // non-terminals that do not exist.
                    Node::Seq(Vec::new())
                } else {
                    let mut p = Body { toks: &body_toks, i: 0, line };
                    let node = p.alternation()?;
                    if p.i != body_toks.len() {
                        return Err(format!("line {line}: `{name}` has trailing input"));
                    }
                    node
                };
                out.productions.push(Production { name, body, ann, line });
                for (text, at) in trailing {
                    take_directives(&text, &mut out, &mut pending, at)?;
                }
                i = end;
            }
            other => {
                return Err(format!("line {}: expected a production, found {other:?}", lines[i]))
            }
        }
    }
    if out.grammar_name.is_empty() {
        return Err("the grammar declares no `@grammar` name".into());
    }
    Ok(out)
}

fn take_directives(
    text: &str,
    out: &mut Ebnf,
    pending: &mut Annotations,
    line: usize,
) -> Result<(), String> {
    for (name, value) in directives(text) {
        let words = || value.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        match name.as_str() {
            "grammar" => out.grammar_name = value.trim().to_string(),
            "word" => out.word = Some(value.trim().to_string()),
            "extras" => out.extras = words(),
            "externals" => out.externals = words(),
            "conflicts" => out.conflicts.push(words()),
            "words" => {
                for pair in value.split_whitespace() {
                    let (k, v) = pair
                        .split_once('=')
                        .ok_or_else(|| format!("line {line}: `@words {pair}` is not `Camel=snake`"))?;
                    out.words.push((k.to_string(), v.to_string()));
                }
            }
            "cascade" => out.cascade = Some(parse_cascade(&value, line)?),
            "node" => pending.node = Some(value.trim().to_string()),
            "hidden" => pending.hidden = true,
            "inline" => pending.inline = true,
            "external" => pending.external = true,
            "token" => pending.token = true,
            "prose" => pending.prose = true,
            "regex" => pending.regex = Some(value.trim().to_string()),
            "as" => pending.same_as = Some(value.trim().to_string()),
            "raw" => pending.raw = Some(value.trim().to_string()),
            "prec" => pending.prec = Some(parse_prec(&value, line)?),
            other => return Err(format!("line {line}: `@{other}` is not a directive")),
        }
    }
    Ok(())
}

fn parse_prec(value: &str, line: usize) -> Result<Prec, String> {
    let parts: Vec<&str> = value.split_whitespace().collect();
    let number = |s: &str| s.parse::<i32>().map_err(|_| format!("line {line}: `{s}` is not a level"));
    match parts.as_slice() {
        ["left"] => Ok(Prec::Left(None)),
        ["right"] => Ok(Prec::Right(None)),
        ["left", n] => Ok(Prec::Left(Some(number(n)?))),
        ["right", n] => Ok(Prec::Right(Some(number(n)?))),
        [n] => Ok(Prec::None(number(n)?)),
        _ => Err(format!("line {line}: `@prec {value}` is not a precedence")),
    }
}

/// ```text
/// @cascade _operand
///   OrExpr AndExpr ... = binary_expression
///   UnaryExpr          = unary_expression
///   PostfixExpr
/// ```
fn parse_cascade(value: &str, line: usize) -> Result<Cascade, String> {
    let mut words = value.split_whitespace();
    let operand = words
        .next()
        .ok_or_else(|| format!("line {line}: `@cascade` names no operand rule"))?
        .to_string();
    let mut cascade = Cascade { operand, levels: Vec::new() };
    let mut group: Vec<String> = Vec::new();
    let mut expect_fold = false;
    for w in words {
        if expect_fold {
            for production in group.drain(..) {
                cascade.levels.push(Level { production, fold: Some(w.to_string()) });
            }
            expect_fold = false;
        } else if w == "=" {
            if group.is_empty() {
                return Err(format!("line {line}: `@cascade` has an `=` with no levels before it"));
            }
            expect_fold = true;
        } else {
            group.push(w.to_string());
        }
    }
    if expect_fold {
        return Err(format!("line {line}: `@cascade` ends in an `=`"));
    }
    for production in group {
        cascade.levels.push(Level { production, fold: None });
    }
    Ok(cascade)
}

struct Body<'a> {
    toks: &'a [Tok],
    i: usize,
    line: usize,
}

impl Body<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.i)
    }

    fn alternation(&mut self) -> Result<Node, String> {
        let mut alts = vec![self.sequence()?];
        while self.peek() == Some(&Tok::Bar) {
            self.i += 1;
            alts.push(self.sequence()?);
        }
        Ok(if alts.len() == 1 { alts.pop().unwrap() } else { Node::Choice(alts) })
    }

    fn sequence(&mut self) -> Result<Node, String> {
        let mut items = Vec::new();
        while matches!(self.peek(), Some(Tok::Ident(_) | Tok::Str(_) | Tok::LParen)) {
            items.push(self.labelled()?);
        }
        if items.is_empty() {
            return Err(format!("line {}: an alternative is empty", self.line));
        }
        Ok(if items.len() == 1 { items.pop().unwrap() } else { Node::Seq(items) })
    }

    fn labelled(&mut self) -> Result<Node, String> {
        if let (Some(Tok::Ident(label)), Some(Tok::Eq)) = (self.peek(), self.toks.get(self.i + 1)) {
            let label = label.clone();
            self.i += 2;
            let inner = self.postfix()?;
            return Ok(Node::Field(label, Box::new(inner)));
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<Node, String> {
        let mut node = self.atom()?;
        loop {
            node = match self.peek() {
                Some(Tok::Question) => Node::Opt(Box::new(node)),
                Some(Tok::Star) => Node::Star(Box::new(node)),
                Some(Tok::Plus) => Node::Plus(Box::new(node)),
                _ => break,
            };
            self.i += 1;
        }
        Ok(node)
    }

    fn atom(&mut self) -> Result<Node, String> {
        match self.peek().cloned() {
            Some(Tok::Str(s)) => {
                self.i += 1;
                if self.peek() == Some(&Tok::DotDot) {
                    self.i += 1;
                    let Some(Tok::Str(hi)) = self.peek().cloned() else {
                        return Err(format!("line {}: a range needs a second terminal", self.line));
                    };
                    self.i += 1;
                    let (Some(a), Some(b)) = (one_char(&s), one_char(&hi)) else {
                        return Err(format!("line {}: a range needs single characters", self.line));
                    };
                    return Ok(Node::Range(a, b));
                }
                Ok(Node::Lit(s))
            }
            Some(Tok::Ident(name)) => {
                self.i += 1;
                Ok(Node::Ref(name))
            }
            Some(Tok::LParen) => {
                self.i += 1;
                let inner = self.alternation()?;
                if self.peek() != Some(&Tok::RParen) {
                    return Err(format!("line {}: a group is never closed", self.line));
                }
                self.i += 1;
                Ok(inner)
            }
            other => Err(format!("line {}: expected a term, found {other:?}", self.line)),
        }
    }
}

fn one_char(s: &str) -> Option<char> {
    let mut it = s.chars();
    let c = it.next()?;
    it.next().is_none().then_some(c)
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

/// `ReExport` becomes `re_export`, and `FnDecl` becomes `function_declaration`
/// through the `@words` table. A plural is the singular's expansion with an
/// `s`, so `Params` follows `Param` without being listed.
fn node_name(name: &str, words: &[(String, String)], hidden: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    for c in name.chars() {
        if c.is_uppercase() && !current.is_empty() {
            parts.push(std::mem::take(&mut current));
        }
        current.push(c);
    }
    if !current.is_empty() {
        parts.push(current);
    }
    let lookup = |w: &str| words.iter().find(|(k, _)| k == w).map(|(_, v)| v.clone());
    let expanded: Vec<String> = parts
        .iter()
        .map(|p| {
            if let Some(v) = lookup(p) {
                return v;
            }
            if let Some(stem) = p.strip_suffix('s') {
                if let Some(v) = lookup(stem) {
                    return format!("{v}s");
                }
            }
            p.to_lowercase()
        })
        .collect();
    let joined = expanded.join("_");
    if hidden {
        format!("_{joined}")
    } else {
        joined
    }
}

// ---------------------------------------------------------------------------
// Token patterns
// ---------------------------------------------------------------------------

/// A regular expression, as much of one as the lexical grammar needs.
#[derive(Debug, Clone)]
enum Re {
    /// A run of literal characters.
    Lit(String),
    /// The inside of a `[...]`, already escaped.
    Class(String),
    /// A pattern written out by `@regex`, opaque to everything here.
    Raw(String),
    Seq(Vec<Re>),
    Alt(Vec<Re>),
    Rep(Box<Re>, char),
}

impl Re {
    /// Whether this needs no `(?:...)` around it before a `*`, `+` or `?`.
    fn atomic(&self) -> bool {
        match self {
            Re::Lit(s) => s.chars().count() == 1,
            Re::Class(_) => true,
            Re::Raw(s) => {
                s.chars().count() == 1
                    || (s.starts_with('[') && s.ends_with(']') && !s[1..s.len() - 1].contains(']'))
            }
            _ => false,
        }
    }

    /// The single character this matches, if it matches exactly one.
    fn single(&self) -> Option<String> {
        match self {
            Re::Lit(s) if s.chars().count() == 1 => Some(escape_in_class(s.chars().next().unwrap())),
            Re::Class(c) => Some(c.clone()),
            _ => None,
        }
    }

    fn render(&self, nested: bool) -> String {
        match self {
            Re::Lit(s) => s.chars().map(escape_outside_class).collect(),
            Re::Class(c) => format!("[{c}]"),
            Re::Raw(s) => s.clone(),
            Re::Seq(items) => items
                .iter()
                .map(|it| {
                    if matches!(it, Re::Alt(_)) {
                        format!("(?:{})", it.render(false))
                    } else {
                        it.render(true)
                    }
                })
                .collect(),
            Re::Alt(items) => {
                let inner =
                    items.iter().map(|it| it.render(false)).collect::<Vec<_>>().join("|");
                if nested {
                    format!("(?:{inner})")
                } else {
                    inner
                }
            }
            Re::Rep(inner, op) => {
                if inner.atomic() {
                    format!("{}{op}", inner.render(true))
                } else {
                    format!("(?:{}){op}", inner.render(false))
                }
            }
        }
    }
}

fn escape_outside_class(c: char) -> String {
    match c {
        '\n' => "\\n".into(),
        '\r' => "\\r".into(),
        '\t' => "\\t".into(),
        '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|'
        | '/' => format!("\\{c}"),
        other => other.to_string(),
    }
}

fn escape_in_class(c: char) -> String {
    match c {
        '\n' => "\\n".into(),
        '\r' => "\\r".into(),
        '\t' => "\\t".into(),
        '\\' | ']' | '^' | '-' | '[' | '/' => format!("\\{c}"),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// tree-sitter's DSL, as a tree
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Js {
    /// Already-rendered JavaScript: `$.identifier`, `'fn'`, `/[0-9]+/`.
    Atom(String),
    Call(String, Vec<Js>),
}

impl Js {
    fn render(&self, indent: usize, out: &mut String) {
        let flat = self.flat();
        if indent + flat.len() <= 92 || matches!(self, Js::Atom(_)) {
            out.push_str(&flat);
            return;
        }
        let Js::Call(name, args) = self else { unreachable!() };
        out.push_str(name);
        out.push_str("(\n");
        for (k, arg) in args.iter().enumerate() {
            out.push_str(&" ".repeat(indent + 2));
            arg.render(indent + 2, out);
            if k + 1 < args.len() {
                out.push(',');
            }
            out.push('\n');
        }
        out.push_str(&" ".repeat(indent));
        out.push(')');
    }

    fn flat(&self) -> String {
        match self {
            Js::Atom(s) => s.clone(),
            Js::Call(name, args) => {
                let inner = args.iter().map(Js::flat).collect::<Vec<_>>().join(", ");
                format!("{name}({inner})")
            }
        }
    }
}

fn js_string(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        match c {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out.push('\'');
    out
}

// ---------------------------------------------------------------------------
// The generator
// ---------------------------------------------------------------------------

struct Gen<'a> {
    ebnf: &'a Ebnf,
    /// EBNF name to tree-sitter node name, after `@node`, `@as` and `@hidden`.
    names: HashMap<String, String>,
    /// EBNF name to the level it occupies in the cascade, and its fold node.
    levels: HashMap<String, (usize, Option<String>)>,
    cascade: Cascade,
    /// The precedence a level that keeps its own rule hands to each member.
    /// `field_expression` is at the postfix level because `PostfixExpr` is.
    member_prec: HashMap<String, i32>,
}

/// Read the annotated EBNF and write the tree-sitter grammar.
pub fn generate(source: &str) -> Result<String, String> {
    let ebnf = parse(source)?;
    emit(&ebnf)
}

/// Every non-terminal a production names must be a production. Nothing else
/// in the toolchain executes the EBNF, so this is what stops a rename from
/// leaving a reference behind.
pub fn dangling_references(ebnf: &Ebnf) -> Vec<String> {
    let known: HashSet<&str> = ebnf.productions.iter().map(|p| p.name.as_str()).collect();
    let mut out = Vec::new();
    let note = |from: &str, to: &str, out: &mut Vec<String>| {
        if !known.contains(to) {
            let line = format!("{from} names `{to}`, which is not a production");
            if !out.contains(&line) {
                out.push(line);
            }
        }
    };
    for p in &ebnf.productions {
        let mut refs = Vec::new();
        collect_refs(&p.body, &mut refs);
        for r in refs {
            note(&p.name, &r, &mut out);
        }
        if let Some(other) = &p.ann.same_as {
            note(&p.name, other, &mut out);
        }
    }
    for name in ebnf.extras.iter().chain(ebnf.externals.iter()) {
        note("@extras/@externals", name, &mut out);
    }
    if let Some(word) = &ebnf.word {
        note("@word", word, &mut out);
    }
    if let Some(c) = &ebnf.cascade {
        for level in &c.levels {
            note("@cascade", &level.production, &mut out);
        }
    }
    out
}

fn collect_refs(node: &Node, out: &mut Vec<String>) {
    match node {
        Node::Ref(name) => out.push(name.clone()),
        Node::Seq(items) | Node::Choice(items) => items.iter().for_each(|n| collect_refs(n, out)),
        Node::Opt(n) | Node::Star(n) | Node::Plus(n) | Node::Field(_, n) => collect_refs(n, out),
        Node::Lit(_) | Node::Range(_, _) => {}
    }
}

fn emit(ebnf: &Ebnf) -> Result<String, String> {
    let dangling = dangling_references(ebnf);
    if !dangling.is_empty() {
        return Err(dangling.join("\n"));
    }

    let mut names = HashMap::new();
    for p in &ebnf.productions {
        let name = match (&p.ann.node, &p.ann.same_as) {
            (Some(n), _) => n.clone(),
            (None, Some(_)) => continue,
            (None, None) => node_name(&p.name, &ebnf.words, p.ann.hidden),
        };
        names.insert(p.name.clone(), name);
    }
    // `@as` is resolved after every other name is known, so that it may point
    // at a production declared later.
    for p in &ebnf.productions {
        if let Some(other) = &p.ann.same_as {
            let target = names
                .get(other)
                .ok_or_else(|| format!("`{}` is `@as {other}`, which has no node", p.name))?
                .clone();
            names.insert(p.name.clone(), target);
        }
    }

    let cascade = ebnf.cascade.clone().unwrap_or_default();
    let mut levels = HashMap::new();
    for (k, level) in cascade.levels.iter().enumerate() {
        levels.insert(level.production.clone(), (k + 1, level.fold.clone()));
    }
    // A level that keeps its own rule is a choice of nodes, and every one of
    // them that recurs through the level sits at the level's precedence.
    let mut member_prec = HashMap::new();
    for (k, level) in cascade.levels.iter().enumerate() {
        if level.fold.is_some() {
            continue;
        }
        let p = ebnf.find(&level.production).ok_or_else(|| {
            format!("`@cascade` names `{}`, which is not a production", level.production)
        })?;
        let alts: Vec<&Node> = match &p.body {
            Node::Choice(items) => items.iter().collect(),
            other => vec![other],
        };
        for alt in alts {
            let Node::Ref(name) = alt else { continue };
            let member = ebnf.find(name).ok_or_else(|| format!("`{name}` is not a production"))?;
            let mut refs = Vec::new();
            collect_refs(&member.body, &mut refs);
            if refs.contains(&level.production) {
                member_prec.insert(name.clone(), (k + 1) as i32);
            }
        }
    }

    let g = Gen { ebnf, names, levels, cascade, member_prec };
    g.write()
}

impl Gen<'_> {
    fn prod(&self, name: &str) -> &Production {
        self.ebnf.find(name).expect("checked by dangling_references")
    }

    fn node_of(&self, name: &str) -> &str {
        &self.names[name]
    }

    // -- the operator cascade ----------------------------------------------

    /// What a reference to `name` becomes. A level that folds loses its
    /// identity — every operand in the cascade is the one operand rule.
    fn cascade_target(&self, name: &str) -> Option<String> {
        let (_, fold) = self.levels.get(name)?;
        Some(match fold {
            Some(_) => self.cascade.operand.clone(),
            None => self.node_of(name).to_string(),
        })
    }

    /// The associativity a level's shape states.
    ///
    /// `X ::= X op Y | Y` is left, `X ::= Y op X | Y` is right, and
    /// `X ::= Y op Y | Y` is neither — which tree-sitter has no word for, so
    /// it reads as left and the compiler objects to the chain.
    fn assoc_of(&self, level: &str, next: Option<&str>, alt: &Node) -> Result<Prec, String> {
        let Node::Seq(items) = alt else {
            return Err(format!("`{level}` has an alternative that is not an operator"));
        };
        let head = match &items[0] {
            Node::Ref(n) => Some(n.as_str()),
            _ => None,
        };
        let tail = match items.last() {
            Some(Node::Ref(n)) => Some(n.as_str()),
            _ => None,
        };
        // A prefix operator: the level recurses on its right and nothing else.
        if head.is_none() {
            if tail == Some(level) {
                return Ok(Prec::Right(None));
            }
            return Err(format!("`{level}` opens with an operator but does not recur"));
        }
        match (head == Some(level), tail == Some(level)) {
            (true, false) => Ok(Prec::Left(None)),
            (false, true) => Ok(Prec::Right(None)),
            (false, false) => {
                if head != next || tail != next {
                    return Err(format!("`{level}` is not written over the level below it"));
                }
                Ok(Prec::Left(None))
            }
            (true, true) => Err(format!("`{level}` recurs on both sides; that is ambiguous")),
        }
    }

    /// The rules the cascade contributes, in level order, plus the operand
    /// rule the whole of it hangs from.
    fn cascade_rules(&self) -> Result<Vec<(String, Js)>, String> {
        let mut folds: Vec<(String, Vec<Js>)> = Vec::new();
        let mut own: Vec<(String, Js)> = Vec::new();
        // What `_operand` is a choice of, in the order the levels give it.
        let mut operand: Vec<String> = Vec::new();

        for (k, level) in self.cascade.levels.iter().enumerate() {
            let number = (k + 1) as i32;
            let next = self.cascade.levels.get(k + 1).map(|l| l.production.as_str());
            let p = self.prod(&level.production);
            let alts: Vec<&Node> = match &p.body {
                Node::Choice(items) => items.iter().collect(),
                other => vec![other],
            };
            match &level.fold {
                Some(fold) => {
                    if !operand.contains(fold) {
                        operand.push(fold.clone());
                    }
                    for alt in alts {
                        // A bare reference is the fall-through to the level
                        // below, and the operand rule already covers it.
                        if let Node::Ref(name) = alt {
                            if Some(name.as_str()) != next && self.levels.get(name).is_none() {
                                let target = self.node_of(name).to_string();
                                if !operand.contains(&target) {
                                    operand.push(target);
                                }
                            }
                            continue;
                        }
                        let assoc = self.assoc_of(&level.production, next, alt)?;
                        let body = self.js(alt)?;
                        let call = match assoc {
                            Prec::Left(_) => "prec.left",
                            Prec::Right(_) => "prec.right",
                            Prec::None(_) => "prec",
                        };
                        let wrapped =
                            Js::Call(call.into(), vec![Js::Atom(number.to_string()), body]);
                        match folds.iter_mut().find(|(n, _)| n == fold) {
                            Some((_, list)) => list.push(wrapped),
                            None => folds.push((fold.clone(), vec![wrapped])),
                        }
                    }
                }
                None => {
                    // The level keeps its own rule, and every alternative that
                    // recurs through it is one node at this precedence.
                    let mut choices = Vec::new();
                    for alt in alts {
                        let Node::Ref(name) = alt else {
                            return Err(format!(
                                "`{}` keeps its own rule, so every alternative must name one",
                                level.production
                            ));
                        };
                        let _ = number;
                        choices.push(Js::Atom(format!("$.{}", self.node_of(name))));
                    }
                    let target = self.node_of(&level.production).to_string();
                    own.push((target.clone(), Js::Call("choice".into(), choices)));
                    if !operand.contains(&target) {
                        operand.push(target);
                    }
                }
            }
        }

        let mut out = Vec::new();
        out.push((
            self.cascade.operand.clone(),
            Js::Call(
                "choice".into(),
                operand.iter().map(|n| Js::Atom(format!("$.{n}"))).collect(),
            ),
        ));
        for (name, alts) in folds {
            let body = if alts.len() == 1 {
                alts.into_iter().next().unwrap()
            } else {
                Js::Call("choice".into(), alts)
            };
            out.push((name, body));
        }
        out.extend(own);
        Ok(out)
    }

    // -- ordinary rules -----------------------------------------------------

    fn js(&self, node: &Node) -> Result<Js, String> {
        Ok(match node {
            Node::Lit(s) => Js::Atom(js_string(s)),
            Node::Range(_, _) => return Err("a range is only legal in a token".into()),
            Node::Ref(name) => {
                if let Some(target) = self.cascade_target(name) {
                    return Ok(Js::Atom(format!("$.{target}")));
                }
                let p = self.prod(name);
                if p.ann.inline {
                    return self.rule_body(p);
                }
                Js::Atom(format!("$.{}", self.node_of(name)))
            }
            Node::Seq(items) => {
                let mut js = Vec::new();
                for it in items {
                    js.push(self.js(it)?);
                }
                if js.len() == 1 {
                    js.pop().unwrap()
                } else {
                    Js::Call("seq".into(), js)
                }
            }
            Node::Choice(items) => {
                let mut js = Vec::new();
                for it in items {
                    js.push(self.js(it)?);
                }
                if js.len() == 1 {
                    js.pop().unwrap()
                } else {
                    Js::Call("choice".into(), js)
                }
            }
            Node::Opt(n) => Js::Call("optional".into(), vec![self.js(n)?]),
            Node::Star(n) => Js::Call("repeat".into(), vec![self.js(n)?]),
            Node::Plus(n) => Js::Call("repeat1".into(), vec![self.js(n)?]),
            Node::Field(name, n) => {
                Js::Call("field".into(), vec![Js::Atom(js_string(name)), self.js(n)?])
            }
        })
    }

    fn rule_body(&self, p: &Production) -> Result<Js, String> {
        if let Some(raw) = &p.ann.raw {
            return Ok(Js::Atom(raw.clone()));
        }
        if let Some(re) = &p.ann.regex {
            return Ok(Js::Atom(format!("/{re}/")));
        }
        if p.ann.token {
            return Ok(Js::Atom(format!("/{}/", self.pattern(&p.body)?.render(false))));
        }
        let mut body = self.js(&p.body)?;
        let number = self.member_prec.get(&p.name).copied();
        let prec = match (&p.ann.prec, number) {
            (Some(_), Some(_)) => {
                return Err(format!("`{}` is given a precedence twice", p.name));
            }
            (Some(prec), None) => Some(prec.clone()),
            (None, Some(n)) => Some(Prec::None(n)),
            (None, None) => None,
        };
        if let Some(prec) = prec {
            let (call, number) = match prec {
                Prec::None(n) => ("prec", Some(n)),
                Prec::Left(n) => ("prec.left", n),
                Prec::Right(n) => ("prec.right", n),
            };
            let mut args = Vec::new();
            if let Some(n) = number {
                args.push(Js::Atom(n.to_string()));
            }
            args.push(body);
            body = Js::Call(call.into(), args);
        }
        Ok(body)
    }

    /// The pattern a lexical production compiles to.
    fn pattern(&self, node: &Node) -> Result<Re, String> {
        Ok(match node {
            Node::Lit(s) => Re::Lit(s.clone()),
            Node::Range(a, b) => {
                Re::Class(format!("{}-{}", escape_in_class(*a), escape_in_class(*b)))
            }
            Node::Ref(name) => {
                let p = self.prod(name);
                if let Some(re) = &p.ann.regex {
                    Re::Raw(re.clone())
                } else if p.ann.external {
                    return Err(format!("`{name}` comes from the external scanner"));
                } else {
                    self.pattern(&p.body)?
                }
            }
            Node::Seq(items) => {
                let mut out = Vec::new();
                for it in items {
                    out.push(self.pattern(it)?);
                }
                Re::Seq(out)
            }
            Node::Choice(items) => {
                let mut out = Vec::new();
                for it in items {
                    out.push(self.pattern(it)?);
                }
                // Branches that match one character each become one character
                // class, standing where the first of them stood. `Digit | "_"`
                // is `[0-9_]`, which is what a reader of the generated grammar
                // expects to see and what they would have written by hand.
                let mut class = String::new();
                let mut merged: Vec<Option<Re>> = Vec::new();
                let mut slot = None;
                for re in out {
                    match re.single() {
                        Some(chars) => {
                            class.push_str(&chars);
                            if slot.is_none() {
                                slot = Some(merged.len());
                                merged.push(None);
                            }
                        }
                        None => merged.push(Some(re)),
                    }
                }
                if let Some(k) = slot {
                    merged[k] = Some(Re::Class(class));
                }
                let mut branches: Vec<Re> = merged.into_iter().flatten().collect();
                if branches.len() == 1 {
                    branches.pop().unwrap()
                } else {
                    Re::Alt(branches)
                }
            }
            Node::Opt(n) => Re::Rep(Box::new(self.pattern(n)?), '?'),
            Node::Star(n) => Re::Rep(Box::new(self.pattern(n)?), '*'),
            Node::Plus(n) => Re::Rep(Box::new(self.pattern(n)?), '+'),
            Node::Field(_, n) => self.pattern(n)?,
        })
    }

    // -- the file -----------------------------------------------------------

    /// Which productions become rules: whatever the start symbol reaches,
    /// plus the extras. A lexical helper is reached only from inside a token,
    /// where it is compiled into the pattern rather than named.
    fn reachable(&self) -> Vec<String> {
        let start = &self.ebnf.productions[0].name;
        let mut seen: HashSet<String> = HashSet::new();
        let mut order: Vec<String> = Vec::new();
        let mut queue: Vec<String> = vec![start.clone()];
        queue.extend(self.ebnf.extras.iter().cloned());
        while let Some(name) = queue.pop() {
            if !seen.insert(name.clone()) {
                continue;
            }
            order.push(name.clone());
            let p = self.prod(&name);
            if p.ann.token || p.ann.external || p.ann.regex.is_some() || p.ann.prose {
                continue;
            }
            if let Some(other) = &p.ann.same_as {
                queue.push(other.clone());
                continue;
            }
            let mut refs = Vec::new();
            collect_refs(&p.body, &mut refs);
            queue.extend(refs);
        }
        self.ebnf
            .productions
            .iter()
            .map(|p| p.name.clone())
            .filter(|n| order.contains(n))
            .collect()
    }

    fn write(&self) -> Result<String, String> {
        let cascade_rules = self.cascade_rules()?;
        let reachable = self.reachable();

        let mut out = String::new();
        out.push_str(HEADER);
        out.push_str("module.exports = grammar({\n");
        out.push_str(&format!("  name: {},\n\n", js_string(&self.ebnf.grammar_name)));

        if let Some(word) = &self.ebnf.word {
            out.push_str(&format!("  word: $ => $.{},\n\n", self.node_of(word)));
        }

        if !self.ebnf.extras.is_empty() {
            let mut parts = Vec::new();
            for name in &self.ebnf.extras {
                let p = self.prod(name);
                if p.ann.inline {
                    parts.push(self.rule_body(p)?.flat());
                } else {
                    parts.push(format!("$.{}", self.node_of(name)));
                }
            }
            out.push_str(&format!("  extras: $ => [{}],\n\n", parts.join(", ")));
        }

        if !self.ebnf.externals.is_empty() {
            out.push_str("  externals: $ => [\n");
            for name in &self.ebnf.externals {
                out.push_str(&format!("    $.{},\n", self.node_of(name)));
            }
            out.push_str("  ],\n\n");
        }

        if !self.ebnf.conflicts.is_empty() {
            out.push_str("  conflicts: $ => [\n");
            for group in &self.ebnf.conflicts {
                let names: Vec<String> =
                    group.iter().map(|n| format!("$.{}", self.node_of(n))).collect();
                out.push_str(&format!("    [{}],\n", names.join(", ")));
            }
            out.push_str("  ],\n\n");
        }

        out.push_str("  rules: {\n");
        let mut emitted: HashSet<String> = HashSet::new();
        let mut first_level = true;
        for name in &reachable {
            let p = self.prod(name);
            if p.ann.inline || p.ann.external || p.ann.same_as.is_some() {
                continue;
            }
            if self.levels.contains_key(name) {
                // The whole cascade is written where its first level stands.
                if first_level {
                    first_level = false;
                    for (rule, body) in &cascade_rules {
                        write_rule(&mut out, rule, body);
                        emitted.insert(rule.clone());
                    }
                }
                continue;
            }
            let rule = self.node_of(name).to_string();
            if !emitted.insert(rule.clone()) {
                continue;
            }
            write_rule(&mut out, &rule, &self.rule_body(p)?);
        }
        // Every rule is followed by a blank line; the last one is not.
        while out.ends_with("\n\n") {
            out.pop();
        }
        out.push_str("  },\n");
        out.push_str("});\n");
        Ok(out)
    }
}

/// A rule's body wraps from the rule's own indentation rather than from the
/// column its head happens to end at, so a rename never reflows the file.
fn write_rule(out: &mut String, name: &str, body: &Js) {
    let head = format!("    {name}: $ => ");
    let flat = body.flat();
    out.push_str(&head);
    if head.len() + flat.len() <= 96 {
        out.push_str(&flat);
    } else {
        let mut rendered = String::new();
        body.render(4, &mut rendered);
        out.push_str(&rendered);
    }
    out.push_str(",\n\n");
}

const HEADER: &str = "\
// GENERATED from cli/src/docs/grammar.ebnf — do not edit.
//
// The EBNF is the normative grammar and the only place this language's syntax
// is written down. It carries what tree-sitter needs beyond a context-free
// grammar — node names, hidden rules, field names, the external scanner's
// terminals, and the precedence cascade — as `@` directives in its comments,
// and `cli/src/documentation/grammar.rs` turns them into this file.
//
// To change the grammar, edit the EBNF and run:
//
//   BURI_BLESS=1 cargo test -p buri --test corpus the_tree_sitter_grammar
//
// `src/scanner.c` is hand-written and stays that way: string interpolation and
// nestable block comments need a lexer with state, which no declarative
// grammar can express. The order of `externals` below is the order of the
// enum in that file.

";

#[cfg(test)]
mod tests {
    use super::*;

    fn ebnf() -> Ebnf {
        parse(crate::documentation::topics::GRAMMAR).expect("the grammar parses")
    }

    #[test]
    fn the_grammar_parses_and_every_reference_resolves() {
        let g = ebnf();
        assert!(g.productions.len() > 80, "only {} productions", g.productions.len());
        let dangling = dangling_references(&g);
        assert!(dangling.is_empty(), "{}", dangling.join("\n"));
    }

    #[test]
    fn names_follow_the_words_table() {
        let words = vec![
            ("Decl".to_string(), "declaration".to_string()),
            ("Fn".to_string(), "function".to_string()),
            ("Param".to_string(), "parameter".to_string()),
        ];
        assert_eq!(node_name("FnDecl", &words, false), "function_declaration");
        assert_eq!(node_name("Params", &words, false), "parameters");
        assert_eq!(node_name("ReExport", &words, false), "re_export");
        assert_eq!(node_name("Item", &words, true), "_item");
    }

    #[test]
    fn a_lexical_production_compiles_to_a_pattern() {
        let out = generate(crate::documentation::topics::GRAMMAR).expect("generates");
        assert!(out.contains("integer_literal: $ => /"), "no integer pattern:\n{out}");
        assert!(out.contains("0x[0-9a-fA-F]"), "the hexadecimal form is not derived");
    }

    #[test]
    fn the_cascade_becomes_one_node_with_precedences() {
        let out = generate(crate::documentation::topics::GRAMMAR).expect("generates");
        assert!(out.contains("prec.left(1, seq($._operand, '||', $._operand))"));
        assert!(out.contains("prec.right(2, seq($._operand, '??', $._operand))"));
        assert!(out.contains("prec.right(10,"), "the prefix level is not right-associative");
    }

    #[test]
    fn generating_twice_gives_the_same_bytes() {
        let once = generate(crate::documentation::topics::GRAMMAR).expect("generates");
        let twice = generate(crate::documentation::topics::GRAMMAR).expect("generates");
        assert_eq!(once, twice);
    }

    /// The two escape hatches, on a grammar small enough to read whole.
    ///
    /// `@raw` is not used by `grammar.ebnf` and is here so that a corner which
    /// cannot be said in EBNF has somewhere to go that is not the EBNF. An
    /// escape hatch nothing exercises is an escape hatch that has quietly
    /// stopped working, so this exercises it.
    #[test]
    fn the_escape_hatches_work() {
        let source = "\
(* @grammar tiny
   @word WORD

   @words Decl=declaration *)

(* @node source_file *)
Doc             ::= Line*

(* @hidden *)
Line            ::= Decl | Blank

(* @prec 3 *)
Decl            ::= \"let\" name=WORD \";\"

(* An error-recovery shape with no EBNF for it.

   @raw seq('#', repeat(choice($.declaration, '!'))) *)
Blank           ::= \"#\"

(* @node word @regex [a-z]+ *)
WORD            ::= a lowercase word
";
        let out = generate(source).expect("the small grammar generates");
        assert!(out.contains("name: 'tiny',"), "{out}");
        assert!(out.contains("word: $ => $.word,"), "{out}");
        assert!(out.contains("source_file: $ => repeat($._line),"), "{out}");
        assert!(out.contains("_line: $ => choice($.declaration, $.blank),"), "{out}");
        assert!(
            out.contains("declaration: $ => prec(3, seq('let', field('name', $.word), ';')),"),
            "{out}"
        );
        // The raw body is passed through, and the prose beside it is not read.
        assert!(
            out.contains("blank: $ => seq('#', repeat(choice($.declaration, '!'))),"),
            "{out}"
        );
        assert!(out.contains("word: $ => /[a-z]+/,"), "{out}");
    }

    /// A production naming something that is not a production is the one
    /// mistake a file of prose cannot report on its own.
    #[test]
    fn a_dangling_reference_is_named() {
        let source = "(* @grammar tiny *)\nDoc ::= Line*\nLine ::= Gone\n";
        let g = parse(source).expect("parses");
        let dangling = dangling_references(&g);
        assert_eq!(dangling, vec!["Line names `Gone`, which is not a production"]);
        assert!(generate(source).is_err(), "generation accepted a dangling reference");
    }
}
