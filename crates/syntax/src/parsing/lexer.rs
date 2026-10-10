//! The lexer.
//!
//! Follows the LEXICAL GRAMMAR section of `grammar.ebnf`. Two things about it
//! are worth knowing:
//!
//! * There is no `<<`, `>>`, or `?.` token. Their absence is what keeps
//!   `Wrapper<Wrapper<Int>>` from mis-lexing and `x?.field` from needing a
//!   token splitter (design/grammar-rationale.md 12.6).
//! * Template interpolation is the only mode-dependent part. The lexer keeps
//!   one stack of what is currently open — a brace or an interpolation hole —
//!   so a `}` closing a block inside a hole is told apart from the `}` that
//!   resumes template text by which of the two is on top.

use crate::diagnostics::{Diagnostic, FileId, Invariant as _, Span};
use crate::parsing::flat::{Docs, Location};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Keyword {
    As,
    Const,
    Context,
    Ctx,
    Derive,
    Effect,
    Else,
    Enum,
    Export,
    False,
    Fn,
    For,
    From,
    If,
    Impl,
    Import,
    Let,
    Match,
    SelfValue,
    SelfType,
    Struct,
    Test,
    Trait,
    True,
    Type,
}

impl Keyword {
    /// Every keyword, so a test can hold the hand-written grammar and the
    /// lexer to the same list. Adding a variant without adding it here is a
    /// compile error, because `text` matches exhaustively and this is checked
    /// against it.
    pub const ALL: &'static [Keyword] = &[
        Keyword::As,
        Keyword::Const,
        Keyword::Context,
        Keyword::Ctx,
        Keyword::Derive,
        Keyword::Effect,
        Keyword::Else,
        Keyword::Enum,
        Keyword::Export,
        Keyword::False,
        Keyword::Fn,
        Keyword::For,
        Keyword::From,
        Keyword::If,
        Keyword::Impl,
        Keyword::Import,
        Keyword::Let,
        Keyword::Match,
        Keyword::SelfValue,
        Keyword::SelfType,
        Keyword::Struct,
        Keyword::Test,
        Keyword::Trait,
        Keyword::True,
        Keyword::Type,
    ];

    pub fn text(self) -> &'static str {
        match self {
            Keyword::As => "as",
            Keyword::Const => "const",
            Keyword::Context => "context",
            Keyword::Ctx => "ctx",
            Keyword::Derive => "derive",
            Keyword::Effect => "effect",
            Keyword::Else => "else",
            Keyword::Enum => "enum",
            Keyword::Export => "export",
            Keyword::False => "false",
            Keyword::Fn => "fn",
            Keyword::For => "for",
            Keyword::From => "from",
            Keyword::If => "if",
            Keyword::Impl => "impl",
            Keyword::Import => "import",
            Keyword::Let => "let",
            Keyword::Match => "match",
            Keyword::SelfValue => "self",
            Keyword::SelfType => "Self",
            Keyword::Struct => "struct",
            Keyword::Test => "test",
            Keyword::Trait => "trait",
            Keyword::True => "true",
            Keyword::Type => "type",
        }
    }

}

/// What an identifier-shaped word is, when it is not an identifier: a token
/// of its own — a keyword, or `_` — or a word reserved but unused in v0.3 and
/// rejected by the lexer so that later versions can claim it without breaking
/// source compatibility.
#[derive(Clone, Copy)]
enum Word {
    Kind(TokenKind),
    Reserved,
}

/// The words [`Word::of`] knows, written once.
///
/// "unreachable" is the one reserved word longer than the eight bytes a key
/// holds, so it is not in the table and [`Word::of`] asks for it by name.
const WORDS: &[(&[u8], Word)] = &[
    (b"_", Word::Kind(TokenKind::Underscore)),
    (b"as", Word::Kind(TokenKind::KeywordAs)),
    (b"const", Word::Kind(TokenKind::KeywordConst)),
    (b"context", Word::Kind(TokenKind::KeywordContext)),
    (b"ctx", Word::Kind(TokenKind::KeywordCtx)),
    (b"derive", Word::Kind(TokenKind::KeywordDerive)),
    (b"effect", Word::Kind(TokenKind::KeywordEffect)),
    (b"else", Word::Kind(TokenKind::KeywordElse)),
    (b"enum", Word::Kind(TokenKind::KeywordEnum)),
    (b"export", Word::Kind(TokenKind::KeywordExport)),
    (b"false", Word::Kind(TokenKind::KeywordFalse)),
    (b"fn", Word::Kind(TokenKind::KeywordFn)),
    (b"for", Word::Kind(TokenKind::KeywordFor)),
    (b"from", Word::Kind(TokenKind::KeywordFrom)),
    (b"if", Word::Kind(TokenKind::KeywordIf)),
    (b"impl", Word::Kind(TokenKind::KeywordImpl)),
    (b"import", Word::Kind(TokenKind::KeywordImport)),
    (b"let", Word::Kind(TokenKind::KeywordLet)),
    (b"match", Word::Kind(TokenKind::KeywordMatch)),
    (b"self", Word::Kind(TokenKind::KeywordSelfValue)),
    (b"Self", Word::Kind(TokenKind::KeywordSelfType)),
    (b"struct", Word::Kind(TokenKind::KeywordStruct)),
    (b"test", Word::Kind(TokenKind::KeywordTest)),
    (b"trait", Word::Kind(TokenKind::KeywordTrait)),
    (b"true", Word::Kind(TokenKind::KeywordTrue)),
    (b"type", Word::Kind(TokenKind::KeywordType)),
    (b"async", Word::Reserved),
    (b"await", Word::Reserved),
    (b"break", Word::Reserved),
    (b"continue", Word::Reserved),
    (b"do", Word::Reserved),
    (b"in", Word::Reserved),
    (b"is", Word::Reserved),
    (b"loop", Word::Reserved),
    (b"module", Word::Reserved),
    (b"mut", Word::Reserved),
    (b"opaque", Word::Reserved),
    (b"panic", Word::Reserved),
    (b"pub", Word::Reserved),
    (b"return", Word::Reserved),
    (b"use", Word::Reserved),
    (b"when", Word::Reserved),
    (b"where", Word::Reserved),
    (b"while", Word::Reserved),
    (b"with", Word::Reserved),
    (b"yield", Word::Reserved),
];

/// A word of at most eight bytes as one integer: its bytes little-endian,
/// zero above the last. An identifier byte is never zero, so two words have
/// the same key exactly when they are the same word.
const fn word_key(word: &[u8]) -> u64 {
    let mut key = 0u64;
    let mut shift = 0u32;
    let mut rest = word;
    while let [b, tail @ ..] = rest {
        key |= (*b as u64).wrapping_shl(shift);
        shift = shift.wrapping_add(8);
        rest = tail;
    }
    key
}

/// The multiplier of the hash that puts every key in [`WORDS`] in a slot of
/// its own. Found by search, among multipliers of two nonzero sixteen-bit
/// halves, which take two instructions to load rather than four;
/// [`WORD_TABLE`] refuses to build, at compile time, if a new word collides,
/// and then a new multiplier is needed.
const WORD_HASH: u64 = 0xbe48_0000_9c55_0000;
const WORD_SLOTS: usize = 128;

const fn word_slot(key: u64) -> usize {
    key.wrapping_mul(WORD_HASH).wrapping_shr(57) as usize
}

/// [`WORDS`], addressed by [`word_slot`]. A slot holds its word's key, so a
/// lookup is one multiply, one load and one comparison, against what used to
/// be a `match` on a `&str` that rustc lowered to calls to `memcmp`.
const WORD_TABLE: [(u64, Option<Word>); WORD_SLOTS] = {
    let mut table = [(0u64, None); WORD_SLOTS];
    let mut rest = WORDS;
    while let [(word, kind), tail @ ..] = rest {
        assert!(word.len() <= 8, "a word longer than a key holds");
        let key = word_key(word);
        let slot = word_slot(key);
        match table.split_at_mut_checked(slot) {
            Some((_, [entry, ..])) => {
                assert!(entry.1.is_none(), "two words share a slot: find a new WORD_HASH");
                *entry = (key, Some(*kind));
            }
            _ => panic!("a slot past the table"),
        }
        rest = tail;
    }
    table
};

