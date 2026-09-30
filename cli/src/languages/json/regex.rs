//! The regular expressions `pattern` and `patternProperties` hold.
//!
//! JSON Schema asks for ECMA-262 syntax, matched anywhere in the string. This
//! reads the part of it schemas use, over Unicode code points:
//!
//! ```text
//! a|b  (x)  (?:x)  (?<name>x)  (?=x)  (?!x)
//! *  +  ?  {n}  {n,}  {n,m}, each greedy or lazy with a trailing `?`
//! .  ^  $  \b  \B  [a-z]  [^0-9]  \d \D \w \W \s \S
//! \t \n \r \v \f \0  \xHH  \uHHHH  \u{H…}  \cX  and `\` before punctuation
//! ```
//!
//! Lookbehind, back-references and `\p{…}` are refused by name rather than
//! read as something else.

#[derive(Clone, Debug)]
enum Node {
    Char(char),
    Any,
    Class(Vec<Item>, bool),
    Start,
    End,
    WordBoundary(bool),
    Group(Box<Node>),
    Look(Box<Node>, bool),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat { node: Box<Node>, min: u32, max: Option<u32>, greedy: bool },
}

#[derive(Clone, Copy, Debug)]
enum Item {
    Range(char, char),
    Digit(bool),
    Word(bool),
    Space(bool),
}

impl Item {
    fn matches(self, c: char) -> bool {
        match self {
            Item::Range(a, b) => a <= c && c <= b,
            Item::Digit(yes) => c.is_ascii_digit() == yes,
            Item::Word(yes) => is_word(c) == yes,
            Item::Space(yes) => is_space(c) == yes,
        }
    }
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2028}' | '\u{2029}'
            | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    ) || ('\u{2000}'..='\u{200a}').contains(&c)
}

fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

#[derive(Clone, Debug)]
pub struct Regex {
    root: Node,
}

/// How many steps one match may take before it is abandoned.
const BUDGET: u64 = 2_000_000;

/// How deeply the matcher may recurse.
const MAX_DEPTH: usize = 2_000;

/// Why a match was abandoned.
#[derive(Debug, PartialEq, Eq)]
pub struct TooCostly;

impl Regex {
    pub fn new(pattern: &str) -> Result<Regex, String> {
        let chars: Vec<char> = pattern.chars().collect();
        let mut p = Parser { chars: &chars, at: 0 };
        let root = p.alternation(0)?;
        if let Some(c) = p.peek() {
            return Err(format!("`{c}` is unbalanced"));
        }
        Ok(Regex { root })
    }

    /// Whether the pattern matches anywhere in `text`.
    pub fn is_match(&self, text: &str) -> Result<bool, TooCostly> {
        let chars: Vec<char> = text.chars().collect();
        let mut m = Matcher { text: &chars, steps: 0, depth: 0 };
        for start in 0..=chars.len() {
            if m.at(&self.root, start, &Cont::Done)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

struct Parser<'p> {
    chars: &'p [char],
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.chars.get(self.at.saturating_add(n)).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at = self.at.saturating_add(1);
        Some(c)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.at = self.at.saturating_add(1);
            return true;
        }
        false
    }

    fn alternation(&mut self, depth: usize) -> Result<Node, String> {
        if depth > 100 {
            return Err("groups nest too deeply".to_string());
        }
        let mut alts = vec![self.sequence(depth)?];
        while self.eat('|') {
            alts.push(self.sequence(depth)?);
        }
        Ok(if alts.len() == 1 { alts.pop().unwrap_or(Node::Concat(Vec::new())) } else { Node::Alt(alts) })
    }

