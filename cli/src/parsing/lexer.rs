//! The lexer.
//!
//! Follows the LEXICAL GRAMMAR section of `grammar.ebnf`. Two things about it
//! are worth knowing:
//!
//! * There is no `<<`, `>>`, or `?.` token. Their absence is what keeps
//!   `Wrapper<Wrapper<Int>>` from mis-lexing and `x?.field` from needing a
//!   token splitter (SPEC 12.6).
//! * Template interpolation is the only mode-dependent part. The lexer keeps
//!   one stack of what is currently open — a brace or an interpolation hole —
//!   so a `}` closing a block inside a hole is told apart from the `}` that
//!   resumes template text by which of the two is on top.

use crate::diagnostics::{Diagnostic, FileId, Invariant as _, Span};
use crate::parsing::flat::Loc;

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

    /// What an identifier-shaped word is.
    ///
    /// One `match` over the keywords *and* the reserved words rather than a
    /// keyword lookup followed by a scan of [`RESERVED`]: the scan ran for
    /// every ordinary identifier, which is most words in a file, and rustc
    /// lowers a single `match` on a `&str` to a switch on the length and a
    /// short chain of comparisons within it.
    fn from_str(s: &str) -> Option<Word> {
        Some(Word::Keyword(match s {
            "async" | "await" | "break" | "continue" | "do" | "in" | "is" | "loop" | "module"
            | "mut" | "opaque" | "panic" | "pub" | "return" | "unreachable" | "use" | "when"
            | "where" | "while" | "with" | "yield" => return Some(Word::Reserved),
            "as" => Keyword::As,
            "const" => Keyword::Const,
            "context" => Keyword::Context,
            "ctx" => Keyword::Ctx,
            "derive" => Keyword::Derive,
            "effect" => Keyword::Effect,
            "else" => Keyword::Else,
            "enum" => Keyword::Enum,
            "export" => Keyword::Export,
            "false" => Keyword::False,
            "fn" => Keyword::Fn,
            "for" => Keyword::For,
            "from" => Keyword::From,
            "if" => Keyword::If,
            "impl" => Keyword::Impl,
            "import" => Keyword::Import,
            "let" => Keyword::Let,
            "match" => Keyword::Match,
            "self" => Keyword::SelfValue,
            "Self" => Keyword::SelfType,
            "struct" => Keyword::Struct,
            "test" => Keyword::Test,
            "trait" => Keyword::Trait,
            "true" => Keyword::True,
            "type" => Keyword::Type,
            _ => return None,
        }))
    }
}

/// What [`Keyword::from_str`] found: a keyword, or a word reserved but unused
/// in v0.3 and rejected by the lexer so that later versions can claim it
/// without breaking source compatibility.
enum Word {
    Keyword(Keyword),
    Reserved,
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
    pub text: String,
    /// Whether a blank line sat immediately above this comment.
    pub blank_before: bool,
    /// The column its first character was written at, so a formatter can move
    /// the whole of a `/* … */` and keep the shape inside it.
    pub column: u32,
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
    /// Doc comment lines (`///`) immediately preceding this token.
    pub docs: Vec<String>,
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

/// The token buffer: three parallel columns and three sparse side tables.
///
/// A token used to be a forty-eight-byte record — a tagged union wide enough
/// for a `u128` beside a `Span` — and the buffer is the largest thing the
/// front end builds, written once by the lexer and read once by the parser. Of
/// those forty-eight bytes the parser reads one for almost every token it
/// looks at, so the buffer is columns: the kind stream it walks is dense, the
/// spans it takes on a `bump` are their own array, and the value of a literal
/// — which fewer than one token in ten has — is an index into a table beside
/// them rather than a hole in every token that is not one.
///
/// Two consequences beyond the width. Nothing in the three columns owns
/// anything, so dropping the buffer is three `free`s rather than a walk over
/// every token asking whether it holds a `String`; and an identifier is not in
/// the buffer at all, because its text is the source under its own span.
///
/// The widths are pinned here rather than left to whatever a new field happens
/// to cost. A field that is empty on almost every token belongs in a side
/// table keyed by token index, not in a fourth column.
pub struct Tokens<'a> {
    src: &'a str,
    file: FileId,
    kinds: Vec<TokenKind>,
    locs: Vec<Loc>,
    /// Decoded by kind: an index into `ints`, `floats` or `strs`, the scalar
    /// value of a character literal, and unread for every other kind.
    pays: Vec<u32>,
    ints: Vec<u128>,
    floats: Vec<f64>,
    /// Cooked text — a string literal's contents, a template segment's. The
    /// only owned text the buffer holds, and the only reason it is not `Copy`
    /// throughout.
    strs: Vec<String>,
}