impl Word {
    /// What the identifier-shaped run of `len` bytes at `start` is, or `None`
    /// for an ordinary identifier. `key` is the run as [`scan_word`] read it:
    /// its first eight bytes, zero above the last.
    fn of(src: &[u8], start: usize, len: usize, key: u64) -> Option<Word> {
        if len > 8 {
            let word = src.get(start..start.saturating_add(len)).unwrap_or(&[]);
            return (word == b"unreachable").then_some(Word::Reserved);
        }
        match WORD_TABLE.get(word_slot(key)) {
            Some(&(k, found)) if k == key => found,
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Punctuation {
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Colon,
    ColonColon,
    Dot,
    DotDot,
    At,
    Underscore,
    Eq,
    FatArrow,
    EqEq,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    AndAnd,
    OrOr,
    Bang,
    QuestionQuestion,
    And,
    Or,
    Caret,
    Tilde,
    Question,
}

impl Punctuation {
    pub fn text(self) -> &'static str {
        match self {
            Punctuation::LBrace => "{",
            Punctuation::RBrace => "}",
            Punctuation::LParen => "(",
            Punctuation::RParen => ")",
            Punctuation::LBracket => "[",
            Punctuation::RBracket => "]",
            Punctuation::Comma => ",",
            Punctuation::Semi => ";",
            Punctuation::Colon => ":",
            Punctuation::ColonColon => "::",
            Punctuation::Dot => ".",
            Punctuation::DotDot => "..",
            Punctuation::At => "@",
            Punctuation::Underscore => "_",
            Punctuation::Eq => "=",
            Punctuation::FatArrow => "=>",
            Punctuation::EqEq => "==",
            Punctuation::BangEq => "!=",
            Punctuation::Lt => "<",
            Punctuation::LtEq => "<=",
            Punctuation::Gt => ">",
            Punctuation::GtEq => ">=",
            Punctuation::Plus => "+",
            Punctuation::Minus => "-",
            Punctuation::Star => "*",
            Punctuation::Slash => "/",
            Punctuation::Percent => "%",
            Punctuation::AndAnd => "&&",
            Punctuation::OrOr => "||",
            Punctuation::Bang => "!",
            Punctuation::QuestionQuestion => "??",
            Punctuation::And => "&",
            Punctuation::Or => "|",
            Punctuation::Caret => "^",
            Punctuation::Tilde => "~",
            Punctuation::Question => "?",
        }
    }
}

/// What a token is, with the keyword and the punctuator folded into the byte.
///
/// The parser asks "is this a `,`" several times per token, and against a
/// tagged union each question was a load of the discriminant followed by a
/// load of the payload beside it. Here it is one byte against a constant, and
/// the kind column is a dense `u8` stream the parser walks in order — which is
/// the whole reason the token buffer is columns rather than records.
///
/// `Keyword` and `Punctuation` survive as public enums, and
/// [`TokenKind::as_keyword`] and [`TokenKind::as_punctuation`] hand one back,
/// so the formatter's tables and every diagnostic that spells a token are
/// untouched.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum TokenKind {
    Ident,
    Int,
    Float,
    Str,
    Char,
    TemplateHead,
    TemplateSpan,
    TemplateTail,
    Eof,
    // `Keyword`, in its own order.
    KeywordAs,
    KeywordConst,
    KeywordContext,
    KeywordCtx,
    KeywordDerive,
    KeywordEffect,
    KeywordElse,
    KeywordEnum,
    KeywordExport,
    KeywordFalse,
    KeywordFn,
    KeywordFor,
    KeywordFrom,
    KeywordIf,
    KeywordImpl,
    KeywordImport,
    KeywordLet,
    KeywordMatch,
    KeywordSelfValue,
    KeywordSelfType,
    KeywordStruct,
    KeywordTest,
    KeywordTrait,
    KeywordTrue,
    KeywordType,
    // `Punctuation`, in its own order.
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Colon,
    ColonColon,
    Dot,
    DotDot,
    At,
    Underscore,
    Eq,
    FatArrow,
    EqEq,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    AndAnd,
    OrOr,
    Bang,
    QuestionQuestion,
    And,
    Or,
    Caret,
    Tilde,
    Question,
}

/// The keyword and punctuator tables, written once and expanded in both
/// directions.
///
/// A token's kind has to become a `Keyword` or a `Punctuation` again — the
/// formatter's tables and every "expected `,`" message are written against
/// those — so the mapping is needed forwards and backwards. Written as two
/// hand-kept matches they could disagree, and a disagreement is a keyword that
/// lexes as another keyword: not a compile error, not obviously a bug in a
/// diff. One list generating both makes that unrepresentable, and the forward
/// arm is exhaustive over the source enum, so a new keyword or punctuator is a
/// build error here rather than a token nothing produces.
macro_rules! kind_tables {
    (keyword: $($k:ident => $kt:ident),* $(,)?; punctuation: $($p:ident => $pt:ident),* $(,)?) => {
        impl TokenKind {
            /// The kind a keyword lexes to.
            pub fn of_keyword(k: Keyword) -> TokenKind {
                match k { $(Keyword::$k => TokenKind::$kt),* }
            }

            /// The kind a punctuator lexes to.
            pub fn of_punctuation(p: Punctuation) -> TokenKind {
                match p { $(Punctuation::$p => TokenKind::$pt),* }
            }

            /// The keyword this kind is, if it is one.
            pub fn as_keyword(self) -> Option<Keyword> {
                Some(match self { $(TokenKind::$kt => Keyword::$k,)* _ => return None })
            }

            /// The punctuator this kind is, if it is one.
            pub fn as_punctuation(self) -> Option<Punctuation> {
                Some(match self { $(TokenKind::$pt => Punctuation::$p,)* _ => return None })
            }
        }
    };
}

kind_tables! {
    keyword:
        As => KeywordAs,
        Const => KeywordConst,
        Context => KeywordContext,
        Ctx => KeywordCtx,
        Derive => KeywordDerive,
        Effect => KeywordEffect,
        Else => KeywordElse,
        Enum => KeywordEnum,
        Export => KeywordExport,
        False => KeywordFalse,
        Fn => KeywordFn,
        For => KeywordFor,
        From => KeywordFrom,
        If => KeywordIf,
        Impl => KeywordImpl,
        Import => KeywordImport,
        Let => KeywordLet,
        Match => KeywordMatch,
        SelfValue => KeywordSelfValue,
        SelfType => KeywordSelfType,
        Struct => KeywordStruct,
        Test => KeywordTest,
        Trait => KeywordTrait,
        True => KeywordTrue,
        Type => KeywordType;
    punctuation:
        LBrace => LBrace,
        RBrace => RBrace,
        LParen => LParen,
        RParen => RParen,
        LBracket => LBracket,
        RBracket => RBracket,
        Comma => Comma,
        Semi => Semi,
        Colon => Colon,
        ColonColon => ColonColon,
        Dot => Dot,
        DotDot => DotDot,
        At => At,
        Underscore => Underscore,
        Eq => Eq,
        FatArrow => FatArrow,
        EqEq => EqEq,
        BangEq => BangEq,
        Lt => Lt,
        LtEq => LtEq,
        Gt => Gt,
        GtEq => GtEq,
        Plus => Plus,
        Minus => Minus,
        Star => Star,
        Slash => Slash,
        Percent => Percent,
        AndAnd => AndAnd,
        OrOr => OrOr,
        Bang => Bang,
        QuestionQuestion => QuestionQuestion,
        And => And,
        Or => Or,
        Caret => Caret,
        Tilde => Tilde,
        Question => Question,
}

/// One token, decoded.
///
/// This is a *view* on [`Tokens`], not what the buffer holds: a token is
/// stored as a byte in the kind column, a span in the location column and at
/// most one index in the payload column, and [`Tokens::token`] puts one of
/// these back together on demand. Every reader of it is cold — a diagnostic
/// that spells the token it found, the formatter's shape check, the lexer's own
/// tests — and the parser, which is not, reads the kind column directly.
#[derive(Clone, PartialEq, Debug)]
pub enum Token<'a> {
    /// Borrowed from the source rather than copied out of it. An identifier is
    /// about a third of the tokens in a file and its text is exactly the
    /// bytes under the token's span, so a `String` here was one allocation per
    /// identifier — the largest single line of the front end's allocation
    /// budget — buying nothing the source did not already hold.
    Ident(&'a str),
    Keyword(Keyword),
    Punctuation(Punctuation),
    /// The value only. What was *written* — `0xFF` rather than `255` — is the
    /// source under the token's span, which every reader of a token already
    /// has; carrying a second copy of it cost a `String` per literal, and made
    /// every token in the file wide enough to hold one.
    Int(u128),
    Float(f64),
    Str(String),
    Char(char),
    /// `"head${`
    TemplateHead(String),
    /// `}span${`
    TemplateSpan(String),
    /// `}tail"`
    TemplateTail(String),
    Eof,
}

impl Token<'_> {
    /// How this token reads in a message, given the source under its span.
    ///
    /// `raw` is a parameter rather than something the token carries because a
    /// numeric literal has to be quoted as it was spelled — `0xFF` and `255`
    /// are one value and two messages — and the spelling is in the source that
    /// every caller already holds. Every other token either spells itself or
    /// is named by its kind, and ignores it.
    pub fn describe(&self, raw: &str) -> String {
        match self {
            Token::Ident(s) => format!("`{s}`"),
            Token::Keyword(k) => format!("`{}`", k.text()),
            Token::Punctuation(p) => format!("`{}`", p.text()),
            Token::Int(_) | Token::Float(_) => format!("`{raw}`"),
            Token::Str(_) => "a string literal".to_string(),
            Token::Char(_) => "a character literal".to_string(),
            Token::TemplateHead(_) | Token::TemplateSpan(_) | Token::TemplateTail(_) => {
                "an interpolated string".to_string()
            }
            Token::Eof => "end of file".to_string(),
        }
    }
}

/// One ordinary comment: a `//` line, or a whole `/* */` block.
///
/// The blank line above it is part of it. A run of comment lines above one
/// declaration is not necessarily one paragraph — a section heading and the
/// sentence about the declaration under it are two — and a formatter that
/// keeps only the lines glues them together.
#[derive(Clone, Debug)]
pub struct Comment {
    /// Where its text is in the file: a `//` line without its trailing
    /// blanks, or the whole of a `/* */`. Read it with [`Comment::text`].
    pub at: Location,
    /// Whether a blank line sat immediately above this comment.
    pub blank_before: bool,
    /// The column its first character was written at, so a formatter can move
    /// the whole of a `/* … */` and keep the shape inside it.
    pub column: u32,
    /// Where its first character was written, so a formatter can tell a
    /// comment written at the end of a line from one written on a line of its
    /// own. The two are different things — the first is about the code beside
    /// it — and the trivia table alone cannot say which this is, because it is
    /// keyed by the token *after* the comment.
    pub offset: u32,
}

impl Comment {
    /// The comment's text, out of the source it was lexed from.
    pub fn text<'s>(&self, src: &'s str) -> &'s str {
        src.get(self.at.start as usize..self.at.end as usize).unwrap_or("")
    }
}