    fn sequence(&mut self, depth: usize) -> Result<Node, String> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom(depth)?;
            items.push(self.quantified(atom)?);
        }
        Ok(Node::Concat(items))
    }

    fn quantified(&mut self, atom: Node) -> Result<Node, String> {
        let (min, max) = match self.peek() {
            Some('{') => match self.braces() {
                Some(bounds) => bounds,
                None => return Ok(atom),
            },
            Some(c @ ('*' | '+' | '?')) => {
                self.bump();
                match c {
                    '*' => (0, None),
                    '+' => (1, None),
                    _ => (0, Some(1)),
                }
            }
            _ => return Ok(atom),
        };
        if matches!(atom, Node::Start | Node::End | Node::WordBoundary(_) | Node::Look(..)) {
            return Err("an assertion cannot be repeated".to_string());
        }
        if let Some(max) = max {
            if max < min {
                return Err(format!("`{{{min},{max}}}` is out of order"));
            }
        }
        let greedy = !self.eat('?');
        if matches!(self.peek(), Some('*' | '+' | '?')) {
            return Err("a quantifier cannot follow a quantifier".to_string());
        }
        Ok(Node::Repeat { node: Box::new(atom), min, max, greedy })
    }

    /// `{n}`, `{n,}` or `{n,m}`, consumed. `None`, and nothing consumed, for a
    /// brace that is not one of those.
    fn braces(&mut self) -> Option<(u32, Option<u32>)> {
        let start = self.at;
        self.bump();
        let min = self.decimal();
        let result = match (min, self.peek()) {
            (Some(n), Some('}')) => Some((n, Some(n))),
            (Some(n), Some(',')) => {
                self.bump();
                match (self.decimal(), self.peek()) {
                    (m, Some('}')) => Some((n, m)),
                    _ => None,
                }
            }
            _ => None,
        };
        match result {
            Some(r) => {
                self.bump();
                Some(r)
            }
            None => {
                self.at = start;
                None
            }
        }
    }

    fn decimal(&mut self) -> Option<u32> {
        let mut n: Option<u32> = None;
        while let Some(d) = self.peek().and_then(|c| c.to_digit(10)) {
            self.bump();
            n = Some(n.unwrap_or(0).saturating_mul(10).saturating_add(d));
        }
        n
    }

    fn atom(&mut self, depth: usize) -> Result<Node, String> {
        let Some(c) = self.bump() else { return Err("the pattern ends early".to_string()) };
        Ok(match c {
            '.' => Node::Any,
            '^' => Node::Start,
            '$' => Node::End,
            '(' => {
                let look = if self.peek() == Some('?') {
                    match (self.peek_at(1), self.peek_at(2)) {
                        (Some(':'), _) => {
                            self.at = self.at.saturating_add(2);
                            None
                        }
                        (Some('='), _) => {
                            self.at = self.at.saturating_add(2);
                            Some(true)
                        }
                        (Some('!'), _) => {
                            self.at = self.at.saturating_add(2);
                            Some(false)
                        }
                        (Some('<'), Some('=' | '!')) => {
                            return Err("lookbehind is not supported".to_string())
                        }
                        (Some('<'), _) => {
                            self.at = self.at.saturating_add(2);
                            while self.peek().is_some_and(|c| c != '>') {
                                self.bump();
                            }
                            if !self.eat('>') {
                                return Err("a group name is not closed".to_string());
                            }
                            None
                        }
                        _ => return Err("`(?` starts no group this reads".to_string()),
                    }
                } else {
                    None
                };
                let inner = self.alternation(depth.saturating_add(1))?;
                if !self.eat(')') {
                    return Err("a group is not closed".to_string());
                }
                match look {
                    Some(positive) => Node::Look(Box::new(inner), positive),
                    None => Node::Group(Box::new(inner)),
                }
            }
            ')' => return Err("`)` closes nothing".to_string()),
            '[' => self.class()?,
            '*' | '+' | '?' => return Err(format!("`{c}` has nothing to repeat")),
            '{' if self.braces_follow() => return Err("`{` has nothing to repeat".to_string()),
            '\\' => match self.escape(false)? {
                Escaped::Char(c) => Node::Char(c),
                Escaped::Item(item) => Node::Class(vec![item], false),
                Escaped::Boundary(yes) => Node::WordBoundary(yes),
            },
            c => Node::Char(c),
        })
    }

    fn braces_follow(&mut self) -> bool {
        self.at = self.at.saturating_sub(1);
        let is = self.braces().is_some();
        if !is {
            self.bump();
        }
        is
    }

    fn class(&mut self) -> Result<Node, String> {
        let negated = self.eat('^');
        let mut items = Vec::new();
        loop {
            let Some(c) = self.bump() else { return Err("a `[` class is not closed".to_string()) };
            if c == ']' {
                break;
            }
            let low = if c == '\\' {
                match self.escape(true)? {
                    Escaped::Char(c) => c,
                    Escaped::Item(item) => {
                        items.push(item);
                        continue;
                    }
                    Escaped::Boundary(_) => '\u{8}',
                }
            } else {
                c
            };
            if self.peek() == Some('-') && self.peek_at(1).is_some_and(|c| c != ']') {
                self.bump();
                let hc = self.bump().unwrap_or(low);
                let high = if hc == '\\' {
                    match self.escape(true)? {
                        Escaped::Char(c) => c,
                        _ => return Err("a range cannot end in a class escape".to_string()),
                    }
                } else {
                    hc
                };
                if high < low {
                    return Err(format!("`{low}-{high}` is out of order"));
                }
                items.push(Item::Range(low, high));
            } else {
                items.push(Item::Range(low, low));
            }
        }
        Ok(Node::Class(items, negated))
    }

    fn escape(&mut self, in_class: bool) -> Result<Escaped, String> {
        let Some(c) = self.bump() else { return Err("the pattern ends in `\\`".to_string()) };
        Ok(match c {
            'd' => Escaped::Item(Item::Digit(true)),
            'D' => Escaped::Item(Item::Digit(false)),
            'w' => Escaped::Item(Item::Word(true)),
            'W' => Escaped::Item(Item::Word(false)),
            's' => Escaped::Item(Item::Space(true)),
            'S' => Escaped::Item(Item::Space(false)),
            'b' if !in_class => Escaped::Boundary(true),
            'B' if !in_class => Escaped::Boundary(false),
            'b' => Escaped::Char('\u{8}'),
            't' => Escaped::Char('\t'),
            'n' => Escaped::Char('\n'),
            'r' => Escaped::Char('\r'),
            'v' => Escaped::Char('\u{b}'),
            'f' => Escaped::Char('\u{c}'),
            '0' if !self.peek().is_some_and(|c| c.is_ascii_digit()) => Escaped::Char('\0'),
            'x' => Escaped::Char(self.hex(2)?),
            'u' if self.eat('{') => {
                let mut v = 0u32;
                let mut any = false;
                while let Some(d) = self.peek().and_then(|c| c.to_digit(16)) {
                    self.bump();
                    any = true;
                    v = v.saturating_mul(16).saturating_add(d);
                }
                if !any || !self.eat('}') {
                    return Err("`\\u{` needs hexadecimal digits and a `}`".to_string());
                }
                Escaped::Char(char::from_u32(v).ok_or("`\\u{…}` names no character")?)
            }
            'u' => Escaped::Char(self.hex(4)?),
            'c' => match self.bump() {
                Some(l) if l.is_ascii_alphabetic() => {
                    Escaped::Char(char::from(u8::try_from(u32::from(l) & 31).unwrap_or(0)))
                }
                _ => return Err("`\\c` needs a letter".to_string()),
            },
            'p' | 'P' => return Err("`\\p{…}` Unicode properties are not supported".to_string()),
            'k' => return Err("back-references are not supported".to_string()),
            '1'..='9' => return Err("back-references are not supported".to_string()),
            c if c.is_ascii_alphanumeric() => return Err(format!("`\\{c}` is not an escape")),
            c => Escaped::Char(c),
        })
    }

    fn hex(&mut self, n: usize) -> Result<char, String> {
        let mut v = 0u32;
        for _ in 0..n {
            let d = self
                .bump()
                .and_then(|c| c.to_digit(16))
                .ok_or_else(|| format!("an escape needs {n} hexadecimal digits"))?;
            v = v.saturating_mul(16).saturating_add(d);
        }
        char::from_u32(v).ok_or_else(|| "a surrogate escape is not supported".to_string())
    }
}

