//! **One token wrong, over every source in the repository.**
//!
//! `fuzz.rs` mutates *bytes*, which is how a fuzzer finds a crash. This
//! mutates *tokens*, which is how a suite finds a bad diagnostic: a deleted
//! comma is a mistake a person makes, and what the toolchain says about it is
//! the thing under test. So every mutation here is one a typist could produce
//! and every one of them carries, with the broken text, the three facts an
//! assertion needs — where the mistake is, what token would fix it, and how
//! many diagnostics one mistake is allowed to be worth.
//!
//! Seeded and bounded exactly as `fuzz.rs` is, and for the same reason: a
//! suite whose input moves between runs reports the day rather than the
//! toolchain. `recovery.rs` owns the environment variables; this module is
//! handed a seed and a per-file bound and is otherwise a pure function of the
//! corpus.

#![allow(dead_code, reason = "each test binary takes the part of this it needs")]

use buri::diagnostics::FileId;
use buri::parsing::lexer::{lex, TokenKind};
use std::path::Path;

// ---------------------------------------------------------------------------
// The PRNG
// ---------------------------------------------------------------------------

/// SplitMix64, the same four lines `fuzz.rs` and `benches/generate.rs` use: no
/// dependency, and the same sequence on every machine.
pub struct Rng(u64);

impl Rng {
    pub fn seeded(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

/// A file's own seed, so adding a file to the corpus does not move every other
/// file's draws.
fn seed_of(base: u64, name: &str) -> u64 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for b in name.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    base ^ h
}

// ---------------------------------------------------------------------------
// The corpus
// ---------------------------------------------------------------------------

/// One checked-in source that parses with no error today.
pub struct Source {
    pub name: String,
    pub text: String,
}

/// Every compiling source a mutation starts from.
///
/// Compiling, because the point is what the toolchain says about *one*
/// mistake: a seed that already has three would make every count meaningless.
/// The reject corpus is therefore not here — every file in it is supposed to be
/// refused — and neither is `cli/tests/fuzz/`, whose whole content is input
/// that already broke something.
pub fn corpus(root: &Path) -> Vec<Source> {
    let mut out: Vec<Source> = Vec::new();
    for dir in ["cli/tests/conformance", "cli/tests/example", "cli/tests/formatting"] {
        walk(&root.join(dir), root, &mut out);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.retain(|s| {
        let parsed = buri::parsing::parser::parse(&s.text, FileId(0));
        parsed.errors.is_empty()
    });
    out
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<Source>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<std::path::PathBuf> =
        entries.filter_map(Result::ok).map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            walk(&p, root, out);
            continue;
        }
        let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
        // A build file is textproto, not Buri, and has its own parser.
        if !name.ends_with(".buri") || name == "BUILD.buri" || name == "REPO.buri" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        out.push(Source {
            name: p.strip_prefix(root).unwrap_or(&p).display().to_string(),
            text,
        });
    }
}

// ---------------------------------------------------------------------------
// What a mutation is
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// A `,` or a `;` removed.
    DeleteSeparator,
    /// A `}`, `)` or `]` removed.
    DeleteCloser,
    /// A token the grammar does not admit here, written in.
    InsertStray,
    /// Two adjacent tokens exchanged.
    SwapAdjacent,
}

impl Kind {
    pub const ALL: &'static [Kind] =
        &[Kind::DeleteSeparator, Kind::DeleteCloser, Kind::InsertStray, Kind::SwapAdjacent];

    pub fn name(self) -> &'static str {
        match self {
            Kind::DeleteSeparator => "delete-separator",
            Kind::DeleteCloser => "delete-closer",
            Kind::InsertStray => "insert-stray",
            Kind::SwapAdjacent => "swap-adjacent",
        }
    }
}

/// The innermost delimiter open at the mutation site.
///
/// It is what decides how strict the error-count bound may be. Every
/// comma-separated list terminated by `}` or `]` is written in the breaking
/// form — the loop that `break`s when the separator is absent and then reports
/// the catch-all at the closer — so a missing separator inside one of those is
/// exactly one mistake and must read as exactly one diagnostic. Four of the
/// lists terminated by `)` are written the other way round (`while eat(Comma)`)
/// and fail in a different place, so a comma inside a paren gets the looser
/// bound rather than a claim this module cannot make.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Nest {
    Brace,
    Bracket,
    Paren,
    Module,
}

impl Nest {
    pub fn name(self) -> &'static str {
        match self {
            Nest::Brace => "{}",
            Nest::Bracket => "[]",
            Nest::Paren => "()",
            Nest::Module => "module",
        }
    }
}