/// What was written above one token: its documentation, the comments above
/// it, and the blank lines around them.
///
/// This is a side table on [`Lexed`] rather than five fields on a token
/// because almost no token has any of it — a run of comments belongs to a
/// declaration, not to the hundreds of tokens inside one — and carrying the
/// fields on the token made every token in the file pay for it. Entries are
/// keyed by token index and pushed in source order, so the table is sorted.
///
/// A token with nothing above it has no entry at all, which is what makes
/// "empty" unrepresentable: `detached` says nothing when there is no run, and
/// `docs_blank` says nothing when there is no documentation.
#[derive(Clone, Debug, Default)]
pub struct Trivia {
    /// Doc comment lines (`///`) immediately preceding this token, as a run
    /// of [`Lexed::docs`].
    pub docs: Docs,
    /// Whether a blank line sat above the doc-comment run. It matters only
    /// when ordinary comments came first: a section heading, a blank line, and
    /// then the declaration's own documentation are three things, not one.
    pub docs_blank: bool,
    /// Ordinary comments immediately preceding, kept so the formatter can put
    /// them back where they were.
    pub comments: Vec<Comment>,
    /// Whether a blank line separated this token's trivia — its comments and
    /// doc lines, or the token itself when it has none — from whatever was
    /// written before. The formatter preserves paragraph breaks between
    /// declarations; the breaks *inside* a comment run are on the comments.
    pub blank_before: bool,
    /// Whether a blank line separated the comment run *below* from this token.
    /// `false` when there is no run, where the question does not arise.
    ///
    /// This is here rather than left to the formatter because the formatter
    /// used to re-derive it by scanning the source backwards, which made two
    /// independent answers to "is there a blank line here" — this one, counted
    /// while lexing, and that one — that could disagree about the same gap.
    pub detached: bool,
}

/// The token buffer: one twelve-byte record per token, and sparse side tables.
///
/// A token used to be a forty-eight-byte record — a tagged union wide enough
/// for a `u128` beside a `Span` — and the buffer is the largest thing the
/// front end builds, written once by the lexer and read once by the parser.
/// The value of a literal — which fewer than one token in ten has — is an
/// index into a table beside the records rather than a hole in every token
/// that is not one, and an identifier is not in the buffer at all, because
/// its text is the source under its own span.
///
/// After that the buffer was three columns, a kind, a span and a payload,
/// thirteen bytes a token. One record is smaller, because the payload fits in
/// the three bytes of padding beside the kind, and it is one store per token
/// rather than three: the lexer's write side was an eighth of its time. The
/// parser reads a kind and then, on `bump`, the span beside it, so the two
/// sharing a cache line costs it nothing.
///
/// Nothing in a record owns anything, so dropping the buffer is a handful of
/// `free`s rather than a walk over every token asking whether it holds a
/// `String`. The width is pinned here rather than left to whatever a new field
/// happens to cost. A field that is empty on almost every token belongs in a
/// side table keyed by token index, not in the record.
pub struct Tokens<'a> {
    src: &'a str,
    file: FileId,
    records: Vec<Record>,
    /// Payloads too wide for a record's three bytes, by token index,
    /// ascending: an index past sixteen million entries, which a file of that
    /// many literals would need. Searched, and empty in every file anybody
    /// has written.
    wide: Vec<(u32, u32)>,
    ints: Vec<u128>,
    floats: Vec<f64>,
    /// Cooked text — a string literal's contents, a template segment's — end
    /// to end, one allocation for the file rather than one per literal.
    cooked: String,
    /// Where each literal's text sits in `cooked`.
    strs: Vec<(u32, u32)>,
    /// The string tokens whose closing `"` was never written, ascending.
    ///
    /// Such a token holds whatever was left on the line rather than what
    /// somebody wrote, so the parser refuses to read a construct out of it —
    /// see [`Tokens::is_unterminated`]. A file has none of these or one, so
    /// this is a list that is searched rather than a set.
    unterminated: Vec<u32>,
}

/// How a run of string body ended — see [`Lexer::scan_str_body`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum StrEnd {
    /// The closing `"`.
    Quote,
    /// An unescaped `${`, so a template hole follows.
    Hole,
    /// A line break, or the end of the file, with `unterminated-string`
    /// reported. The token holds the rest of the line rather than a literal.
    Unterminated,
}

/// One token as the buffer holds it.
///
/// `pay` is decoded by kind: an index into `ints`, `floats` or `strs`, the
/// scalar value of a character literal, and unread for every other kind. It
/// is three bytes, little-endian, which holds every scalar value and an index
/// up to sixteen million; [`WIDE`] stands for one that does not fit.
#[derive(Clone, Copy)]
struct Record {
    loc: Location,
    kind: TokenKind,
    pay: [u8; 3],
}

/// The payload that says "look in `Tokens::wide`".
const WIDE: u32 = 0x00ff_ffff;

const _: () = assert!(std::mem::size_of::<TokenKind>() == 1);
const _: () = assert!(std::mem::size_of::<Location>() == 8);
const _: () = assert!(std::mem::size_of::<Record>() == 12);
/// `Token` is a view built on demand and never stored, so its width is a
/// register-allocation question rather than a memory one. It is pinned anyway,
/// because a variant that grew past this would mean somebody had put owned
/// data on a token again.
const _: () = assert!(std::mem::size_of::<Token<'_>>() == 32);

impl<'a> Tokens<'a> {
    fn new(src: &'a str, file: FileId) -> Tokens<'a> {
        // Buri source runs about four bytes to the token, comments included,
        // so this is the buffer the file needs rather than the first of a
        // dozen doublings — each of which copied everything written so far.
        let n = src.len().wrapping_div(4).saturating_add(1);
        Tokens {
            src,
            file,
            records: Vec::with_capacity(n),
            wide: Vec::new(),
            ints: Vec::new(),
            floats: Vec::new(),
            cooked: String::new(),
            strs: Vec::new(),
            unterminated: Vec::new(),
        }
    }

    #[inline]
    fn push(&mut self, kind: TokenKind, pay: u32, loc: Location) {
        let mut word = pay;
        if pay >= WIDE {
            self.wide.push((self.records.len() as u32, pay));
            word = WIDE;
        }
        let [a, b, c, _] = word.to_le_bytes();
        self.records.push(Record { loc, kind, pay: [a, b, c] });
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// The kind at `i`, or `Eof` past the end.
    ///
    /// Reading off the end as end-of-file rather than as a missing token is
    /// what lets the parser peek unconditionally; `lex` finishes by pushing an
    /// `Eof` on every path, so in a correct front end the fallback is
    /// unreachable and this is the one place that has to know it.
    pub fn kind(&self, i: usize) -> TokenKind {
        self.records.get(i).map_or(TokenKind::Eof, |r| r.kind)
    }

    /// Whether the token at `i` is a string whose closing `"` is missing.
    ///
    /// The lexer ends such a string at the line break so that the parser
    /// carries on from the next line, which means the token has swallowed
    /// whatever was written after the quote. Nothing read out of it is what
    /// somebody wrote, so the construct around it is abandoned rather than
    /// diagnosed a second time.
    pub fn is_unterminated(&self, i: usize) -> bool {
        u32::try_from(i).is_ok_and(|i| self.unterminated.binary_search(&i).is_ok())
    }

    /// The kind and location of the token at `i`: `Eof` and an empty
    /// location past the end, as [`Tokens::kind`] and [`Tokens::loc`] read.
    pub fn record(&self, i: usize) -> (TokenKind, Location) {
        self.records.get(i).map_or((TokenKind::Eof, Location::default()), |r| (r.kind, r.loc))
    }

    pub fn loc(&self, i: usize) -> Location {
        self.records.get(i).map(|r| r.loc).unwrap_or_default()
    }

    pub fn span(&self, i: usize) -> Span {
        let l = self.loc(i);
        Span { file: self.file, start: l.start, end: l.end }
    }

    /// The source under the token at `i`: an identifier's text, and a numeric
    /// literal's spelling.
    pub fn text(&self, i: usize) -> &'a str {
        let l = self.loc(i);
        self.src.get(l.start as usize..l.end as usize).unwrap_or("")
    }

    fn pay(&self, i: usize) -> usize {
        let Some(r) = self.records.get(i) else { return 0 };
        let [a, b, c] = r.pay;
        let word = u32::from_le_bytes([a, b, c, 0]);
        if word != WIDE {
            return word as usize;
        }
        let at = i as u32;
        match self.wide.binary_search_by_key(&at, |(t, _)| *t) {
            Ok(k) => self.wide.get(k).map_or(0, |(_, pay)| *pay as usize),
            Err(_) => 0,
        }
    }

    pub fn int(&self, i: usize) -> u128 {
        self.ints.get(self.pay(i)).copied().unwrap_or(0)
    }

    pub fn float(&self, i: usize) -> f64 {
        self.floats.get(self.pay(i)).copied().unwrap_or(0.0)
    }

    /// The cooked text of a string literal or template segment.
    pub fn str_at(&self, i: usize) -> &str {
        self.strs
            .get(self.pay(i))
            .and_then(|&(start, end)| self.cooked.get(start as usize..end as usize))
            .unwrap_or("")
    }

    pub fn ch(&self, i: usize) -> char {
        char::from_u32(self.pay(i) as u32).unwrap_or('\0')
    }

    /// The token at `i`, decoded. See [`Token`].
    pub fn token(&self, i: usize) -> Token<'a> {
        match self.kind(i) {
            TokenKind::Ident => Token::Ident(self.text(i)),
            TokenKind::Int => Token::Int(self.int(i)),
            TokenKind::Float => Token::Float(self.float(i)),
            TokenKind::Str => Token::Str(self.str_at(i).to_string()),
            TokenKind::Char => Token::Char(self.ch(i)),
            TokenKind::TemplateHead => Token::TemplateHead(self.str_at(i).to_string()),
            TokenKind::TemplateSpan => Token::TemplateSpan(self.str_at(i).to_string()),
            TokenKind::TemplateTail => Token::TemplateTail(self.str_at(i).to_string()),
            TokenKind::Eof => Token::Eof,
            k => match (k.as_keyword(), k.as_punctuation()) {
                (Some(keyword), _) => Token::Keyword(keyword),
                (_, Some(p)) => Token::Punctuation(p),
                _ => Token::Eof,
            },
        }
    }

    /// Every token, decoded, in source order.
    pub fn tokens(&self) -> impl Iterator<Item = Token<'a>> + '_ {
        (0..self.len()).map(|i| self.token(i))
    }

    /// How a message names the token at `i` — see [`Token::describe`].
    pub fn describe(&self, i: usize) -> String {
        self.token(i).describe(self.text(i))
    }
}

pub struct Lexer<'a> {
    src: &'a [u8],
    text: &'a str,
    pos: usize,
    file: FileId,
    tokens: Tokens<'a>,
    trivia: Vec<(u32, Trivia)>,
    errors: Vec<Diagnostic>,
    /// The open interpolation holes, innermost last, each with the number of
    /// `{` opened inside it and not yet closed.
    ///
    /// A `}` resumes template text exactly when the innermost hole has no
    /// brace of its own open; otherwise it closes one of those braces. A brace
    /// outside every hole is nothing this stack needs to know about, so the
    /// common file — whose braces are all outside templates — never touches
    /// it. An unbalanced `}` is a count that is already zero or a stack that
    /// is empty, and neither can make a later `}` mean something else.
    ///
    /// This replaces a stack with an entry per open brace as well as per hole,
    /// which was a push and a pop for every pair of braces in the file. It
    /// keeps that stack's guarantee: there is one structure, so there is
    /// nothing to disagree with.
    holes: Vec<u32>,
    /// Whether anything is waiting to be attached to the next token: a
    /// documentation line, a comment, or a blank line above it.
    ///
    /// It is exactly `docs.len() > pending_docs || !pending_comments.is_empty()
    /// || blank_before`, kept as one byte so that the test [`Lexer::push`]
    /// makes for every token in the file is one load rather than three. Every
    /// site that can make it true goes through [`Lexer::hold_blank`] or pushes
    /// onto a pending list beside a `self.has_trivia = true;`, and
    /// [`Lexer::attach_trivia`] is the only place that clears it.
    has_trivia: bool,
    /// Every `///` line so far, as the location of its text. The ones from
    /// `pending_docs` on are waiting for the next token.
    docs: Vec<Location>,
    pending_docs: u32,
    pending_docs_blank: bool,
    pending_comments: Vec<Comment>,
    module_docs: Vec<(Location, Span)>,
    blank_before: bool,
    detached: bool,
}

