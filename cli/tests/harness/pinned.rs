//! **Which mutations get checked in, and under what name.**
//!
//! `mutation.rs` makes thousands of broken files and `recovery.rs` holds them
//! to invariants. An invariant says what every case must satisfy; it cannot
//! say what one case *prints*. So a second, smaller population is checked in
//! whole — a broken input and the toolchain's answer to it, recorded — and
//! this module decides which mutations that population is made of.
//!
//! # The sampling rule
//!
//! Blind volume is not coverage: a thousand deleted commas from one file are
//! one test run a thousand times. So a case is chosen for the *cell* it is the
//! only member of.
//!
//! A **cell** is the shape of the mistake, and it is five things:
//!
//! 1. the mutation kind — deleted separator, deleted closer, stray token, swap;
//! 2. the innermost delimiter open at the site — `{}`, `[]`, `()`, module;
//! 3. the declaration the site is inside — `fn`, `struct`, `enum`, `import`, …;
//! 4. what opened that delimiter, as the token before it — an identifier before
//!    a `(` is a call, `fn` before one is a parameter list;
//! 5. the kinds of the tokens either side of the site.
//!
//! Two mutations in the same cell are the same test, so at most
//! [`PER_CELL`] of any cell is kept. Within a cell the candidates are ordered
//! **shortest seed first**: a pinned case is read by a person, and the smallest
//! file that exhibits a shape is the one worth reading. Cells are then taken
//! round-robin, so that cutting the corpus at a smaller `total` thins every
//! shape evenly rather than truncating the alphabet.
//!
//! The whole selection is a function of the corpus, the seed and two numbers,
//! and nothing in it reads the checked-in tree — which is what makes
//! regeneration a byte-identical operation rather than a bless.

#![allow(dead_code, reason = "each test binary takes the part of this it needs")]

use crate::mutation::{mutations_of, Kind, Mutation, Nest, Source};
use buri::diagnostics::FileId;
use buri::parsing::lexer::{lex, TokenKind};
use std::collections::BTreeMap;

/// How many mutations of each shape each seed offers the sampler.
///
/// Larger than the invariants' own draw, because this population is filtered
/// down to one case per cell afterwards: a wider offer fills more cells from
/// the seeds that have them rather than more cases from the seeds that do not.
pub const PER_KIND: usize = 12;

/// The largest seed a pinned case is made from.
///
/// A pinned case is read by a person, and a thirty-kilobyte conformance file
/// with one comma missing is not a case anybody reads — it is the *invariants*
/// that hold the whole corpus, at every size, and they run over all of it. So
/// the checked-in half is drawn from the small end: at four kilobytes the
/// corpus keeps every construct the language has and loses the six files whose
/// bytes would otherwise be most of the tree.
pub const SEED_BYTES: usize = 4_000;

/// How many cases one cell may contribute.
///
/// Two rather than one so that a shape is seen in two different files — the
/// same missing comma in a record and in a struct literal reads differently —
/// and not more, because a third is the same test a third time.
pub const PER_CELL: usize = 2;

/// One mutation chosen to be checked in.
pub struct Pick {
    /// The case directory's name, stable under corpus growth.
    pub name: String,
    /// The coverage cell this case was chosen for.
    pub cell: String,
    pub mutation: Mutation,
}

/// Every token of one seed, with what an assertion about a site needs.
struct Shape {
    kind: Vec<TokenKind>,
    start: Vec<usize>,
    text: Vec<String>,
    nest: Vec<Nest>,
    /// The token that opened the innermost delimiter, or `None` at module level.
    opener: Vec<Option<usize>>,
    /// The declaration keyword this token is written inside.
    decl: Vec<Option<usize>>,
}