/// What one token costs in the three columns.
const BYTES_PER_TOKEN: usize = std::mem::size_of::<TokenKind>()
    .saturating_add(std::mem::size_of::<Loc>())
    .saturating_add(std::mem::size_of::<u32>());

const _: () = assert!(std::mem::size_of::<TokenKind>() == 1);
const _: () = assert!(std::mem::size_of::<Loc>() == 8);
const _: () = assert!(BYTES_PER_TOKEN == 13);
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
            kinds: Vec::with_capacity(n),
            locs: Vec::with_capacity(n),
            pays: Vec::with_capacity(n),
            ints: Vec::new(),
            floats: Vec::new(),
            strs: Vec::new(),
        }
    }

    #[inline]
    fn push(&mut self, kind: TokenKind, pay: u32, loc: Loc) {
        self.kinds.push(kind);
        self.locs.push(loc);
        self.pays.push(pay);
    }

    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    /// The kind at `i`, or `Eof` past the end.
    ///
    /// Reading off the end as end-of-file rather than as a missing token is
    /// what lets the parser peek unconditionally; `lex` finishes by pushing an
    /// `Eof` on every path, so in a correct front end the fallback is
    /// unreachable and this is the one place that has to know it.
    pub fn kind(&self, i: usize) -> TokenKind {
        self.kinds.get(i).copied().unwrap_or(TokenKind::Eof)
    }

    pub fn loc(&self, i: usize) -> Loc {
        self.locs.get(i).copied().unwrap_or_default()
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
        self.pays.get(i).copied().unwrap_or(0) as usize
    }

    pub fn int(&self, i: usize) -> u128 {
        self.ints.get(self.pay(i)).copied().unwrap_or(0)
    }

    pub fn float(&self, i: usize) -> f64 {
        self.floats.get(self.pay(i)).copied().unwrap_or(0.0)
    }

    /// The cooked text of a string literal or template segment.
    pub fn str_at(&self, i: usize) -> &str {
        self.strs.get(self.pay(i)).map_or("", String::as_str)
    }

    /// The same, moved out of the buffer.
    ///
    /// The parser owns the stream outright and no consumed token's text is
    /// read twice, so the copy the lexer already made is the one the tree
    /// keeps. See `Parser::take_text` for the one case — a speculative parse —
    /// where that is not true.
    pub fn take_str(&mut self, i: usize) -> String {
        let at = self.pay(i);
        self.strs.get_mut(at).map(std::mem::take).unwrap_or_default()
    }

    pub fn ch(&self, i: usize) -> char {
        char::from_u32(self.pays.get(i).copied().unwrap_or(0)).unwrap_or('\0')
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

/// One thing a `}` could be closing, innermost last.
///
/// This was two coupled fields — a `Vec<u32>` of the brace depth each open
/// interpolation began at, and a `u32` counter — which had to agree for the
/// lexer to tell a block's `}` from the one that resumes template text. An
/// unbalanced `}` clamped the counter with `saturating_sub`, leaving it saying
/// a smaller depth than the interpolations recorded, and a later `}` closing a
/// genuine block was then read as "resume the template" — turning the rest of
/// the file into string content. One stack cannot disagree with itself: the
/// depth *is* its length, and an unbalanced `}` is a `pop` that finds nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LexMode {
    /// An open `{`. Its `}` is a block terminator.
    Braces,
    /// An open interpolation hole. Its `}` resumes the template text, and it
    /// is popped by the `}"` that ends the template rather than by the `}`
    /// that ends the hole, because a template may have several holes.
    Interpolation,
}

pub struct Lexer<'a> {
    src: &'a [u8],
    text: &'a str,
    pos: usize,
    file: FileId,
    tokens: Tokens<'a>,
    trivia: Vec<(u32, Trivia)>,
    errors: Vec<Diagnostic>,
    /// What is currently open, innermost last.
    modes: Vec<LexMode>,
    /// Whether anything is waiting to be attached to the next token: a
    /// documentation line, a comment, or a blank line above it.
    ///
    /// It is exactly `!pending_docs.is_empty() || !pending_comments.is_empty()
    /// || blank_before`, kept as one byte so that the test [`Lexer::push`]
    /// makes for every token in the file is one load rather than three. Every
    /// site that can make it true goes through [`Lexer::hold_blank`] or pushes
    /// onto a pending list beside a `self.has_trivia = true;`, and
    /// [`Lexer::attach_trivia`] is the only place that clears it.
    has_trivia: bool,
    pending_docs: Vec<String>,
    pending_docs_blank: bool,
    pending_comments: Vec<Comment>,
    module_docs: Vec<(String, Span)>,
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
    /// `//!` lines, with the span of each, in source order. The parser keeps
    /// the ones before the first item and reports the rest.
    pub module_docs: Vec<(String, Span)>,
}