pub struct Lexed<'a> {
    pub tokens: Tokens<'a>,
    /// What was written above a token, keyed by its index in `tokens` and in
    /// ascending order of that index. Only tokens that have something above
    /// them appear.
    pub trivia: Vec<(u32, Trivia)>,
    pub errors: Vec<Diagnostic>,
    /// Every `///` line, as the location of its text — see [`doc_at`]. A
    /// token's [`Trivia::docs`] is a run of these, and the parser hands the
    /// whole list to the tree rather than copying a line out of it.
    pub docs: Vec<Location>,
    /// `//!` lines, as the location of each line's text and the span of the
    /// whole comment, in source order. The parser keeps the ones before the
    /// first item and reports the rest.
    pub module_docs: Vec<(Location, Span)>,
}

impl<'a> Lexed<'a> {
    /// The text of a doc line.
    pub fn doc(&self, at: Location) -> &'a str {
        self.tokens.src.get(at.start as usize..at.end as usize).unwrap_or("")
    }

    /// The text of a token's doc lines.
    pub fn doc_lines(&self, d: Docs) -> impl Iterator<Item = &'a str> + '_ {
        let a = d.start as usize;
        let lines = self.docs.get(a..a.saturating_add(d.len as usize)).unwrap_or(&[]);
        lines.iter().map(|at| self.doc(*at))
    }
}

/// What a source file says to the compiler: each token's kind and spelling,
/// whether a line break comes before it, and every doc comment. Ordinary
/// comments and other whitespace are left out, so editing them leaves this
/// unchanged. `None` when lexing reports an error, since then the bytes matter.
pub fn program_text(text: &str) -> Option<Vec<u8>> {
    let lexed = lex(text, FileId::NONE);
    if !lexed.errors.is_empty() {
        return None;
    }
    let tokens = &lexed.tokens;
    let mut out = Vec::with_capacity(text.len());
    let mut trivia = lexed.trivia.iter().peekable();
    let mut module_docs = lexed.module_docs.iter().peekable();
    let mut previous_end = 0usize;
    for i in 0..tokens.len() {
        let loc = tokens.loc(i);
        // A `//!` line is reported when it follows the first item, so where it sits matters.
        while let Some((at, _)) = module_docs.next_if(|(_, span)| span.start <= loc.start) {
            out.extend_from_slice(format!("\0//!{i}:{}\n", lexed.doc(*at)).as_bytes());
        }
        while let Some((_, above)) = trivia.next_if(|(at, _)| *at as usize == i) {
            for line in lexed.doc_lines(above.docs) {
                out.extend_from_slice(b"\0///");
                out.extend_from_slice(line.as_bytes());
                out.push(b'\n');
            }
        }
        // The first token and the end of the file have no token on the line before.
        let gap = text.get(previous_end..loc.start as usize).unwrap_or("");
        let edge = i == 0 || tokens.kind(i) == TokenKind::Eof;
        out.push(if edge || gap.contains('\n') { b'\n' } else { b' ' });
        out.push(tokens.kind(i) as u8);
        // Length-prefixed, so two token streams can't spell the same bytes.
        let spelling = tokens.text(i).as_bytes();
        out.extend_from_slice(&(spelling.len() as u64).to_le_bytes());
        out.extend_from_slice(spelling);
        previous_end = loc.end as usize;
    }
    for (at, _) in module_docs {
        out.extend_from_slice(format!("\0//!end:{}\n", lexed.doc(*at)).as_bytes());
    }
    Some(out)
}

pub fn lex(text: &str, file: FileId) -> Lexed<'_> {
    let mut l = Lexer {
        src: text.as_bytes(),
        text,
        pos: 0,
        file,
        tokens: Tokens::new(text, file),
        // A file whose every declaration is documented writes a run of
        // comments every few hundred bytes: sized for that, not grown to it.
        trivia: Vec::with_capacity(text.len() / 512),
        errors: Vec::new(),
        holes: Vec::new(),
        has_trivia: false,
        docs: Vec::new(),
        pending_docs: 0,
        pending_docs_blank: false,
        pending_comments: Vec::new(),
        module_docs: Vec::new(),
        blank_before: false,
        detached: false,
    };
    l.run();
    Lexed {
        tokens: l.tokens,
        trivia: l.trivia,
        errors: l.errors,
        docs: l.docs,
        module_docs: l.module_docs,
    }
}

impl<'a> Lexer<'a> {
    fn peek(&self) -> u8 {
        *self.src.get(self.pos).unwrap_or(&0)
    }

    fn peek_at(&self, n: usize) -> u8 {
        *self.src.get(self.pos.saturating_add(n)).unwrap_or(&0)
    }

    fn bump(&mut self) -> u8 {
        let c = self.peek();
        self.pos = self.pos.saturating_add(1);
        c
    }