/// One mutated source, with everything an assertion about it needs.
pub struct Mutation {
    pub kind: Kind,
    pub nest: Nest,
    /// The checked-in source this was made from.
    pub file: String,
    /// That file and the line the mutation is on, as a failure message names it.
    pub origin: String,
    /// How the mutation reads in a failure message.
    pub what: String,
    /// The mutated text.
    pub source: String,
    /// Byte offset of the mutation in [`Mutation::source`].
    pub site: usize,
    /// Where the first diagnostic has to land: from the start of the token
    /// before the mutation to the end of the token after it, in mutated
    /// coordinates. "At or adjacent to the site", stated in bytes.
    pub window: (usize, usize),
    /// The token whose absence the fix must name, as it is spelled in a
    /// message. `None` where no single token would repair the file.
    pub wants: Option<String>,
}

impl Mutation {
    /// How many diagnostics one mistake may be worth.
    ///
    /// One, wherever the shape of the language says one token repairs the
    /// file and the list it sits in is the breaking form. Two where the
    /// construct is genuinely truncated — a deleted closer leaves the parser a
    /// real choice about where the construct ended — and three for a swap,
    /// which perturbs two tokens rather than one.
    pub fn bound(&self) -> usize {
        match (self.kind, self.nest) {
            (Kind::DeleteSeparator, Nest::Brace | Nest::Bracket | Nest::Module) => 1,
            (Kind::DeleteSeparator, Nest::Paren) => 2,
            (Kind::DeleteCloser, _) | (Kind::InsertStray, _) => 2,
            (Kind::SwapAdjacent, _) => 3,
        }
    }

    /// Whether the bound above is the strict claim — the one R1 is written to.
    pub fn is_strict(&self) -> bool {
        self.bound() == 1
    }
}

// ---------------------------------------------------------------------------
// Making them
// ---------------------------------------------------------------------------

/// Tokens the stray-insertion draw picks from.
///
/// Short, and every entry is a real token of the language: a fragment the
/// lexer refuses stops at the lexer, and what is under test here is what the
/// *parser* says.
const STRAY: &[&str] = &["@", "?", ",", "extra"];