pub fn lex(text: &str, file: FileId) -> Lexed<'_> {
    let mut l = Lexer {
        src: text.as_bytes(),
        text,
        pos: 0,
        file,
        tokens: Tokens::new(text, file),
        trivia: Vec::new(),
        errors: Vec::new(),
        modes: Vec::new(),
        has_trivia: false,
        pending_docs: Vec::new(),
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

    /// Every lexical error carries the edit that resolves it. `fix` is a
    /// parameter rather than something a caller may add later, so a new error
    /// site cannot forget one.
    /// Hands the diagnostic back so a caller can name the rule it enforces.
    fn err(
        &mut self,
        span: Span,
        msg: impl Into<String>,
        fix: impl Into<String>,
    ) -> &mut Diagnostic {
        self.errors.push(Diagnostic::error(span, msg).with_fix(fix));
        // Pushed on the line above, so there is a last one.
        self.errors.last_mut().or_ice("the diagnostic just pushed is still there")
    }

    /// Append one token.
    ///
    /// This runs once per token in the file and is the lexer's whole write
    /// side, so it is three stores and one predictable branch: everything
    /// about the rare token that has something written above it is behind
    /// [`Lexer::has_trivia`] and outlined, because a body large enough to be
    /// worth a call is a body the ten call sites pay a call for.
    #[inline]
    fn push(&mut self, kind: TokenKind, pay: u32, start: usize) {
        if self.has_trivia {
            self.attach_trivia();
        }
        self.tokens.push(kind, pay, Loc { start: start as u32, end: self.pos as u32 });
    }

    /// Hand what was written above the next token to the trivia table.
    ///
    /// Almost no token has any of this — a run of comments belongs to a
    /// declaration, not to the hundreds of tokens inside one — so the
    /// arithmetic on five fields lives here rather than in [`Lexer::push`].
    #[cold]
    #[inline(never)]
    fn attach_trivia(&mut self) {
        let at = self.tokens.len() as u32;
        self.trivia.push((
            at,
            Trivia {
                docs: std::mem::take(&mut self.pending_docs),
                docs_blank: self.pending_docs_blank,
                comments: std::mem::take(&mut self.pending_comments),
                blank_before: self.blank_before,
                detached: self.detached,
            },
        ));
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

    fn push_text(&mut self, kind: TokenKind, body: String, start: usize) {
        let at = self.tokens.strs.len() as u32;
        self.tokens.strs.push(body);
        self.push(kind, at, start);
    }

    fn run(&mut self) {
        loop {
            self.skip_trivia();
            if self.pos >= self.src.len() {
                let start = self.pos;
                self.push(TokenKind::Eof, 0, start);
                return;
            }
            let start = self.pos;
            let c = self.peek();
            match c {
                b'0'..=b'9' => self.number(start),
                b'"' => self.string_or_template(start),
                b'\'' => self.char_literal(start),
                c if is_ident_start(c) => self.ident(start),
                _ => self.punctuation(start),
            }
        }
    }

    /// The column `at` sits at, counting from zero.
    fn column(&self, at: usize) -> u32 {
        let line = self.slice(0, at).rfind('\n').map_or(0, |i| i.saturating_add(1));
        self.slice(line, at).chars().count() as u32
    }

    /// Whether nothing of this token's trivia has been read yet, so a blank
    /// line here is the one above the whole run rather than one inside it.
    fn run_empty(&self) -> bool {
        self.pending_comments.is_empty() && self.pending_docs.is_empty()
    }

    fn skip_trivia(&mut self) {
        let mut newlines = 0usize;
        loop {
            match self.peek() {
                b' ' | b'\t' | b'\r' => {
                    self.pos = self.pos.saturating_add(1);
                }
                b'\n' => {
                    newlines = newlines.saturating_add(1);
                    self.pos = self.pos.saturating_add(1);
                }
                b'/' if self.peek_at(1) == b'/' => {
                    let start = self.pos;
                    let is_doc = self.peek_at(2) == b'/' && self.peek_at(3) != b'/';
                    // `//!` documents the module rather than the declaration
                    // that follows, which is the only comment form that
                    // attaches upward. It is legal only before the first
                    // token; `check` reports one that appears later, where a
                    // reader would take it for a `///` typo.
                    let is_module_doc = self.peek_at(2) == b'!';
                    while self.pos < self.src.len() && self.peek() != b'\n' {
                        self.pos = self.pos.saturating_add(1);
                    }
                    let raw = self.slice(start, self.pos);
                    // Two newlines with nothing between them is a blank line.
                    // Above the first thing in the run it is the token's; above
                    // a later comment it is that comment's own paragraph break.
                    let blank = newlines >= 2;
                    if is_module_doc {
                        let span = self.span(start);
                        self.module_docs.push((doc_body(raw.get(3..).unwrap_or("")), span));
                    } else if is_doc {
                        if self.run_empty() {
                            self.hold_blank(blank);
                        }
                        if self.pending_docs.is_empty() {
                            self.pending_docs_blank = blank;
                        }
                        self.pending_docs.push(doc_body(raw.get(3..).unwrap_or("")));
                        self.has_trivia = true;
                    } else {
                        if self.run_empty() {
                            self.hold_blank(blank);
                        }
                        let text = raw.trim_end().to_string();
                        let column = self.column(start);
                        self.pending_comments.push(Comment { text, blank_before: blank, column });
                        self.has_trivia = true;
                    }
                    newlines = 0;
                }
                b'/' if self.peek_at(1) == b'*' => {
                    let start = self.pos;
                    self.pos = self.pos.saturating_add(2);
                    // Block comments nest.
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
                        self.err(span, "unterminated block comment", "close it with `*/`; block comments nest, so each `/*` needs one").code("unterminated-comment");
                    }
                    let blank = newlines >= 2;
                    if self.run_empty() {
                        self.hold_blank(blank);
                    }
                    let text = self.slice(start, self.pos).to_string();
                    let column = self.column(start);
                    self.pending_comments.push(Comment { text, blank_before: blank, column });
                    self.has_trivia = true;
                    newlines = 0;
                }
                _ => {
                    // The newlines counted since the last thing read. With an
                    // empty run that gap is above the token; with a run above
                    // it, it is the gap between the run and the token, which
                    // is what makes a file header a header rather than a
                    // comment about the first declaration.
                    if self.run_empty() {
                        self.hold_blank(newlines >= 2);
                    } else {
                        self.detached = newlines >= 2;
                    }
                    return;
                }
            }
        }
    }

    fn ident(&mut self, start: usize) {
        while is_ident_continue(self.peek()) {
            self.pos = self.pos.saturating_add(1);
        }
        let s = self.slice(start, self.pos);
        if s == "_" {
            self.push(TokenKind::Underscore, 0, start);
            return;
        }
        match Keyword::from_str(s) {
            Some(Word::Keyword(keyword)) => self.push(TokenKind::of_keyword(keyword), 0, start),
            Some(Word::Reserved) => {
                let span = self.span(start);
                self.err(
                    span,
                    format!("`{s}` is a reserved word and may not be used as an identifier"),
                    format!("pick another name; `{s}` is not available"),
                ).code("reserved-word");
                if let Some(d) = self.errors.last_mut() {
                    d.notes.push(
                        "reserved for a future version of Buri; see grammar.ebnf, ReservedWord"
                            .into(),
                    );
                }
                self.push(TokenKind::Ident, 0, start);
            }
            None => self.push(TokenKind::Ident, 0, start),
        }
    }

    fn number(&mut self, start: usize) {
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
                self.err(span, format!("`{raw}` has no digits"), "write at least one digit after the base prefix, as in `0x1F`");
                self.push_int(0, start);
                return;
            }
            match u128::from_str_radix(&digits, radix) {
                Ok(v) => self.push_int(v, start),
                Err(_) => {
                    let span = self.span(start);
                    self.err(
                span,
                format!("`{raw}` is not a valid base-{radix} integer, or does not fit in 128 bits"),
                format!("use digits base-{radix} admits, and a value inside 128 bits"),
            );
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
                    self.err(
                span,
                format!("`{raw}` is not a valid float literal"),
                "a float needs a digit on each side of the point, as in `0.5`, and at most one exponent",
            );
                    self.push_float(0.0, start);
                }
            }
        } else {
            match clean.parse::<u128>() {
                Ok(v) => self.push_int(v, start),
                Err(_) => {
                    let span = self.span(start);
                    self.err(span, format!("`{raw}` does not fit in 128 bits"), "write a smaller value; 128 bits is the widest integer type");
                    self.push_int(0, start);
                }
            }
        }
    }

    /// Scans string body text, stopping at `"` or at an unescaped `${`.
    /// Returns (contents, ended_with_hole).
    /// The four bytes that end a run of ordinary string content. None of them
    /// can appear inside a multi-byte UTF-8 sequence, which is what lets the
    /// run be found by scanning bytes and copied without decoding.
    fn plain_str_byte(c: u8) -> bool {
        !matches!(c, b'"' | b'\\' | b'$' | b'\n')
    }

    fn scan_str_body(&mut self) -> (String, bool) {
        // `chunk` is the start of the run of source that belongs in the result
        // verbatim. A string with no escape is one such run, so the common
        // literal is copied once rather than a character at a time through a
        // fresh UTF-8 decode per character.
        let mut out = String::new();
        let mut chunk = self.pos;
        loop {
            if self.pos >= self.src.len() {
                out.push_str(self.slice(chunk, self.pos));
                let span = Span::new(self.file, self.pos, self.pos);
                self.err(span, "unterminated string literal", "close it with `\"`; a string literal does not span a line break");
                return (out, false);
            }
            match self.peek() {
                b'"' => {
                    let text = self.slice(chunk, self.pos);
                    self.pos = self.pos.saturating_add(1);
                    if out.is_empty() {
                        return (text.to_string(), false);
                    }
                    out.push_str(text);
                    return (out, false);
                }
                b'$' if self.peek_at(1) == b'{' => {
                    let text = self.slice(chunk, self.pos);
                    self.pos = self.pos.saturating_add(2);
                    if out.is_empty() {
                        return (text.to_string(), true);
                    }
                    out.push_str(text);
                    return (out, true);
                }
                b'\\' => {
                    out.push_str(self.slice(chunk, self.pos));
                    let start = self.pos;
                    self.pos = self.pos.saturating_add(1);
                    if let Some(c) = self.escape(start) {
                        out.push(c);
                    }
                    chunk = self.pos;
                }
                b'\n' => {
                    out.push_str(self.slice(chunk, self.pos));
                    let span = Span::new(self.file, self.pos, self.pos.saturating_add(1));
                    self.err(span, "unterminated string literal", "close it with `\"`; a string literal does not span a line break");
                    return (out, false);
                }
                _ => {
                    // A `$` with no `{` after it is content, so the first step
                    // is unconditional: without it this would stop on the same
                    // byte forever.
                    self.pos = self.pos.saturating_add(1);
                    while matches!(self.src.get(self.pos), Some(c) if Lexer::plain_str_byte(*c)) {
                        self.pos = self.pos.saturating_add(1);
                    }
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
                    self.err(
                        span,
                        "`\\u` must be followed by `{`, as in `\\u{1F600}`",
                        "brace the code point: `\\u{1F600}`",
                    );
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
                    self.err(span, "unterminated `\\u{...}` escape", "close it with `}`");
                    return None;
                }
                self.pos = self.pos.saturating_add(1);
                match u32::from_str_radix(&digits, 16).ok().and_then(char::from_u32) {
                    Some(c) => c,
                    None => {
                        let span = self.span(start);
                        self.err(
                            span,
                            format!("`\\u{{{digits}}}` is not a Unicode scalar value"),
                            "a scalar value is at most 10FFFF and outside D800-DFFF",
                        );
                        return None;
                    }
                }
            }
            _ => {
                let span = self.span(start);
                let shown = (c as char).to_string();
                self.err(
                    span,
                    format!("unknown escape `\\{shown}`"),
                    "the escapes are `\\n` `\\r` `\\t` `\\0` `\\\\` `\\\"` `\\'` `\\$` and `\\u{...}`",
                );
                if let Some(d) = self.errors.last_mut() {
                    d.notes
                        .push("the escapes are \\n \\r \\t \\0 \\\\ \\\" \\' \\$ and \\u{...}".into());
                }
                return None;
            }
        })
    }

    fn string_or_template(&mut self, start: usize) {
        self.pos = self.pos.saturating_add(1); // the opening quote
        let (body, hole) = self.scan_str_body();
        if hole {
            self.push_text(TokenKind::TemplateHead, body, start);
            self.modes.push(LexMode::Interpolation);
        } else {
            self.push_text(TokenKind::Str, body, start);
        }
    }

    /// Resumes template text after the `}` that closes a hole.
    fn resume_template(&mut self, start: usize) {
        let (body, hole) = self.scan_str_body();
        if hole {
            self.push_text(TokenKind::TemplateSpan, body, start);
        } else {
            self.push_text(TokenKind::TemplateTail, body, start);
            debug_assert_eq!(self.modes.last(), Some(&LexMode::Interpolation));
            self.modes.pop();
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
            self.err(span, "unterminated character literal", "close it with `\'`");
            self.push(TokenKind::Char, 0, start);
            return;
        } else {
            self.next_char()
        };
        if self.peek() != b'\'' {
            let span = self.span(start);
            self.err(
                span,
                "a character literal holds exactly one Unicode scalar value",
                "use a string literal for more than one",
            );
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

    /// A two-character token, the first character of which `bump` has already
    /// taken: this consumes the second.
    fn second(&mut self, p: Punctuation) -> Punctuation {
        self.pos = self.pos.saturating_add(1);
        p
    }

    fn punctuation(&mut self, start: usize) {
        use Punctuation::*;
        let c = self.bump();
        let two = self.peek();
        let p = match (c, two) {
            (b'{', _) => {
                self.modes.push(LexMode::Braces);
                LBrace
            }
            (b'}', _) => {
                match self.modes.last() {
                    // The innermost thing open is a hole, so this `}` resumes
                    // template text rather than terminating a block. The mode
                    // stays open until the template itself ends.
                    Some(LexMode::Interpolation) => {
                        self.resume_template(start);
                        return;
                    }
                    Some(LexMode::Braces) => {
                        self.modes.pop();
                    }
                    // Nothing is open. This `}` closes nothing, which the
                    // parser reports where it can say what was expected
                    // instead; what matters here is that there is no counter
                    // to clamp, so nothing after it is mis-lexed.
                    None => {}
                }
                RBrace
            }
            (b'(', _) => LParen,
            (b')', _) => RParen,
            (b'[', _) => LBracket,
            (b']', _) => RBracket,
            (b',', _) => Comma,
            (b';', _) => Semi,
            (b':', b':') => self.second(ColonColon),
            (b':', _) => Colon,
            (b'.', b'.') => self.second(DotDot),
            (b'.', _) => Dot,
            (b'@', _) => At,
            (b'=', b'>') => self.second(FatArrow),
            (b'=', b'=') => self.second(EqEq),
            (b'=', _) => Eq,
            (b'!', b'=') => self.second(BangEq),
            (b'!', _) => Bang,
            (b'<', b'=') => self.second(LtEq),
            (b'<', _) => Lt,
            (b'>', b'=') => self.second(GtEq),
            // No `>>` token: `Wrapper<Wrapper<Int>>` closes with two `>`.
            (b'>', _) => Gt,
            (b'+', _) => Plus,
            (b'-', _) => Minus,
            (b'*', _) => Star,
            (b'/', _) => Slash,
            (b'%', _) => Percent,
            (b'&', b'&') => self.second(AndAnd),
            (b'&', _) => And,
            (b'|', b'|') => self.second(OrOr),
            (b'|', _) => Or,
            (b'^', _) => Caret,
            (b'~', _) => Tilde,
            (b'?', b'?') => self.second(QuestionQuestion),
            // No `?.` token, so `x?.field` is `x` `?` `.` `field`.
            (b'?', _) => Question,
            _ => {
                // `bump` advanced one *byte*, and this arm is where every
                // character outside ASCII arrives — a `×` someone typed for
                // multiplication, a non-breaking space a word processor left
                // behind. Those are two, three, or four bytes, so reporting
                // the span `bump` left would cut a character in half; taking
                // the whole scalar is what makes the message show what was
                // typed. The code point is spelled out because the two
                // characters most likely to reach here are invisible.
                self.pos = start;
                let shown = self.next_char();
                let span = self.span(start);
                self.err(
                    span,
                    format!("unexpected character `{shown}` (U+{:04X})", shown as u32),
                    "delete it; no token in the language starts with it",
                );
                return;
            }
        };
        self.push(TokenKind::of_punctuation(p), 0, start);
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

fn is_ident_start(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphabetic()
}

fn is_ident_continue(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphanumeric()
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

    #[test]
    fn tuple_access_is_three_tokens() {
        // `pair.0` must not lex as IDENT FLOAT — this is why a float literal
        // has to begin with a digit (SPEC 12.14).
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