    /// The source between two offsets, empty if they do not describe one.
    ///
    /// Every offset here is one the lexer walked to, so in a correct lexer
    /// this is `&self.text[a..b]` — but that spelling panics on the one
    /// arrangement of bytes where the lexer is wrong, and the arrangement in
    /// question is "a character outside ASCII", which is not exotic input. A
    /// total accessor makes the failure a short message instead of a crash,
    /// and there is exactly one of it to reason about.
    fn slice(&self, a: usize, b: usize) -> &'a str {
        self.text.get(a..b).unwrap_or("")
    }

    fn span(&self, start: usize) -> Span {
        Span::new(self.file, start, self.pos)
    }

    /// Every lexical error's wording lives on its page. What follows is
    /// `.bind(…)` for each `{placeholder}` the page names.
    /// Hands the diagnostic back so a caller can add what varies.
    fn templated(&mut self, code: &str, span: Span) -> &mut Diagnostic {
        self.errors.push(Diagnostic::templated(code, span));
        self.errors.last_mut().or_ice("the diagnostic just pushed is still there")
    }

    /// Append one token, from `start` to the cursor.
    ///
    /// What was written above it is already attached: every arm of
    /// [`Lexer::run`] that reads a token calls [`Lexer::gap`] first, and pushes
    /// exactly one token after it.
    #[inline]
    fn push(&mut self, kind: TokenKind, pay: u32, start: usize) {
        debug_assert!(!self.has_trivia, "a token pushed without `gap` before it");
        self.tokens.push(kind, pay, Location { start: start as u32, end: self.pos as u32 });
    }


    /// Hand what was written above the next token to the trivia table.
    ///
    /// Almost no token has any of this — a run of comments belongs to a
    /// declaration, not to the hundreds of tokens inside one — so the
    /// arithmetic on five fields lives here rather than in [`Lexer::push`].
    #[cold]
    #[inline(never)]
    fn attach_trivia(&mut self, at: u32) {
        self.trivia.push((
            at,
            Trivia {
                docs: Docs {
                    start: self.pending_docs,
                    len: (self.docs.len() as u32).saturating_sub(self.pending_docs),
                },
                docs_blank: self.pending_docs_blank,
                comments: std::mem::take(&mut self.pending_comments),
                blank_before: self.blank_before,
                detached: self.detached,
            },
        ));
        self.pending_docs = self.docs.len() as u32;
        self.pending_docs_blank = false;
        self.blank_before = false;
        self.detached = false;
        self.has_trivia = false;
    }

    /// Record that a blank line sat above whatever comes next.
    ///
    /// The one way `blank_before` is set, so that it cannot be set without
    /// [`Lexer::has_trivia`] learning about it.
    fn hold_blank(&mut self, blank: bool) {
        self.blank_before = blank;
        self.has_trivia |= blank;
    }

    /// A token whose value lives in a side table: the payload column holds the
    /// index the value was appended at.
    fn push_int(&mut self, v: u128, start: usize) {
        let at = self.tokens.ints.len() as u32;
        self.tokens.ints.push(v);
        self.push(TokenKind::Int, at, start);
    }

    fn push_float(&mut self, v: f64, start: usize) {
        let at = self.tokens.floats.len() as u32;
        self.tokens.floats.push(v);
        self.push(TokenKind::Float, at, start);
    }

    /// A string token whose text is what `scan_str_body` appended to the
    /// cooked text since `from`.
    fn push_text(&mut self, kind: TokenKind, from: usize, start: usize) {
        let at = self.tokens.strs.len() as u32;
        let end = u32::try_from(self.tokens.cooked.len()).unwrap_or(u32::MAX);
        self.tokens.strs.push((u32::try_from(from).unwrap_or(u32::MAX), end));
        self.push(kind, at, start);
    }

    /// The whole lexer: one `match` per thing read.
    ///
    /// Whitespace, comments and every token start are arms of the same
    /// `match`, so deciding what the next byte begins is one jump per token.
    ///
    /// The cursor is a local rather than `self.pos`, so it stays in a register
    /// through the common arms. An arm that hands off to a method writes it to
    /// `self.pos` first and reads it back after.
    fn run(&mut self) {
        use TokenKind::*;
        let src = self.src;
        // The line breaks read since the last comment or token. Two of them
        // with nothing between is a blank line.
        let mut newlines = 0usize;
        // `self.has_trivia || newlines >= 2`: whether the next token has
        // something to settle before it. Kept here, so the test each token
        // makes is one register.
        let mut pending = self.has_trivia;
        let mut pos = self.pos;
        // The token buffer, out of `self` so that its length and capacity
        // stay in registers. An arm that hands off to a method that pushes a
        // token puts it back for the call.
        let mut records = std::mem::take(&mut self.tokens.records);
        // What was written above the token about to be read goes to the
        // trivia table first, keyed by the index the token is about to get.
        macro_rules! settled {
            () => {{
                if pending {
                    self.settle(newlines >= 2, records.len());
                    pending = false;
                }
                newlines = 0;
            }};
        }
        macro_rules! handoff {
            ($call:expr) => {{
                self.tokens.records = records;
                self.pos = pos;
                $call;
                pos = self.pos;
                records = std::mem::take(&mut self.tokens.records);
            }};
        }
        loop {
            let mut c = byte_at(src, pos);
            // One space is what follows most tokens, so it is stepped over
            // here rather than by a trip through the `match`.
            if c == b' ' {
                pos = pos.wrapping_add(1);
                c = byte_at(src, pos);
            }
            let start = pos;
            // Whitespace and comments go round again; every other arm reads
            // one token, and settles what was above it first.
            let kind = match class_of(c) {
                Class::Word => {
                    settled!();
                    let (len, word) = scan_word(src, start);
                    pos = start.wrapping_add(len);
                    let kind = match Word::of(src, start, len, word) {
                        None => Ident,
                        Some(Word::Kind(kind)) => kind,
                        Some(Word::Reserved) => {
                            self.pos = pos;
                            self.reserved(start);
                            Ident
                        }
                    };
                    push_plain(&mut records, kind, start, pos);
                    continue;
                }
                // A run of blanks — an indentation, mostly — is stepped over
                // in a loop of its own rather than one trip round this
                // `match` per byte.
                Class::Blank => {
                    pos = blanks(src, start);
                    continue;
                }
                Class::Newline => {
                    newlines = newlines.wrapping_add(1);
                    pending |= newlines >= 2;
                    pos = blanks(src, start.wrapping_add(1));
                    continue;
                }
                Class::Slash => match byte_at(src, pos.wrapping_add(1)) {
                    b'/' => {
                        self.pos = pos;
                        self.line_comment(start, newlines >= 2);
                        pos = self.pos;
                        newlines = 0;
                        pending = self.has_trivia;
                        continue;
                    }
                    b'*' => {
                        self.pos = pos;
                        self.block_comment(start, newlines >= 2);
                        pos = self.pos;
                        newlines = 0;
                        pending = self.has_trivia;
                        continue;
                    }
                    _ => Slash,
                },
                Class::Nul if start >= src.len() => {
                    if pending {
                        self.settle(newlines >= 2, records.len());
                    }
                    push_plain(&mut records, Eof, start, start);
                    self.tokens.records = records;
                    self.pos = start;
                    return;
                }
                Class::Digit => {
                    settled!();
                    handoff!(self.number(start));
                    continue;
                }
                Class::Quote => {
                    settled!();
                    handoff!(self.string_or_template(start));
                    continue;
                }
                Class::Apostrophe => {
                    settled!();
                    handoff!(self.char_literal(start));
                    continue;
                }
                Class::LBrace => {
                    if let Some(open) = self.holes.last_mut() {
                        *open = open.saturating_add(1);
                    }
                    LBrace
                }
                Class::RBrace => {
                    match self.holes.last_mut() {
                        // The innermost thing open is a hole, so this `}`
                        // resumes template text rather than terminating a
                        // block. The hole stays on the stack until the
                        // template itself ends.
                        Some(0) => {
                            settled!();
                            pos = start.wrapping_add(1);
                            handoff!(self.resume_template(start));
                            continue;
                        }
                        Some(open) => *open = open.saturating_sub(1),
                        // No hole is open. This `}` closes a block, or closes
                        // nothing, which the parser reports where it can say
                        // what was expected instead; either way nothing after
                        // it is mis-lexed.
                        None => {}
                    }
                    RBrace
                }
                Class::One => one_byte(c),
                Class::Colon => pair(src, &mut pos, b':', ColonColon, Colon),
                Class::Dot => pair(src, &mut pos, b'.', DotDot, Dot),
                Class::Equals => match byte_at(src, pos.wrapping_add(1)) {
                    b'>' => wide(&mut pos, FatArrow),
                    b'=' => wide(&mut pos, EqEq),
                    _ => Eq,
                },
                Class::Bang => pair(src, &mut pos, b'=', BangEq, Bang),
                Class::Less => pair(src, &mut pos, b'=', LtEq, Lt),
                // No `>>` token: `Wrapper<Wrapper<Int>>` closes with two `>`.
                Class::Greater => pair(src, &mut pos, b'=', GtEq, Gt),
                Class::Amp => pair(src, &mut pos, b'&', AndAnd, And),
                Class::Pipe => pair(src, &mut pos, b'|', OrOr, Or),
                // `??` is not an operator, and is still one token: read as two
                // `?`s it would be a double `try`, and the parser would report
                // something other than what was written. There is no `?.`
                // token, so `x?.field` is `x` `?` `.` `field`.
                Class::Question => pair(src, &mut pos, b'?', QuestionQuestion, Question),
                Class::Nul | Class::Other => {
                    self.pos = pos;
                    self.unexpected(start);
                    pos = self.pos;
                    continue;
                }
            };
            // A punctuator: one byte, or two where `pair` or `wide` already
            // stepped over the first.
            settled!();
            pos = pos.wrapping_add(1);
            push_plain(&mut records, kind, start, pos);
        }
    }

    /// A word the language reserves, reported where it was written.
    #[cold]
    #[inline(never)]
    fn reserved(&mut self, start: usize) {
        let span = self.span(start);
        let word = self.slice(start, self.pos).to_string();
        self.templated("reserved-word", span).bind("word", word);
    }

    /// A character the grammar has no use for.
    ///
    /// This is where every character outside ASCII arrives — a `×` someone
    /// typed for multiplication, a non-breaking space a word processor left
    /// behind. Those are two, three, or four bytes, so the whole scalar is
    /// taken, which is what makes the message show what was typed and keeps
    /// the next token from starting inside a character. The code point is
    /// spelled out because the two characters most likely to reach here are
    /// invisible.
    #[cold]
    #[inline(never)]
    fn unexpected(&mut self, start: usize) {
        let shown = self.next_char();
        let span = self.span(start);
        self.templated("unexpected-character", span)
            .bind("character", shown.to_string())
            .bind("code_point", format!("{:04X}", shown as u32));
    }

    /// With nothing waiting above the token, a blank line before it is the
    /// token's own; with a run of comments above it, it is the gap between the
    /// run and the token, which is what makes a file header a header rather
    /// than a comment about the first declaration.
    #[cold]
    #[inline(never)]
    fn settle(&mut self, blank: bool, at: usize) {
        if self.run_empty() {
            self.hold_blank(blank);
        } else {
            self.detached = blank;
        }
        if self.has_trivia {
            self.attach_trivia(at as u32);
        }
    }

    /// The column `at` sits at, counting from zero.
    fn column(&self, at: usize) -> u32 {
        let line = self.slice(0, at).rfind('\n').map_or(0, |i| i.saturating_add(1));
        self.slice(line, at).chars().count() as u32
    }

    /// Whether a `///` line is waiting for the next token.
    fn docs_pending(&self) -> bool {
        self.docs.len() as u32 > self.pending_docs
    }

    /// Whether nothing of this token's trivia has been read yet, so a blank
    /// line here is the one above the whole run rather than one inside it.
    fn run_empty(&self) -> bool {
        self.pending_comments.is_empty() && !self.docs_pending()
    }

    /// A `//` comment from `start` to the end of its line. `blank` is whether
    /// a blank line sat above it, which is the token's when nothing of the run
    /// has been read yet and the comment's own paragraph break otherwise.
    fn line_comment(&mut self, start: usize, blank: bool) {
        let is_doc = self.peek_at(2) == b'/' && self.peek_at(3) != b'/';
        // `//!` documents the module rather than the declaration that
        // follows, which is the only comment form that attaches upward. It is
        // legal only before the first token; `check` reports one that appears
        // later, where a reader would take it for a `///` typo.
        let is_module_doc = self.peek_at(2) == b'!';
        self.pos = line_end(self.src, self.pos);
        let raw = self.slice(start, self.pos);
        if is_module_doc {
            let span = self.span(start);
            self.module_docs.push((doc_at(start, raw), span));
        } else if is_doc {
            if self.run_empty() {
                self.hold_blank(blank);
            }
            if !self.docs_pending() {
                self.pending_docs_blank = blank;
            }
            self.docs.push(doc_at(start, raw));
            self.has_trivia = true;
        } else {
            if self.run_empty() {
                self.hold_blank(blank);
            }
            let end = start.saturating_add(trimmed_end(raw).len());
            let column = self.column(start);
            self.pending_comments.push(Comment {
                at: Location { start: start as u32, end: end as u32 },
                blank_before: blank,
                column,
                offset: start as u32,
            });
            self.has_trivia = true;
        }
    }

    /// A `/* */` comment from `start`, nested ones included.
    fn block_comment(&mut self, start: usize, blank: bool) {
        self.pos = self.pos.saturating_add(2);
        let mut depth = 1usize;
        while self.pos < self.src.len() && depth > 0 {
            if self.peek() == b'/' && self.peek_at(1) == b'*' {
                depth = depth.saturating_add(1);
                self.pos = self.pos.saturating_add(2);
            } else if self.peek() == b'*' && self.peek_at(1) == b'/' {
                depth = depth.saturating_sub(1);
                self.pos = self.pos.saturating_add(2);
            } else {
                self.pos = self.pos.saturating_add(1);
            }
        }
        if depth > 0 {
            let span = self.span(start);
            self.templated("unterminated-comment", span);
        }
        if self.run_empty() {
            self.hold_blank(blank);
        }

        let column = self.column(start);
        self.pending_comments.push(Comment {
            at: Location { start: start as u32, end: self.pos as u32 },
            blank_before: blank,
            column,
            offset: start as u32,
        });
        self.has_trivia = true;
    }

    fn number(&mut self, start: usize) {
        if let Some((value, end)) = plain_decimal(self.src, start) {
            self.pos = end;
            self.push_int(u128::from(value), start);
            return;
        }
        // Radix prefixes. `0x`, `0o`, `0b` are integers only.
        if self.peek() == b'0' && matches!(self.peek_at(1), b'x' | b'X' | b'o' | b'O' | b'b' | b'B')
        {
            let radix_char = self.peek_at(1).to_ascii_lowercase();
            let radix: u32 = match radix_char {
                b'x' => 16,
                b'o' => 8,
                _ => 2,
            };
            self.pos = self.pos.saturating_add(2);
            let digits_start = self.pos;
            while self.peek().is_ascii_alphanumeric() || self.peek() == b'_' {
                self.pos = self.pos.saturating_add(1);
            }
            let raw = self.slice(start, self.pos);
            let digits = without_underscores(self.slice(digits_start, self.pos));
            if digits.is_empty() {
                let span = self.span(start);
                self.templated("integer-missing-digits", span).bind("literal", raw);
                self.push_int(0, start);
                return;
            }
            match u128::from_str_radix(&digits, radix) {
                Ok(v) => self.push_int(v, start),
                Err(_) => {
                    let span = self.span(start);
                    self.templated("invalid-integer", span)
                        .bind("literal", raw)
                        .bind("radix", radix.to_string());
                    self.push_int(0, start);
                }
            }
            return;
        }

        while self.peek().is_ascii_digit() || self.peek() == b'_' {
            self.pos = self.pos.saturating_add(1);
        }

        // A FLOAT must begin with a digit, and `pair.0` must lex as three
        // tokens, so a `.` only continues the number when a digit follows it.
        let mut is_float = false;
        if self.peek() == b'.' && self.peek_at(1).is_ascii_digit() {
            is_float = true;
            self.pos = self.pos.saturating_add(1);
            while self.peek().is_ascii_digit() || self.peek() == b'_' {
                self.pos = self.pos.saturating_add(1);
            }
        }
        if matches!(self.peek(), b'e' | b'E') {
            let save = self.pos;
            self.pos = self.pos.saturating_add(1);
            if matches!(self.peek(), b'+' | b'-') {
                self.pos = self.pos.saturating_add(1);
            }
            if self.peek().is_ascii_digit() {
                is_float = true;
                while self.peek().is_ascii_digit() || self.peek() == b'_' {
                    self.pos = self.pos.saturating_add(1);
                }
            } else {
                self.pos = save;
            }
        }

        let raw = self.slice(start, self.pos);
        let clean = without_underscores(raw);
        if is_float {
            match clean.parse::<f64>() {
                Ok(v) => self.push_float(v, start),
                Err(_) => {
                    let span = self.span(start);
                    self.templated("invalid-float", span).bind("literal", raw);
                    self.push_float(0.0, start);
                }
            }
        } else {
            match clean.parse::<u128>() {
                Ok(v) => self.push_int(v, start),
                Err(_) => {
                    let span = self.span(start);
                    self.templated("integer-too-wide", span).bind("literal", raw);
                    self.push_int(0, start);
                }
            }
        }
    }

    /// The four bytes that end a run of ordinary string content. None of them
    /// can appear inside a multi-byte UTF-8 sequence, which is what lets the
    /// run be found by scanning bytes and copied without decoding.
    fn plain_str_byte(c: u8) -> bool {
        !matches!(c, b'"' | b'\\' | b'$' | b'\n')
    }

    /// Scans string body text, stopping at `"`, at an unescaped `${`, or at the
    /// line break that means the closing quote was never written.
    ///
    /// The text goes onto the end of the cooked text, and the caller records
    /// where it started.
    fn scan_str_body(&mut self) -> StrEnd {
        // `chunk` is the start of the run of source that belongs in the result
        // verbatim. A string with no escape is one such run, so the common
        // literal is copied once rather than a character at a time through a
        // fresh UTF-8 decode per character.
        let mut chunk = self.pos;
        loop {
            if self.pos >= self.src.len() {
                self.tokens.cooked.push_str(self.slice(chunk, self.pos));
                let span = Span::new(self.file, self.pos, self.pos);
                self.templated("unterminated-string", span);
                return StrEnd::Unterminated;
            }
            match self.peek() {
                b'"' => {
                    let text = self.slice(chunk, self.pos);
                    self.pos = self.pos.saturating_add(1);
                    self.tokens.cooked.push_str(text);
                    return StrEnd::Quote;
                }
                b'$' if self.peek_at(1) == b'{' => {
                    let text = self.slice(chunk, self.pos);
                    self.pos = self.pos.saturating_add(2);
                    self.tokens.cooked.push_str(text);
                    return StrEnd::Hole;
                }
                b'\\' => {
                    self.tokens.cooked.push_str(self.slice(chunk, self.pos));
                    let start = self.pos;
                    self.pos = self.pos.saturating_add(1);
                    if let Some(c) = self.escape(start) {
                        self.tokens.cooked.push(c);
                    }
                    chunk = self.pos;
                }
                b'\n' => {
                    self.tokens.cooked.push_str(self.slice(chunk, self.pos));
                    let span = Span::new(self.file, self.pos, self.pos.saturating_add(1));
                    self.templated("unterminated-string", span);
                    return StrEnd::Unterminated;
                }
                _ => {
                    // A `$` with no `{` after it is content, so the first step
                    // is unconditional: without it this would stop on the same
                    // byte forever.
                    self.pos = plain_str_end(self.src, self.pos.saturating_add(1));
                }
            }
        }
    }

    fn next_char(&mut self) -> char {
        let ch = self.text.get(self.pos..).and_then(|s| s.chars().next()).unwrap_or('\0');
        self.pos = self.pos.saturating_add(ch.len_utf8());
        ch
    }

    /// Called with `self.pos` just past the backslash.
    fn escape(&mut self, start: usize) -> Option<char> {
        let c = self.bump();
        Some(match c {
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'0' => '\0',
            b'\\' => '\\',
            b'"' => '"',
            b'\'' => '\'',
            b'$' => '$',
            b'u' => {
                if self.peek() != b'{' {
                    let span = self.span(start);
                    self.templated("unbraced-unicode-escape", span);
                    return None;
                }
                self.pos = self.pos.saturating_add(1);
                let ds = self.pos;
                while self.peek().is_ascii_hexdigit() {
                    self.pos = self.pos.saturating_add(1);
                }
                let digits = self.slice(ds, self.pos).to_string();
                if self.peek() != b'}' {
                    let span = self.span(start);
                    self.templated("unterminated-unicode-escape", span);
                    return None;
                }
                self.pos = self.pos.saturating_add(1);
                match u32::from_str_radix(&digits, 16).ok().and_then(char::from_u32) {
                    Some(c) => c,
                    None => {
                        let span = self.span(start);
                        self.templated("invalid-unicode-escape", span).bind("digits", digits);
                        return None;
                    }
                }
            }
            _ => {
                let span = self.span(start);
                let shown = (c as char).to_string();
                self.templated("unknown-escape", span).bind("escape", shown);
                return None;
            }
        })
    }

    fn string_or_template(&mut self, start: usize) {
        self.pos = self.pos.saturating_add(1); // the opening quote
        let from = self.tokens.cooked.len();
        let end = self.scan_str_body();
        if end == StrEnd::Hole {
            self.push_text(TokenKind::TemplateHead, from, start);
            self.holes.push(0);
        } else {
            self.push_text(TokenKind::Str, from, start);
            self.mark(end);
        }
    }

    /// Resumes template text after the `}` that closes a hole.
    fn resume_template(&mut self, start: usize) {
        let from = self.tokens.cooked.len();
        let end = self.scan_str_body();
        if end == StrEnd::Hole {
            self.push_text(TokenKind::TemplateSpan, from, start);
        } else {
            self.push_text(TokenKind::TemplateTail, from, start);
            self.mark(end);
            debug_assert_eq!(self.holes.last(), Some(&0));
            self.holes.pop();
        }
    }

    /// Records the token just pushed as one the parser must not read a
    /// construct out of. Ascending by construction: tokens are pushed in
    /// source order, so [`Tokens::is_unterminated`] can binary-search.
    fn mark(&mut self, end: StrEnd) {
        if end != StrEnd::Unterminated {
            return;
        }
        let at = self.tokens.len().saturating_sub(1);
        if let Ok(at) = u32::try_from(at) {
            self.tokens.unterminated.push(at);
        }
    }

    fn char_literal(&mut self, start: usize) {
        self.pos = self.pos.saturating_add(1);
        let c = if self.peek() == b'\\' {
            let s = self.pos;
            self.pos = self.pos.saturating_add(1);
            self.escape(s).unwrap_or('\0')
        } else if self.pos >= self.src.len() || self.peek() == b'\n' {
            let span = self.span(start);
            self.templated("unterminated-character", span);
            self.push(TokenKind::Char, 0, start);
            return;
        } else {
            self.next_char()
        };
        if self.peek() != b'\'' {
            let span = self.span(start);
            self.templated("character-literal-length", span);
            // Recover by skipping to the closing quote if one is nearby.
            while self.pos < self.src.len() && self.peek() != b'\'' && self.peek() != b'\n' {
                self.pos = self.pos.saturating_add(1);
            }
        }
        if self.peek() == b'\'' {
            self.pos = self.pos.saturating_add(1);
        }
        self.push(TokenKind::Char, c as u32, start);
    }
}