enum Escaped {
    Char(char),
    Item(Item),
    Boundary(bool),
}

struct Matcher<'t> {
    text: &'t [char],
    steps: u64,
    depth: usize,
}

/// What is left to match after the node in hand.
enum Cont<'a> {
    Done,
    Seq(&'a [Node], &'a Cont<'a>),
    /// One iteration of a repeat has ended; `start` is where it began.
    Again(Repeat<'a>),
}

#[derive(Clone, Copy)]
struct Repeat<'a> {
    node: &'a Node,
    done: u32,
    min: u32,
    max: Option<u32>,
    greedy: bool,
    start: usize,
    next: &'a Cont<'a>,
}

impl Matcher<'_> {
    fn one(&self, node: &Node, pos: usize) -> Option<bool> {
        let c = self.text.get(pos).copied();
        match node {
            Node::Char(want) => Some(c == Some(*want)),
            Node::Any => Some(c.is_some_and(|c| !is_line_terminator(c))),
            Node::Class(items, negated) => {
                Some(c.is_some_and(|c| items.iter().any(|i| i.matches(c)) != *negated))
            }
            _ => None,
        }
    }

    fn word_at(&self, pos: usize) -> bool {
        self.text.get(pos).copied().is_some_and(is_word)
    }

    fn spend(&mut self) -> Result<(), TooCostly> {
        self.steps = self.steps.saturating_add(1);
        if self.steps > BUDGET || self.depth > MAX_DEPTH {
            return Err(TooCostly);
        }
        Ok(())
    }

    /// Matches `node` at `pos`, then what `k` says is left.
    fn at(&mut self, node: &Node, pos: usize, k: &Cont) -> Result<bool, TooCostly> {
        self.spend()?;
        self.depth = self.depth.saturating_add(1);
        let r = self.at_inner(node, pos, k);
        self.depth = self.depth.saturating_sub(1);
        r
    }

    fn at_inner(&mut self, node: &Node, pos: usize, k: &Cont) -> Result<bool, TooCostly> {
        if let Some(hit) = self.one(node, pos) {
            return if hit { self.then(k, pos.saturating_add(1)) } else { Ok(false) };
        }
        match node {
            Node::Start => {
                if pos == 0 {
                    self.then(k, pos)
                } else {
                    Ok(false)
                }
            }
            Node::End => {
                if pos == self.text.len() {
                    self.then(k, pos)
                } else {
                    Ok(false)
                }
            }
            Node::WordBoundary(yes) => {
                let before = pos > 0 && self.word_at(pos.saturating_sub(1));
                if (before != self.word_at(pos)) == *yes {
                    self.then(k, pos)
                } else {
                    Ok(false)
                }
            }
            Node::Group(inner) => self.at(inner, pos, k),
            Node::Look(inner, positive) => {
                let found = self.at(inner, pos, &Cont::Done)?;
                if found == *positive {
                    self.then(k, pos)
                } else {
                    Ok(false)
                }
            }
            Node::Alt(alts) => {
                for alt in alts {
                    if self.at(alt, pos, k)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Node::Concat(items) => self.then(&Cont::Seq(items, k), pos),
            Node::Repeat { node, min, max, greedy } => {
                if self.one(node, pos).is_some() {
                    return self.simple_repeat(node, pos, (*min, *max, *greedy), k);
                }
                let r = Repeat { node, done: 0, min: *min, max: *max, greedy: *greedy, start: pos, next: k };
                self.repeat(r, pos)
            }
            Node::Char(_) | Node::Any | Node::Class(..) => Ok(false),
        }
    }

    fn then(&mut self, k: &Cont, pos: usize) -> Result<bool, TooCostly> {
        match k {
            Cont::Done => Ok(true),
            Cont::Seq(items, next) => match items.split_first() {
                None => self.then(next, pos),
                Some((first, rest)) => self.at(first, pos, &Cont::Seq(rest, next)),
            },
            Cont::Again(r) => {
                // An iteration that matched nothing ends the repeat, or it
                // would go round for ever.
                if pos == r.start && r.done >= r.min {
                    return Ok(false);
                }
                self.repeat(Repeat { done: r.done.saturating_add(1), ..*r }, pos)
            }
        }
    }

    /// A repeat of one character, counted without recursing per character.
    fn simple_repeat(
        &mut self,
        node: &Node,
        pos: usize,
        (min, max, greedy): (u32, Option<u32>, bool),
        k: &Cont,
    ) -> Result<bool, TooCostly> {
        let limit = max.map_or(usize::MAX, |m| m as usize);
        let mut count = 0usize;
        while count < limit && self.one(node, pos.saturating_add(count)) == Some(true) {
            count = count.saturating_add(1);
        }
        let min = min as usize;
        if count < min {
            return Ok(false);
        }
        for i in 0..=count.saturating_sub(min) {
            let n = if greedy { count.saturating_sub(i) } else { min.saturating_add(i) };
            self.spend()?;
            if self.then(k, pos.saturating_add(n))? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `r.done` iterations have matched and the next would start at `pos`.
    fn repeat(&mut self, r: Repeat, pos: usize) -> Result<bool, TooCostly> {
        let can_more = r.max.is_none_or(|m| r.done < m);
        let again = Cont::Again(Repeat { start: pos, ..r });
        if r.greedy {
            if can_more && self.at(r.node, pos, &again)? {
                return Ok(true);
            }
            return if r.done >= r.min { self.then(r.next, pos) } else { Ok(false) };
        }
        if r.done >= r.min && self.then(r.next, pos)? {
            return Ok(true);
        }
        if can_more {
            return self.at(r.node, pos, &again);
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pattern: &str, text: &str) -> bool {
        Regex::new(pattern).unwrap().is_match(text).unwrap()
    }

    #[test]
    fn a_pattern_matches_anywhere_unless_anchored() {
        assert!(m("b", "abc"));
        assert!(!m("^b", "abc"));
        assert!(m("^[a-z]+$", "abc"));
        assert!(!m("^[a-z]+$", "ab1"));
        assert!(m("^\\d{3}-\\d{4}$", "555-1212"));
        assert!(!m("^\\d{3}-\\d{4}$", "5555-1212"));
    }

    #[test]
    fn alternation_groups_and_repeats() {
        assert!(m("^(ab|cd)+$", "abcdab"));
        assert!(!m("^(ab|cd)+$", "abc"));
        assert!(m("^a*?b$", "aaab"));
        assert!(m("^(a|b)*c$", "ababc"));
        assert!(m("^(?:x{2,3})$", "xxx"));
        assert!(!m("^(?:x{2,3})$", "xxxx"));
        assert!(m("^(?!foo).*$", "bar"));
        assert!(!m("^(?!foo).*$", "food"));
        assert!(m("^(a*)*$", "aaaa"));
    }

    #[test]
    fn unsupported_syntax_is_refused() {
        assert!(Regex::new("(?<=a)b").is_err());
        assert!(Regex::new("(a)\\1").is_err());
        assert!(Regex::new("\\p{L}").is_err());
        assert!(Regex::new("(a").is_err());
        assert!(Regex::new("*").is_err());
    }

    #[test]
    fn a_catastrophic_pattern_is_abandoned_rather_than_run() {
        let r = Regex::new("^(a+)+$").unwrap();
        assert_eq!(r.is_match(&format!("{}b", "a".repeat(40))), Err(TooCostly));
    }
}