/// Every mutation this file admits, sampled down to `per_kind` of each shape.
///
/// The candidate list is built in token order and the sample is drawn from it
/// with the file's own seed, so the same corpus and the same seed give the same
/// cases on every machine — and a wider bound in a soak run is a superset of
/// what CI drew rather than a different search.
pub fn mutations_of(src: &Source, base_seed: u64, per_kind: usize) -> Vec<Mutation> {
    let lexed = lex(&src.text, FileId(0));
    let tokens = &lexed.tokens;
    if !lexed.errors.is_empty() || tokens.len() < 4 {
        return Vec::new();
    }

    // Byte spans, and the innermost open delimiter at each token.
    let mut span: Vec<(usize, usize)> = Vec::with_capacity(tokens.len());
    let mut nest: Vec<Nest> = Vec::with_capacity(tokens.len());
    let mut stack: Vec<Nest> = Vec::new();
    for i in 0..tokens.len() {
        let s = tokens.span(i);
        span.push((s.start as usize, s.end as usize));
        match tokens.kind(i) {
            TokenKind::RBrace | TokenKind::RParen | TokenKind::RBracket => {
                stack.pop();
            }
            _ => {}
        }
        nest.push(*stack.last().unwrap_or(&Nest::Module));
        match tokens.kind(i) {
            TokenKind::LBrace => stack.push(Nest::Brace),
            TokenKind::LParen => stack.push(Nest::Paren),
            TokenKind::LBracket => stack.push(Nest::Bracket),
            _ => {}
        }
    }

    let text = &src.text;
    let mut by_kind: [Vec<Mutation>; 4] =
        [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    // The first and last token are left alone: a mutation there has no token on
    // one side of it, so the window an assertion needs does not exist.
    for i in 1..tokens.len().saturating_sub(1) {
        let (s, e) = span[i];
        if s >= e || !text.is_char_boundary(s) || !text.is_char_boundary(e) {
            continue;
        }
        let line = line_of(text, s);
        let origin = format!("{}:{line}", src.name);
        match tokens.kind(i) {
            TokenKind::Comma | TokenKind::Semi => {
                let token = if tokens.kind(i) == TokenKind::Comma { "," } else { ";" };
                by_kind[0].push(deletion(
                    Kind::DeleteSeparator,
                    nest[i],
                    &src.name,
                    origin,
                    format!("deleted `{token}`"),
                    text,
                    &span,
                    i,
                    Some(format!("`{token}`")),
                ));
            }
            TokenKind::RBrace | TokenKind::RParen | TokenKind::RBracket => {
                let token = &text[s..e];
                by_kind[1].push(deletion(
                    Kind::DeleteCloser,
                    nest[i],
                    &src.name,
                    origin,
                    format!("deleted `{token}`"),
                    text,
                    &span,
                    i,
                    Some(format!("`{token}`")),
                ));
            }
            _ => {}
        }

        // A stray token before this one. The draw is taken from the index, so
        // it is a function of the site rather than of the sampling order.
        let stray = STRAY[i % STRAY.len()];
        by_kind[2].push(insertion(
            nest[i],
            &src.name,
            origin_of(&src.name, line),
            stray,
            text,
            &span,
            i,
        ));

        // A swap with the token after it, when only blanks lie between them:
        // exchanging across a comment would move the comment too.
        let (ns, ne) = span[i + 1];
        if tokens.kind(i + 1) != TokenKind::Eof
            && ns > e
            && ne > ns
            && text.get(e..ns).is_some_and(|gap| gap.chars().all(|c| c == ' '))
        {
            by_kind[3].push(swap(nest[i], &src.name, origin_of(&src.name, line), text, &span, i));
        }
    }

    let mut out = Vec::new();
    let mut rng = Rng::seeded(seed_of(base_seed, &src.name));
    for mut candidates in by_kind {
        if candidates.len() <= per_kind {
            out.append(&mut candidates);
            continue;
        }
        // A partial Fisher-Yates over the candidate list: `per_kind` distinct
        // draws, no rejection loop, and the same ones for the same seed.
        for slot in 0..per_kind {
            let pick = slot + rng.below(candidates.len() - slot);
            candidates.swap(slot, pick);
        }
        candidates.truncate(per_kind);
        out.append(&mut candidates);
    }
    out
}

fn origin_of(name: &str, line: usize) -> String {
    format!("{name}:{line}")
}

fn line_of(text: &str, at: usize) -> usize {
    text.get(..at).map_or(1, |s| s.matches('\n').count() + 1)
}

/// The mutated text, and the window in *its* coordinates, for a deleted token.
#[allow(clippy::too_many_arguments, reason = "one call site; every field is a field of the result")]
fn deletion(
    kind: Kind,
    nest: Nest,
    file: &str,
    origin: String,
    what: String,
    text: &str,
    span: &[(usize, usize)],
    i: usize,
    wants: Option<String>,
) -> Mutation {
    let (s, e) = span[i];
    let mut source = String::with_capacity(text.len());
    source.push_str(&text[..s]);
    source.push_str(&text[e..]);
    let shift = e - s;
    let lo = span[i - 1].0;
    let hi = span[i + 1].1.saturating_sub(shift);
    Mutation {
        kind,
        nest,
        file: file.to_string(),
        origin,
        what,
        source,
        site: s,
        window: (lo, hi.max(s)),
        wants,
    }
}

fn insertion(
    nest: Nest,
    file: &str,
    origin: String,
    stray: &str,
    text: &str,
    span: &[(usize, usize)],
    i: usize,
) -> Mutation {
    let (s, _) = span[i];
    let mut source = String::with_capacity(text.len() + stray.len() + 1);
    source.push_str(&text[..s]);
    source.push_str(stray);
    source.push(' ');
    source.push_str(&text[s..]);
    let shift = stray.len() + 1;
    let lo = span[i - 1].0;
    let hi = span[i].1 + shift;
    Mutation {
        kind: Kind::InsertStray,
        nest,
        file: file.to_string(),
        origin,
        what: format!("inserted `{stray}`"),
        source,
        site: s,
        window: (lo, hi),
        // Removing the stray token is the repair, and no token names it.
        wants: None,
    }
}

fn swap(
    nest: Nest,
    file: &str,
    origin: String,
    text: &str,
    span: &[(usize, usize)],
    i: usize,
) -> Mutation {
    let (s, e) = span[i];
    let (ns, ne) = span[i + 1];
    let mut source = String::with_capacity(text.len());
    source.push_str(&text[..s]);
    source.push_str(&text[ns..ne]);
    source.push_str(&text[e..ns]);
    source.push_str(&text[s..e]);
    source.push_str(&text[ne..]);
    // The exchange keeps the total length, so offsets past it do not move.
    let lo = span[i - 1].0;
    let hi = span.get(i + 2).map_or(ne, |t| t.1);
    Mutation {
        kind: Kind::SwapAdjacent,
        nest,
        file: file.to_string(),
        origin,
        what: format!("swapped `{}` and `{}`", &text[s..e], &text[ns..ne]),
        source,
        site: s,
        window: (lo, hi),
        wants: None,
    }
}