/// A numeric literal's digits with the group separators taken out.
///
/// Borrowed when there are none, which is nearly every literal written: the
/// copy used to be made whether or not there was anything to take out.
fn without_underscores(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains('_') {
        std::borrow::Cow::Owned(s.chars().filter(|c| *c != '_').collect())
    } else {
        std::borrow::Cow::Borrowed(s)
    }
}

/// What a byte begins, for [`Lexer::run`]'s dispatch: one load and one jump
/// for every token, where a `match` on the byte tested the letters first and
/// then jumped.
#[derive(Clone, Copy)]
#[repr(u8)]
enum Class {
    Word,
    Blank,
    Newline,
    Slash,
    Nul,
    Digit,
    Quote,
    Apostrophe,
    LBrace,
    RBrace,
    /// A punctuator that is one byte whatever follows: [`one_byte`].
    One,
    Colon,
    Dot,
    Equals,
    Bang,
    Less,
    Greater,
    Amp,
    Pipe,
    Question,
    Other,
}

const CLASSES: [Class; 256] = {
    let mut table = [Class::Other; 256];
    let mut c = 0usize;
    while c < 256 {
        let class = match c as u8 {
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => Class::Word,
            b' ' | b'\t' | b'\r' => Class::Blank,
            b'\n' => Class::Newline,
            b'/' => Class::Slash,
            0 => Class::Nul,
            b'0'..=b'9' => Class::Digit,
            b'"' => Class::Quote,
            b'\'' => Class::Apostrophe,
            b'{' => Class::LBrace,
            b'}' => Class::RBrace,
            b'(' | b')' | b'[' | b']' | b',' | b';' | b'@' | b'+' | b'-' | b'*' | b'%' | b'^' | b'~' => {
                Class::One
            }
            b':' => Class::Colon,
            b'.' => Class::Dot,
            b'=' => Class::Equals,
            b'!' => Class::Bang,
            b'<' => Class::Less,
            b'>' => Class::Greater,
            b'&' => Class::Amp,
            b'|' => Class::Pipe,
            b'?' => Class::Question,
            _ => Class::Other,
        };
        if let Some((_, [entry, ..])) = table.split_at_mut_checked(c) {
            *entry = class;
        }
        c = c.wrapping_add(1);
    }
    table
};