impl Shape {
    fn of(text: &str) -> Option<Shape> {
        let lexed = lex(text, FileId(0));
        if !lexed.errors.is_empty() {
            return None;
        }
        let tokens = &lexed.tokens;
        let n = tokens.len();
        let mut shape = Shape {
            kind: Vec::with_capacity(n),
            start: Vec::with_capacity(n),
            text: Vec::with_capacity(n),
            nest: Vec::with_capacity(n),
            opener: Vec::with_capacity(n),
            decl: Vec::with_capacity(n),
        };
        let mut stack: Vec<(Nest, usize)> = Vec::new();
        let mut decl: Option<usize> = None;
        for i in 0..n {
            let span = tokens.span(i);
            let (s, e) = (span.start as usize, span.end as usize);
            let kind = tokens.kind(i);
            if matches!(kind, TokenKind::RBrace | TokenKind::RParen | TokenKind::RBracket) {
                stack.pop();
            }
            let here = stack.last().copied();
            if here.is_none() && is_declaration_keyword(kind) {
                decl = Some(i);
            }
            shape.kind.push(kind);
            shape.start.push(s);
            shape.text.push(text.get(s..e).unwrap_or_default().to_string());
            shape.nest.push(here.map_or(Nest::Module, |(n, _)| n));
            shape.opener.push(here.map(|(_, at)| at));
            shape.decl.push(decl);
            match kind {
                TokenKind::LBrace => stack.push((Nest::Brace, i)),
                TokenKind::LParen => stack.push((Nest::Paren, i)),
                TokenKind::LBracket => stack.push((Nest::Bracket, i)),
                _ => {}
            }
        }
        Some(shape)
    }

    /// The token index a mutation sits at. Every mutation records the start
    /// byte of the token it perturbed, so the index is a lookup.
    fn index_at(&self, byte: usize) -> Option<usize> {
        self.start.binary_search(&byte).ok()
    }
}

/// The keywords a declaration begins with, as they read at module level.
///
/// `export` is one of them and is usually followed by another, so the *last*
/// one before the site wins and a case inside an exported function says `fn`.
fn is_declaration_keyword(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::KeywordFn
            | TokenKind::KeywordStruct
            | TokenKind::KeywordEnum
            | TokenKind::KeywordType
            | TokenKind::KeywordTrait
            | TokenKind::KeywordImpl
            | TokenKind::KeywordEffect
            | TokenKind::KeywordDerive
            | TokenKind::KeywordImport
            | TokenKind::KeywordFrom
            | TokenKind::KeywordConst
            | TokenKind::KeywordTest
            | TokenKind::KeywordContext
    )
}

/// A token kind as a cell writes it: short, and the same for every identifier.
fn tag(kind: TokenKind) -> String {
    format!("{kind:?}")
}

/// The cell one mutation belongs to, or `None` where the site is not a token
/// this module can describe — the first and last token of a file, which
/// `mutation.rs` does not draw from anyway.
fn cell_of(shape: &Shape, m: &Mutation) -> Option<String> {
    let i = shape.index_at(m.site)?;
    let decl = shape
        .decl
        .get(i)
        .copied()
        .flatten()
        .and_then(|at| shape.text.get(at))
        .map_or("-", String::as_str);
    let opener = match shape.opener.get(i).copied().flatten() {
        Some(at) => {
            let before = at.checked_sub(1).and_then(|b| shape.kind.get(b)).copied();
            let open = shape.text.get(at).map_or("-", String::as_str);
            format!("{}{open}", before.map_or(String::from("-"), tag))
        }
        None => String::from("-"),
    };
    let before = i.checked_sub(1).and_then(|b| shape.kind.get(b)).copied().map_or_else(
        || String::from("-"),
        tag,
    );
    let after = shape.kind.get(i.saturating_add(1)).copied().map_or_else(
        || String::from("-"),
        tag,
    );
    Some(format!(
        "{} {} | {decl} | {opener} | {before}>{after}",
        m.kind.name(),
        m.nest.name()
    ))
}

/// A short stable digest of the text a name has to stay unique over.
fn digest(of: &str) -> String {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for b in of.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    format!("{:06x}", h & 0xFF_FFFF)
}

/// The seed file's own name, as a case directory spells it.
fn stem(file: &str) -> String {
    let base = file.rsplit('/').next().unwrap_or(file);
    let base = base.strip_suffix(".buri").unwrap_or(base);
    // A formatting case's file is always `input.buri`; its directory is the
    // half of the path that says what it is about.
    let owner = if base == "input" || base == "main" || base == "lib" {
        file.rsplit('/').nth(1).unwrap_or(base)
    } else {
        base
    };
    owner
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect()
}