#[inline]
fn class_of(c: u8) -> Class {
    CLASSES.get(usize::from(c)).copied().unwrap_or(Class::Other)
}

/// The punctuator a [`Class::One`] byte is.
#[inline]
fn one_byte(c: u8) -> TokenKind {
    use TokenKind::*;
    match c {
        b'(' => LParen,
        b')' => RParen,
        b'[' => LBracket,
        b']' => RBracket,
        b',' => Comma,
        b';' => Semi,
        b'@' => At,
        b'+' => Plus,
        b'-' => Minus,
        b'*' => Star,
        b'%' => Percent,
        b'^' => Caret,
        _ => Tilde,
    }
}

/// Append a token with no payload: a word or a punctuator, nine tokens in ten.
#[inline]
fn push_plain(records: &mut Vec<Record>, kind: TokenKind, start: usize, end: usize) {
    let loc = Location { start: start as u32, end: end as u32 };
    records.push(Record { loc, kind, pay: [0; 3] });
}

/// The byte at `at`, or 0 past the end.
#[inline]
fn byte_at(src: &[u8], at: usize) -> u8 {
    src.get(at).copied().unwrap_or(0)
}

/// A punctuator that is `two` when `next` follows its first byte at `*pos`,
/// with `*pos` stepped onto that second byte, and `one` otherwise.
#[inline]
fn pair(src: &[u8], pos: &mut usize, next: u8, two: TokenKind, one: TokenKind) -> TokenKind {
    if byte_at(src, pos.wrapping_add(1)) == next {
        wide(pos, two)
    } else {
        one
    }
}

/// A two-byte punctuator: step over the first byte, and the caller steps over
/// the second as it would over a one-byte one.
#[inline]
fn wide(pos: &mut usize, kind: TokenKind) -> TokenKind {
    *pos = pos.wrapping_add(1);
    kind
}

/// Eight copies of a byte, one per lane of a `u64`.
const fn lanes(b: u8) -> u64 {
    (b as u64).wrapping_mul(0x0101_0101_0101_0101)
}

const HIGH: u64 = lanes(0x80);

/// How long the word starting at `start` is, and its first eight bytes as
/// one integer, zero above the last: the key [`Word::of`] looks it up by.
/// The byte at `start` begins a word, so it is at least one long.
///
/// Most words are short, so the bytes after the first are tested one at a
/// time against [`IDENT_CONTINUE`], inside the eight-byte load the key comes
/// from.
#[inline]
fn scan_word(src: &[u8], start: usize) -> (usize, u64) {
    let Some(chunk) = src.get(start..).and_then(|rest| rest.first_chunk::<8>()) else {
        return scan_word_at_the_end(src, start);
    };
    let mut len = 1usize;
    while let Some(&c) = chunk.get(len) {
        if !is_ident_continue(c) {
            let key = u64::from_le_bytes(*chunk) & (1u64 << len.wrapping_mul(8)).wrapping_sub(1);
            return (len, key);
        }
        len = len.wrapping_add(1);
    }
    let mut at = start.wrapping_add(8);
    while src.get(at).is_some_and(|c| is_ident_continue(*c)) {
        at = at.wrapping_add(1);
    }
    (at.wrapping_sub(start), u64::from_le_bytes(*chunk))
}

/// [`scan_word`] in the last eight bytes of the file.
#[cold]
fn scan_word_at_the_end(src: &[u8], start: usize) -> (usize, u64) {
    let mut at = start.wrapping_add(1);
    while src.get(at).is_some_and(|c| is_ident_continue(*c)) {
        at = at.wrapping_add(1);
    }
    (at.wrapping_sub(start), word_key(src.get(start..at).unwrap_or(&[])))
}

/// The integer literal at `start` and where it ends, when it is the common
/// kind: at most nineteen decimal digits, which a `u64` holds, and nothing
/// after them that would make it something else. Read in the one pass that
/// finds its end. `None` sends it to [`Lexer::number`]'s general path.
#[inline]
fn plain_decimal(src: &[u8], start: usize) -> Option<(u64, usize)> {
    let mut value = 0u64;
    let mut at = start;
    while let Some(d) = src.get(at).filter(|c| c.is_ascii_digit()) {
        if at.wrapping_sub(start) == 19 {
            return None;
        }
        value = value.wrapping_mul(10).wrapping_add(u64::from(d.wrapping_sub(b'0')));
        at = at.wrapping_add(1);
    }
    // A separator, a fraction, an exponent or a radix prefix: the general path.
    match byte_at(src, at) {
        b'_' | b'.' | b'e' | b'E' | b'x' | b'X' | b'o' | b'O' | b'b' | b'B' => None,
        _ => Some((value, at)),
    }
}

/// Where the run of ordinary string content from `at` ends: at the first
/// `"`, `\`, `$` or line break, eight bytes at a time. None of the four can
/// be part of a longer UTF-8 sequence.
#[inline]
fn plain_str_end(src: &[u8], at: usize) -> usize {
    let mut at = at;
    while let Some(chunk) = src.get(at..).and_then(|rest| rest.first_chunk::<8>()) {
        let w = u64::from_le_bytes(*chunk);
        let stops = (*b"\"\\$\n")
            .map(|b| {
                let x = w ^ lanes(b);
                x.wrapping_sub(lanes(1)) & !x & HIGH
            })
            .iter()
            .fold(0, |all, one| all | one);
        // A borrow only ever marks a lane above a true match, in the same
        // test, so the lowest mark of all four is the first stop.
        if stops != 0 {
            return at.wrapping_add((stops.trailing_zeros() / 8) as usize);
        }
        at = at.wrapping_add(8);
    }
    while src.get(at).is_some_and(|c| Lexer::plain_str_byte(*c)) {
        at = at.wrapping_add(1);
    }
    at
}

/// Where the line that `at` is on ends: the next line break, or the end of
/// the file. Eight bytes at a time; a line break is never part of a longer
/// UTF-8 sequence, so a byte search finds what a `char` search would.
#[inline]
fn line_end(src: &[u8], at: usize) -> usize {
    let mut at = at;
    while let Some(chunk) = src.get(at..).and_then(|rest| rest.first_chunk::<8>()) {
        let x = u64::from_le_bytes(*chunk) ^ lanes(b'\n');
        // The lowest lane that was a line break; a borrow only ever marks a
        // lane above one, so the lowest mark is exact.
        let breaks = x.wrapping_sub(lanes(1)) & !x & HIGH;
        if breaks != 0 {
            return at.wrapping_add((breaks.trailing_zeros() / 8) as usize);
        }
        at = at.wrapping_add(8);
    }
    while src.get(at).is_some_and(|c| *c != b'\n') {
        at = at.wrapping_add(1);
    }
    at
}

/// `s` without its trailing whitespace, as `str::trim_end` gives it. A line
/// almost always ends in an ASCII character that is not a blank, and then
/// there is nothing to decode.
#[inline]
fn trimmed_end(s: &str) -> &str {
    match s.as_bytes().last() {
        Some(&c) if c.is_ascii() && !c.is_ascii_whitespace() && c != 0x0b => s,
        _ => s.trim_end(),
    }
}

/// Where the run of spaces, tabs and carriage returns from `at` ends. Spaces
/// are stepped over eight at a time, since an indentation is mostly them.
#[inline]
fn blanks(src: &[u8], at: usize) -> usize {
    let mut at = at;
    while let Some(chunk) = src.get(at..).and_then(|rest| rest.first_chunk::<8>()) {
        let other = u64::from_le_bytes(*chunk) ^ lanes(b' ');
        if other != 0 {
            at = at.wrapping_add((other.trailing_zeros() / 8) as usize);
            break;
        }
        at = at.wrapping_add(8);
    }
    while matches!(src.get(at), Some(b' ' | b'\t' | b'\r')) {
        at = at.wrapping_add(1);
    }
    at
}

/// Which bytes continue a word: a letter, a digit, or `_`. One load per byte
/// of every identifier in the file, against the three range tests and an
/// equality a predicate makes. Which bytes *start* one is an arm of
/// [`Lexer::run`]'s `match`.
const IDENT_CONTINUE: [bool; 256] = {
    let mut table = [false; 256];
    let mut c = 0usize;
    while c < 256 {
        let b = c as u8;
        if let Some((_, [entry, ..])) = table.split_at_mut_checked(c) {
            *entry = b == b'_' || b.is_ascii_alphanumeric();
        }
        c = c.wrapping_add(1);
    }
    table
};

fn is_ident_continue(c: u8) -> bool {
    IDENT_CONTINUE.get(usize::from(c)).copied().unwrap_or(false)
}

#[cfg(test)]
mod keyword_tests {
    use super::Keyword;

    /// `ALL` must list every variant. `text` matches exhaustively, so the
    /// compiler catches a missing variant there; this catches a variant that
    /// exists but was left out of `ALL`.
    #[test]
    fn all_lists_every_keyword() {
        let mut texts: Vec<&str> = Keyword::ALL.iter().map(|k| k.text()).collect();
        texts.sort();
        texts.dedup();
        assert_eq!(texts.len(), Keyword::ALL.len(), "`ALL` repeats a keyword");
        // `self` and `Self` differ only in case, so the count is the guard.
        assert_eq!(Keyword::ALL.len(), 25, "a keyword was added without updating `ALL`");
    }

    /// The lexer finds a keyword through its own table, [`super::WORDS`], so
    /// each keyword has to be in it and lex to its own kind.
    #[test]
    fn every_keyword_lexes_as_itself() {
        // Alone, and with enough after it that the key is read eight bytes at
        // a time; and one letter longer, which is an identifier.
        for k in Keyword::ALL {
            for text in [k.text().to_string(), format!("{} padding", k.text())] {
                let l = super::lex(&text, crate::diagnostics::FileId(0));
                assert_eq!(l.tokens.kind(0), super::TokenKind::of_keyword(*k), "{text}");
            }
            let longer = format!("{}x padding", k.text());
            let l = super::lex(&longer, crate::diagnostics::FileId(0));
            assert_eq!(l.tokens.kind(0), super::TokenKind::Ident, "{longer}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `punctuation` reported the byte it had bumped past, and every character
    /// outside ASCII is more than one byte — so a stray `×`, an emoji, or a
    /// non-breaking space out of a word processor panicked the lexer on a
    /// `String` index that was not a character boundary. The message names the
    /// code point because the two most likely to arrive are invisible.
    #[test]
    fn a_character_outside_ascii_is_one_diagnostic_naming_it() {
        for (text, wanted) in [
            ("5 × 3", "`×` (U+00D7)"),
            ("🙂", "`🙂` (U+1F642)"),
            ("a\u{a0}b", "(U+00A0)"),
            ("\u{0}", "(U+0000)"),
            ("\u{200f}", "(U+200F)"),
        ] {
            let l = lex(text, FileId(0));
            let messages: Vec<&str> = l.errors.iter().map(|e| e.message.as_str()).collect();
            assert!(
                messages.iter().any(|m| m.contains(wanted)),
                "lexing {text:?} said {messages:?}, not {wanted}"
            );
        }
    }

    /// Every offset the lexer slices at has to be one it walked to. A file made
    /// only of characters it does not recognise is where that stopped being
    /// true.
    #[test]
    fn a_file_of_nothing_but_unrecognised_characters_lexes() {
        let text = "×÷≠🙂\u{a0}\u{200f}—“”";
        let l = lex(text, FileId(0));
        assert_eq!(l.errors.len(), text.chars().count(), "one per character");
        assert_eq!(l.tokens.tokens().last(), Some(Token::Eof));
    }

    fn tokens(src: &str) -> Vec<Token<'_>> {
        let l = lex(src, FileId(0));
        assert!(l.errors.is_empty(), "unexpected errors: {:?}", l.errors);
        l.tokens.tokens().collect()
    }

    /// A payload too wide for a record's three bytes goes to the side table
    /// and comes back whole, and its neighbours are not disturbed.
    #[test]
    fn a_payload_past_three_bytes_comes_back_whole() {
        let mut t = Tokens::new("", FileId(0));
        let at = Location { start: 0, end: 0 };
        t.push(TokenKind::Int, 7, at);
        t.push(TokenKind::Int, super::WIDE, at);
        t.push(TokenKind::Int, u32::MAX, at);
        t.push(TokenKind::Char, 0x1F642, at);
        assert_eq!([t.pay(0), t.pay(1), t.pay(2)], [7, super::WIDE as usize, u32::MAX as usize]);
        assert_eq!(t.ch(3), '🙂');
    }

    #[test]
    fn tuple_access_is_three_tokens() {
        // `pair.0` must not lex as IDENT FLOAT — this is why a float literal
        // has to begin with a digit (design/grammar-rationale.md 12.14).
        assert_eq!(
            tokens("pair.0"),
            vec![
                Token::Ident("pair"),
                Token::Punctuation(Punctuation::Dot),
                Token::Int(0),
                Token::Eof
            ]
        );
    }

    #[test]
    fn nested_generics_close_with_two_gt() {
        let t = tokens("Foo<Bar<Int>>");
        let shifted = [Token::Punctuation(Punctuation::Gt), Token::Punctuation(Punctuation::Gt)];
        assert_eq!(t[t.len() - 3..t.len() - 1], shifted);
    }

    #[test]
    fn the_known_wart_still_lexes_as_documented() {
        // `Foo<Bar<Int>>= x` lexes `>` `>=`. Documented in grammar.ebnf.
        let t = tokens("Foo<Bar<Int>>= x");
        assert!(t.contains(&Token::Punctuation(Punctuation::GtEq)));
    }

    #[test]
    fn block_comments_nest() {
        assert_eq!(tokens("/* a /* b */ c */ x"), vec![Token::Ident("x"), Token::Eof]);
    }

    #[test]
    fn template_holes_track_brace_depth() {
        // The `}` closing the block inside the hole must not end the template.
        let t = tokens(r#""a${ { let x = 1; x } }b""#);
        assert_eq!(t[0], Token::TemplateHead("a".into()));
        assert_eq!(t[t.len() - 2], Token::TemplateTail("b".into()));
    }

    /// A `}` that closes nothing used to clamp a counter with `saturating_sub`,
    /// leaving the counter and the stack of open interpolations describing two
    /// different nestings. There is no counter now — the depth is the stack's
    /// length — so an unbalanced `}` is a `pop` that finds nothing, and the
    /// template after it still lexes as a template.
    #[test]
    fn an_unbalanced_brace_does_not_derail_a_later_template() {
        let t = tokens(r#"} { let s = "a${x}b"; }"#);
        assert!(
            t.contains(&Token::TemplateHead("a".into())),
            "the template head was swallowed: {t:?}"
        );
        assert!(
            t.contains(&Token::TemplateTail("b".into())),
            "the template never ended: {t:?}"
        );
        assert_eq!(t.last(), Some(&Token::Eof));
    }

    #[test]
    fn multiple_holes() {
        let t = tokens(r#""${a}m${b}s""#);
        assert_eq!(t[0], Token::TemplateHead("".into()));
        assert_eq!(t[2], Token::TemplateSpan("m".into()));
        assert_eq!(t[4], Token::TemplateTail("s".into()));
    }

    #[test]
    fn escaped_dollar_is_not_a_hole() {
        assert_eq!(tokens(r#""\$19.05""#), vec![Token::Str("$19.05".into()), Token::Eof]);
    }

    #[test]
    fn radix_and_separators() {
        assert_eq!(tokens("0xFF"), vec![Token::Int(255), Token::Eof]);
        assert_eq!(tokens("0o755"), vec![Token::Int(0o755), Token::Eof]);
        assert_eq!(tokens("0b1010_0110"), vec![Token::Int(0b1010_0110), Token::Eof]);
        assert_eq!(tokens("1_000_000"), vec![Token::Int(1_000_000), Token::Eof]);
    }

    #[test]
    fn floats_need_a_leading_digit() {
        assert_eq!(tokens("1.0e-9"), vec![Token::Float(1.0e-9), Token::Eof]);
        assert_eq!(tokens("6.02e23"), vec![Token::Float(6.02e23), Token::Eof]);
    }

    #[test]
    fn reserved_words_are_rejected() {
        let l = lex("let while = 1;", FileId(0));
        assert!(l.errors.iter().any(|e| e.message.contains("reserved")));
    }

    #[test]
    fn underscore_is_its_own_token() {
        assert_eq!(tokens("_"), vec![Token::Punctuation(Punctuation::Underscore), Token::Eof]);
        assert_eq!(tokens("_x"), vec![Token::Ident("_x"), Token::Eof]);
    }

    #[test]
    fn no_question_dot_token() {
        assert_eq!(
            tokens("x?.f"),
            vec![
                Token::Ident("x"),
                Token::Punctuation(Punctuation::Question),
                Token::Punctuation(Punctuation::Dot),
                Token::Ident("f"),
                Token::Eof
            ]
        );
    }
}

/// Where a doc line's text is: what [`doc_body`] keeps of the comment `raw`,
/// written at `start`, as a location rather than a copy.
fn doc_at(start: usize, raw: &str) -> Location {
    let after = raw.get(3..).unwrap_or("");
    let body = after.strip_prefix(' ').unwrap_or(after);
    let from = start.saturating_add(raw.len().saturating_sub(body.len()));
    let to = from.saturating_add(trimmed_end(body).len());
    Location { start: from as u32, end: to as u32 }
}

/// The text of a `///` or `//!` line, after the marker.
///
/// One leading space is the separator between the marker and the prose and
/// comes off; everything after that is content. Trimming further would be
/// wrong: a fenced code block inside a doc comment is indented relative to the
/// fence, and `trim()` — which this used to do — flattened it, so an example
/// with a nested block came out unparseable.
pub fn doc_body(after_marker: &str) -> String {
    let s = after_marker.strip_prefix(' ').unwrap_or(after_marker);
    s.trim_end().to_string()
}