/// What a case directory is called: the shape of the mistake, the file it was
/// made from, and enough of a digest to stay unique.
///
/// Deliberately not a serial number. A case named `recovery_0731` would be a
/// different case the moment a source landed in the repository, and every
/// pinned file after it would move in the diff for no reason.
fn name_of(m: &Mutation) -> String {
    let nest = match m.nest {
        Nest::Brace => "brace",
        Nest::Bracket => "bracket",
        Nest::Paren => "paren",
        Nest::Module => "module",
    };
    let kind = m.kind.name().replace('-', "_");
    format!(
        "recovery_{kind}_{nest}_{}_{}",
        stem(&m.file),
        digest(&format!("{}:{}:{}", m.file, m.site, m.what))
    )
}

/// The pinned corpus, in the order its directories sort.
///
/// `keep` is the suite's own question — "is this a case I can record an answer
/// for" — and it is asked only of the candidates a cell would otherwise take,
/// because for the formatting corpus it is a format and a re-parse.
pub fn select(
    sources: &[Source],
    seed: u64,
    total: usize,
    keep: &mut dyn FnMut(&Mutation) -> bool,
) -> Vec<Pick> {
    // Every candidate, bucketed by cell, ordered shortest-seed-first inside it.
    let mut cells: BTreeMap<String, Vec<(usize, String, Mutation)>> = BTreeMap::new();
    for src in sources {
        if src.text.len() > SEED_BYTES {
            continue;
        }
        let Some(shape) = Shape::of(&src.text) else { continue };
        for m in mutations_of(src, seed, PER_KIND) {
            let Some(cell) = cell_of(&shape, &m) else { continue };
            cells.entry(cell).or_default().push((src.text.len(), src.name.clone(), m));
        }
    }
    for candidates in cells.values_mut() {
        candidates.sort_by(|a, b| (a.0, &a.1, a.2.site).cmp(&(b.0, &b.1, b.2.site)));
    }

    // At most `PER_CELL` of each cell, asking the suite about each in turn.
    // A cell's rank inside its own row is what makes the cut below even: the
    // rows are the mutation shapes, and taking every row's first cell before
    // any row's second is the whole of "thins every shape evenly".
    let mut ranks: BTreeMap<String, usize> = BTreeMap::new();
    let mut per_cell: Vec<(usize, Vec<Pick>)> = Vec::with_capacity(cells.len());
    for (cell, candidates) in cells {
        let mut taken: Vec<Pick> = Vec::new();
        for (_, _, m) in candidates {
            if taken.len() >= PER_CELL {
                break;
            }
            if !keep(&m) {
                continue;
            }
            taken.push(Pick { name: name_of(&m), cell: cell.clone(), mutation: m });
        }
        if taken.is_empty() {
            continue;
        }
        let row = cell.split(" | ").next().unwrap_or_default().to_string();
        let rank = ranks.entry(row).or_default();
        per_cell.push((*rank, taken));
        *rank = rank.saturating_add(1);
    }

    // Round-robin: every row's first cell, then every row's second, and only
    // then a second case from any cell.
    let mut order: Vec<(usize, usize)> = Vec::new();
    for slot in 0..PER_CELL {
        let mut round: Vec<(usize, usize, usize)> = per_cell
            .iter()
            .enumerate()
            .filter(|(_, (_, picks))| slot < picks.len())
            .map(|(at, (rank, _))| (*rank, at, slot))
            .collect();
        round.sort_unstable();
        order.extend(round.into_iter().map(|(_, at, slot)| (at, slot)));
    }
    order.truncate(total);
    order.sort_unstable();
    let mut out: Vec<Pick> = Vec::new();
    for (at, (_, cell)) in per_cell.into_iter().enumerate() {
        for (slot, pick) in cell.into_iter().enumerate() {
            if order.binary_search(&(at, slot)).is_ok() {
                out.push(pick);
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Which shapes the corpus covers, as the report prints it.
pub fn coverage(picks: &[Pick]) -> BTreeMap<String, usize> {
    let mut rows: BTreeMap<String, usize> = BTreeMap::new();
    for p in picks {
        let row = p.cell.split(" | ").next().unwrap_or_default().to_string();
        *rows.entry(row).or_default() += 1;
    }
    rows
}

/// Every mutation kind, so a corpus that lost one says so.
pub const KINDS: &[Kind] = Kind::ALL;
