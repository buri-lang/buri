//! The JavaScript AST, its printer, and the minifier.
//!
//! The backend builds this tree rather than strings, so minification is a set
//! of passes over a real structure: dead code elimination by reachability,
//! constant folding, dead-branch removal, and identifier mangling with proper
//! scope analysis. The printer emits no whitespace it does not need and
//! inserts parentheses only where precedence requires them.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "the counters here are a printer's indentation, a scope walk's \
              nesting depth and a rename counter, each decremented only by the \
              step that paired with an increment; the rest are byte positions \
              within a source this file is walking, every read of which goes \
              through a bounds check"
)]

use crate::diagnostics::Invariant as _;
use crate::hash::{Map as HashMap, Set as HashSet};

// ---------------------------------------------------------------------------
// The tree
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VarKind {
    Const,
    Let,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Var { kind: VarKind, name: String, init: Option<Expr> },
    Func { name: String, params: Vec<String>, body: Vec<Stmt> },
    Return(Option<Expr>),
    If { cond: Expr, then: Vec<Stmt>, else_: Vec<Stmt> },
    While { cond: Expr, body: Vec<Stmt> },
    Switch { disc: Expr, cases: Vec<(Option<Expr>, Vec<Stmt>)> },
    Expr(Expr),
    Throw(Expr),
    Break,
    Continue,
    Block(Vec<Stmt>),
    /// `export default e`
    ExportDefault(Expr),
    /// Verbatim source, used for the runtime and for nothing else.
    Raw(String),
    /// One verbatim top-level declaration, named so that dead code
    /// elimination can drop it. This is how the hand-written runtime is
    /// tree-shaken: a program that never allocates a string never carries
    /// `$str_split`.
    RawDecl { name: String, src: String },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Lt,
    Le,
    Gt,
    Ge,
    StrictEq,
    StrictNe,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    UShr,
    And,
    Or,
    Coalesce,
    In,
}

impl BinOp {
    fn text(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::StrictEq => "===",
            BinOp::StrictNe => "!==",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::UShr => ">>>",
            BinOp::And => "&&",
            BinOp::Or => "||",
            BinOp::Coalesce => "??",
            BinOp::In => " in ",
        }
    }

    /// Higher binds tighter.
    fn prec(self) -> u8 {
        match self {
            BinOp::Or | BinOp::Coalesce => 3,
            BinOp::And => 4,
            BinOp::BitOr => 5,
            BinOp::BitXor => 6,
            BinOp::BitAnd => 7,
            BinOp::StrictEq | BinOp::StrictNe => 8,
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::In => 9,
            BinOp::Shl | BinOp::Shr | BinOp::UShr => 10,
            BinOp::Add | BinOp::Sub => 11,
            BinOp::Mul | BinOp::Div | BinOp::Rem => 12,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
    TypeOf,
    Void,
}

impl UnOp {
    fn text(self) -> &'static str {
        match self {
            UnOp::Neg => "-",
            UnOp::Not => "!",
            UnOp::BitNot => "~",
            UnOp::TypeOf => "typeof ",
            UnOp::Void => "void ",
        }
    }
}

#[derive(Clone, Debug)]
pub enum Expr {
    Num(f64),
    /// Emitted with an `n` suffix. `Int` is `I64` on every target, so ordinary
    /// integer code lands here (SPEC 15, open question 8).
    BigInt(String),
    Str(String),
    Bool(bool),
    Null,
    Undefined,
    Ident(String),
    Array(Vec<Expr>),
    Object(Vec<(String, Expr)>),
    /// `obj.prop`
    Member { obj: Box<Expr>, prop: String },
    /// `obj[i]`
    Index { obj: Box<Expr>, index: Box<Expr> },
    Call { callee: Box<Expr>, args: Vec<Expr> },
    New { callee: Box<Expr>, args: Vec<Expr> },
    Unary { op: UnOp, operand: Box<Expr> },
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr> },
    Cond { test: Box<Expr>, cons: Box<Expr>, alt: Box<Expr> },
    Assign { target: Box<Expr>, value: Box<Expr> },
    Arrow { params: Vec<String>, body: Box<Expr> },
    /// An arrow whose body is a block, for anything that needs statements.
    ArrowBlock { params: Vec<String>, body: Vec<Stmt> },
    /// An immediately-invoked arrow, which is how a block-valued expression is
    /// spelled where JavaScript wants an expression.
    Seq(Vec<Expr>),
    Spread(Box<Expr>),
}

impl Expr {
    pub fn call(callee: Expr, args: Vec<Expr>) -> Expr {
        Expr::Call { callee: Box::new(callee), args }
    }

    pub fn member(obj: Expr, prop: &str) -> Expr {
        Expr::Member { obj: Box::new(obj), prop: prop.to_string() }
    }

    pub fn index(obj: Expr, index: Expr) -> Expr {
        Expr::Index { obj: Box::new(obj), index: Box::new(index) }
    }

    /// Simplifies as it builds, so the backend gets the peepholes without
    /// asking and the folder has one place to call. See `simplify_bin`.
    pub fn bin(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
        simplify_bin(op, lhs, rhs)
    }

    pub fn un(op: UnOp, operand: Expr) -> Expr {
        simplify_un(op, operand)
    }

    pub fn cond(test: Expr, cons: Expr, alt: Expr) -> Expr {
        simplify_cond(test, cons, alt)
    }

    pub fn ident(name: impl Into<String>) -> Expr {
        Expr::Ident(name.into())
    }

    fn prec(&self) -> u8 {
        match self {
            Expr::Seq(_) => 0,
            Expr::Arrow { .. } | Expr::ArrowBlock { .. } | Expr::Assign { .. } => 1,
            Expr::Cond { .. } => 2,
            Expr::Binary { op, .. } => op.prec(),
            Expr::Unary { .. } => 13,
            Expr::New { .. } => 15,
            Expr::Call { .. } | Expr::Member { .. } | Expr::Index { .. } => 16,
            Expr::Spread(_) => 1,
            _ => 17,
        }
    }

    /// Whether this is a literal with no side effects, so the minifier may
    /// drop or duplicate it.
    pub fn is_pure_literal(&self) -> bool {
        matches!(
            self,
            Expr::Num(_)
                | Expr::BigInt(_)
                | Expr::Str(_)
                | Expr::Bool(_)
                | Expr::Null
                | Expr::Undefined
                | Expr::Ident(_)
        )
    }

    /// Whether evaluating this can be skipped: no call, no assignment, no
    /// `new`. Projections count as pure because the language has no null and
    /// no out-of-bounds index, so `x[0]` on a value the backend built cannot
    /// throw.
    pub fn is_pure(&self) -> bool {
        match self {
            Expr::Call { .. } | Expr::New { .. } | Expr::Assign { .. } => false,
            // A block-bodied arrow is a value, not the work inside it, but
            // treating one as pure invites hoisting it somewhere it would run
            // a different number of times. Only the value form is claimed.
            Expr::ArrowBlock { .. } => false,
            Expr::Arrow { .. } => true,
            Expr::Array(xs) | Expr::Seq(xs) => xs.iter().all(Expr::is_pure),
            Expr::Object(fs) => fs.iter().all(|(_, v)| v.is_pure()),
            Expr::Member { obj, .. } => obj.is_pure(),
            Expr::Index { obj, index } => obj.is_pure() && index.is_pure(),
            Expr::Unary { operand, .. } => operand.is_pure(),
            Expr::Binary { lhs, rhs, .. } => lhs.is_pure() && rhs.is_pure(),
            Expr::Cond { test, cons, alt } => {
                test.is_pure() && cons.is_pure() && alt.is_pure()
            }
            Expr::Spread(x) => x.is_pure(),
            _ => true,
        }
    }

    /// Whether this certainly evaluates to a JavaScript boolean.
    ///
    /// Several rewrites below are sound only for booleans — `!!e` is `e` for a
    /// boolean and `Boolean(e)` for anything else, and `e && true` is `e` for a
    /// boolean and `true` for a truthy number. Every operand the backend gives
    /// these operators is a `Bool`, but the check is syntactic rather than
    /// trusting that, because the cost of being wrong is a silent miscompile.
    fn is_boolean(&self) -> bool {
        match self {
            Expr::Bool(_) => true,
            Expr::Unary { op: UnOp::Not, .. } => true,
            Expr::Binary { op, lhs, rhs } => match op {
                BinOp::StrictEq
                | BinOp::StrictNe
                | BinOp::Lt
                | BinOp::Le
                | BinOp::Gt
                | BinOp::Ge
                | BinOp::In => true,
                // `a && b` is `b` when `a` is truthy, so it is a boolean only
                // when both sides are.
                BinOp::And | BinOp::Or => lhs.is_boolean() && rhs.is_boolean(),
                _ => false,
            },
            Expr::Cond { cons, alt, .. } => cons.is_boolean() && alt.is_boolean(),
            _ => false,
        }
    }

    /// Structural equality, for the rewrites that need two occurrences of the
    /// same expression to be the same value. Only shapes that are cheap to
    /// compare and cannot hold a side effect answer `true`.
    pub(crate) fn same_as(&self, other: &Expr) -> bool {
        match (self, other) {
            (Expr::Ident(a), Expr::Ident(b)) => a == b,
            (Expr::Str(a), Expr::Str(b)) => a == b,
            (Expr::Bool(a), Expr::Bool(b)) => a == b,
            (Expr::Num(a), Expr::Num(b)) => a == b,
            (Expr::BigInt(a), Expr::BigInt(b)) => a == b,
            (Expr::Null, Expr::Null) | (Expr::Undefined, Expr::Undefined) => true,
            (Expr::Member { obj: a, prop: p }, Expr::Member { obj: b, prop: q }) => {
                p == q && a.same_as(b)
            }
            (
                Expr::Index { obj: a, index: i },
                Expr::Index { obj: b, index: j },
            ) => a.same_as(b) && i.same_as(j),
            (
                Expr::Unary { op: x, operand: a },
                Expr::Unary { op: y, operand: b },
            ) => x == y && a.same_as(b),
            (
                Expr::Binary { op: x, lhs: a, rhs: b },
                Expr::Binary { op: y, lhs: c, rhs: d },
            ) => x == y && a.same_as(c) && b.same_as(d),
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Peepholes
// ---------------------------------------------------------------------------
//
// These run at construction, so the backend never has to ask for them and the
// folder never has to duplicate them. Each rewrite is either unconditional in
// JavaScript or guarded by `is_boolean` / `is_pure`; nothing here relies on
// what the Buri type checker knows, because this layer cannot see it.

fn simplify_un(op: UnOp, operand: Expr) -> Expr {
    match (op, &operand) {
        (UnOp::Not, Expr::Bool(b)) => return Expr::Bool(!b),
        (UnOp::Neg, Expr::Num(n)) => return Expr::Num(-n),
        (UnOp::Not, Expr::Unary { op: UnOp::Not, operand: inner })
            // `!!e` is `e` only when `e` is already a boolean.
            if inner.is_boolean() =>
        {
            return (**inner).clone();
        }
        // Negating an equality flips the operator rather than wrapping it.
        // Relational operators are deliberately absent: `!(a < b)` is not
        // `a >= b` when either side is NaN.
        (UnOp::Not, Expr::Binary { op: BinOp::StrictEq, lhs, rhs }) => {
            return Expr::bin(BinOp::StrictNe, (**lhs).clone(), (**rhs).clone());
        }
        (UnOp::Not, Expr::Binary { op: BinOp::StrictNe, lhs, rhs }) => {
            return Expr::bin(BinOp::StrictEq, (**lhs).clone(), (**rhs).clone());
        }
        _ => {}
    }
    Expr::Unary { op, operand: Box::new(operand) }
}

fn simplify_bin(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
    // Both operands known.
    if let (Expr::Num(a), Expr::Num(b)) = (&lhs, &rhs) {
        let (a, b) = (*a, *b);
        match op {
            BinOp::Add => return Expr::Num(a + b),
            BinOp::Sub => return Expr::Num(a - b),
            BinOp::Mul => return Expr::Num(a * b),
            BinOp::Lt => return Expr::Bool(a < b),
            BinOp::Le => return Expr::Bool(a <= b),
            BinOp::Gt => return Expr::Bool(a > b),
            BinOp::Ge => return Expr::Bool(a >= b),
            BinOp::StrictEq => return Expr::Bool(a == b),
            BinOp::StrictNe => return Expr::Bool(a != b),
            // A zero divisor is left alone: `1/0` is `Infinity` in JavaScript
            // and an abort in Buri, so folding it would answer a question the
            // language says has no answer.
            BinOp::Div if b != 0.0 => return Expr::Num(a / b),
            BinOp::Rem if b != 0.0 => return Expr::Num(a % b),
            _ => {}
        }
    }
    if let (Expr::Str(a), Expr::Str(b)) = (&lhs, &rhs) {
        match op {
            BinOp::Add => return Expr::Str(format!("{a}{b}")),
            BinOp::StrictEq => return Expr::Bool(a == b),
            BinOp::StrictNe => return Expr::Bool(a != b),
            _ => {}
        }
    }
    if let (Expr::Bool(a), Expr::Bool(b)) = (&lhs, &rhs) {
        match op {
            BinOp::StrictEq => return Expr::Bool(a == b),
            BinOp::StrictNe => return Expr::Bool(a != b),
            _ => {}
        }
    }

    match op {
        // Comparing a boolean against a literal is the boolean, or its
        // negation. This is what a pattern test on `true`/`false` produces.
        BinOp::StrictEq | BinOp::StrictNe => {
            let eq = op == BinOp::StrictEq;
            if let Expr::Bool(b) = rhs {
                if lhs.is_boolean() {
                    return if b == eq { lhs } else { Expr::un(UnOp::Not, lhs) };
                }
            }
            if let Expr::Bool(b) = lhs {
                if rhs.is_boolean() {
                    return if b == eq { rhs } else { Expr::un(UnOp::Not, rhs) };
                }
            }
        }
        BinOp::And => {
            match (&lhs, &rhs) {
                (Expr::Bool(true), _) => return rhs,
                (Expr::Bool(false), _) => return Expr::Bool(false),
                // `e && true` is `e` only for a boolean `e`: `1 && true` is
                // `true`, not `1`.
                (_, Expr::Bool(true)) if lhs.is_boolean() => return lhs,
                (_, Expr::Bool(false)) if lhs.is_pure() => return Expr::Bool(false),
                _ => {}
            }
            if lhs.same_as(&rhs) && lhs.is_pure() {
                return lhs;
            }
            // A tag test cannot hold two different values at once. This is what
            // an or-pattern's alternatives collapse to once each has been
            // rewritten against the branch it sits under.
            if let Some(false) = both_equalities_agree(&lhs, &rhs, true) {
                return Expr::Bool(false);
            }
        }
        BinOp::Or => {
            match (&lhs, &rhs) {
                (Expr::Bool(true), _) => return Expr::Bool(true),
                (Expr::Bool(false), _) => return rhs,
                (_, Expr::Bool(false)) if lhs.is_boolean() => return lhs,
                (_, Expr::Bool(true)) if lhs.is_pure() => return Expr::Bool(true),
                _ => {}
            }
            if lhs.same_as(&rhs) && lhs.is_pure() {
                return lhs;
            }
        }
        _ => {}
    }

    Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }
}

/// For `x === a` and `x === b` over the same pure `x` and two distinct
/// literals: `Some(false)` when they cannot both hold. `None` when the shape
/// does not apply.
fn both_equalities_agree(lhs: &Expr, rhs: &Expr, conjunction: bool) -> Option<bool> {
    let (Expr::Binary { op: BinOp::StrictEq, lhs: a, rhs: av }, Expr::Binary { op: BinOp::StrictEq, lhs: b, rhs: bv }) =
        (lhs, rhs)
    else {
        return None;
    };
    if !a.same_as(b) || !a.is_pure() {
        return None;
    }
    if !av.is_pure_literal() || !bv.is_pure_literal() || av.same_as(bv) {
        return None;
    }
    // Two different constants, so at most one test can hold.
    if conjunction {
        Some(false)
    } else {
        None
    }
}

fn simplify_cond(test: Expr, cons: Expr, alt: Expr) -> Expr {
    match &test {
        Expr::Bool(true) => return cons,
        Expr::Bool(false) => return alt,
        _ => {}
    }
    // `c ? true : false` is `c`, and `c ? false : true` is `!c`.
    if let (Expr::Bool(a), Expr::Bool(b)) = (&cons, &alt) {
        if *a && !*b && test.is_boolean() {
            return test;
        }
        if !*a && *b {
            return Expr::un(UnOp::Not, test);
        }
    }
    // Both branches the same value, and nothing observable in choosing.
    if cons.same_as(&alt) && test.is_pure() {
        return cons;
    }
    // A branch that yields a constant boolean is a short-circuit. Sound only
    // for a boolean test: `5 ? true : x` is `true`, where `5 || x` is `5`.
    if test.is_boolean() {
        match (&cons, &alt) {
            (Expr::Bool(true), _) => return Expr::bin(BinOp::Or, test, alt),
            (Expr::Bool(false), _) => {
                return Expr::bin(BinOp::And, Expr::un(UnOp::Not, test), alt)
            }
            (_, Expr::Bool(true)) => {
                return Expr::bin(BinOp::Or, Expr::un(UnOp::Not, test), cons)
            }
            (_, Expr::Bool(false)) => return Expr::bin(BinOp::And, test, cons),
            _ => {}
        }
    }
    // `!c ? a : b` is `c ? b : a`, which is one character shorter and reads
    // in the order the source did.
    if let Expr::Unary { op: UnOp::Not, operand } = &test {
        if operand.is_boolean() {
            return simplify_cond((**operand).clone(), alt, cons);
        }
    }
    // A ternary whose branches share an arm folds into a single test.
    // `c ? (p ? x : y) : y`  ->  `c && p ? x : y`
    if let Expr::Cond { test: p, cons: x, alt: y } = &cons {
        if y.same_as(&alt) && p.is_pure() {
            return simplify_cond(
                Expr::bin(BinOp::And, test, (**p).clone()),
                (**x).clone(),
                alt,
            );
        }
    }
    // `c ? x : (p ? x : y)`  ->  `c || p ? x : y`
    if let Expr::Cond { test: p, cons: x, alt: y } = &alt {
        if x.same_as(&cons) && p.is_pure() {
            return simplify_cond(
                Expr::bin(BinOp::Or, test, (**p).clone()),
                cons,
                (**y).clone(),
            );
        }
    }
    Expr::Cond { test: Box::new(test), cons: Box::new(cons), alt: Box::new(alt) }
}

// ---------------------------------------------------------------------------
// Printing
// ---------------------------------------------------------------------------

pub struct Printer {
    out: String,
    /// Pretty output keeps newlines and indentation; `buri build --debug`
    /// wants to be readable and `--release` wants to be small.
    pretty: bool,
    depth: usize,
}

pub fn print(stmts: &[Stmt], pretty: bool) -> String {
    let mut p = Printer { out: String::new(), pretty, depth: 0 };
    p.stmts(stmts);
    p.out
}

impl Printer {
    fn nl(&mut self) {
        if self.pretty {
            self.out.push('\n');
            for _ in 0..self.depth {
                self.out.push_str("  ");
            }
        }
    }

    fn stmts(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Var { kind, name, init } => {
                self.nl();
                self.out.push_str(if *kind == VarKind::Const { "const " } else { "let " });
                self.out.push_str(name);
                if let Some(e) = init {
                    self.out.push('=');
                    self.expr(e, 1);
                }
                self.out.push(';');
            }
            Stmt::Func { name, params, body } => {
                self.nl();
                self.out.push_str("function ");
                self.out.push_str(name);
                self.out.push('(');
                self.out.push_str(&params.join(","));
                self.out.push_str("){");
                self.depth += 1;
                self.stmts(body);
                self.depth -= 1;
                self.nl();
                self.out.push('}');
            }
            Stmt::Return(e) => {
                self.nl();
                match e {
                    Some(e) => {
                        self.out.push_str("return ");
                        self.expr(e, 0);
                    }
                    None => self.out.push_str("return"),
                }
                self.out.push(';');
            }
            Stmt::If { cond, then, else_ } => {
                self.nl();
                self.out.push_str("if(");
                self.expr(cond, 0);
                self.out.push_str("){");
                self.depth += 1;
                self.stmts(then);
                self.depth -= 1;
                self.nl();
                self.out.push('}');
                if !else_.is_empty() {
                    self.out.push_str("else");
                    // `else if` chains stay flat rather than nesting braces.
                    if let [only @ Stmt::If { .. }] = else_.as_slice() {
                        self.out.push(' ');
                        let saved = std::mem::take(&mut self.out);
                        self.stmt(only);
                        let inner = std::mem::replace(&mut self.out, saved);
                        self.out.push_str(inner.trim_start());
                    } else {
                        self.out.push('{');
                        self.depth += 1;
                        self.stmts(else_);
                        self.depth -= 1;
                        self.nl();
                        self.out.push('}');
                    }
                }
            }
            Stmt::While { cond, body } => {
                self.nl();
                self.out.push_str("while(");
                self.expr(cond, 0);
                self.out.push_str("){");
                self.depth += 1;
                self.stmts(body);
                self.depth -= 1;
                self.nl();
                self.out.push('}');
            }
            Stmt::Switch { disc, cases } => {
                self.nl();
                self.out.push_str("switch(");
                self.expr(disc, 0);
                self.out.push_str("){");
                self.depth += 1;
                for (test, body) in cases {
                    self.nl();
                    match test {
                        Some(t) => {
                            self.out.push_str("case ");
                            self.expr(t, 0);
                            self.out.push(':');
                        }
                        None => self.out.push_str("default:"),
                    }
                    self.depth += 1;
                    self.stmts(body);
                    self.depth -= 1;
                }
                self.depth -= 1;
                self.nl();
                self.out.push('}');
            }
            Stmt::Expr(e) => {
                self.nl();
                // A statement may not begin with `{` or `function`.
                let needs_paren = matches!(e, Expr::Object(_) | Expr::ArrowBlock { .. });
                if needs_paren {
                    self.out.push('(');
                }
                self.expr(e, 0);
                if needs_paren {
                    self.out.push(')');
                }
                self.out.push(';');
            }
            Stmt::Throw(e) => {
                self.nl();
                self.out.push_str("throw ");
                self.expr(e, 0);
                self.out.push(';');
            }
            Stmt::Break => {
                self.nl();
                self.out.push_str("break;");
            }
            Stmt::Continue => {
                self.nl();
                self.out.push_str("continue;");
            }
            Stmt::Block(body) => {
                self.nl();
                self.out.push('{');
                self.depth += 1;
                self.stmts(body);
                self.depth -= 1;
                self.nl();
                self.out.push('}');
            }
            Stmt::ExportDefault(e) => {
                self.nl();
                self.out.push_str("export default ");
                self.expr(e, 0);
                self.out.push(';');
            }
            Stmt::Raw(s) => {
                self.nl();
                self.out.push_str(s);
            }
            Stmt::RawDecl { src, .. } => {
                self.nl();
                self.out.push_str(src);
            }
        }
    }

    fn expr(&mut self, e: &Expr, parent_prec: u8) {
        let prec = e.prec();
        let paren = prec < parent_prec;
        if paren {
            self.out.push('(');
        }
        match e {
            Expr::Num(n) => self.out.push_str(&number(*n)),
            Expr::BigInt(s) => {
                self.out.push_str(s);
                self.out.push('n');
            }
            Expr::Str(s) => self.out.push_str(&quote(s)),
            Expr::Bool(b) => self.out.push_str(if *b { "true" } else { "false" }),
            Expr::Null => self.out.push_str("null"),
            Expr::Undefined => self.out.push_str("void 0"),
            Expr::Ident(name) => self.out.push_str(name),
            Expr::Array(items) => {
                self.out.push('[');
                for (i, x) in items.iter().enumerate() {
                    if i > 0 {
                        self.out.push(',');
                    }
                    self.expr(x, 1);
                }
                self.out.push(']');
            }
            Expr::Object(fields) => {
                self.out.push('{');
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        self.out.push(',');
                    }
                    if is_ident_like(k) {
                        self.out.push_str(k);
                    } else {
                        self.out.push_str(&quote(k));
                    }
                    self.out.push(':');
                    self.expr(v, 1);
                }
                self.out.push('}');
            }
            Expr::Member { obj, prop } => {
                self.expr(obj, 16);
                self.out.push('.');
                self.out.push_str(prop);
            }
            Expr::Index { obj, index } => {
                self.expr(obj, 16);
                self.out.push('[');
                self.expr(index, 0);
                self.out.push(']');
            }
            Expr::Call { callee, args } => {
                self.expr(callee, 16);
                self.out.push('(');
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        self.out.push(',');
                    }
                    self.expr(a, 1);
                }
                self.out.push(')');
            }
            Expr::New { callee, args } => {
                self.out.push_str("new ");
                self.expr(callee, 16);
                self.out.push('(');
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        self.out.push(',');
                    }
                    self.expr(a, 1);
                }
                self.out.push(')');
            }
            Expr::Unary { op, operand } => {
                self.out.push_str(op.text());
                // `- -x` must not print as `--x`.
                if matches!(op, UnOp::Neg)
                    && matches!(&**operand, Expr::Unary { op: UnOp::Neg, .. })
                {
                    self.out.push(' ');
                }
                self.expr(operand, 13);
            }
            Expr::Binary { op, lhs, rhs } => {
                let p = op.prec();
                self.expr(lhs, p);
                self.out.push_str(op.text());
                // `a - -b` and `a + +b` need the space back.
                let needs_space = matches!(op, BinOp::Sub | BinOp::Add)
                    && matches!(
                        &**rhs,
                        Expr::Unary { op: UnOp::Neg, .. } | Expr::Num(_)
                    )
                    && starts_with_sign(rhs);
                if needs_space {
                    self.out.push(' ');
                }
                // Left-associative, so the right operand binds one tighter.
                self.expr(rhs, p + 1);
            }
            Expr::Cond { test, cons, alt } => {
                self.expr(test, 3);
                self.out.push('?');
                self.expr(cons, 1);
                self.out.push(':');
                self.expr(alt, 1);
            }
            Expr::Assign { target, value } => {
                self.expr(target, 16);
                self.out.push('=');
                self.expr(value, 1);
            }
            Expr::Arrow { params, body } => {
                self.arrow_params(params);
                self.out.push_str("=>");
                // An arrow body that is an object literal needs parentheses.
                if matches!(&**body, Expr::Object(_)) {
                    self.out.push('(');
                    self.expr(body, 0);
                    self.out.push(')');
                } else {
                    self.expr(body, 1);
                }
            }
            Expr::ArrowBlock { params, body } => {
                self.arrow_params(params);
                self.out.push_str("=>{");
                self.depth += 1;
                self.stmts(body);
                self.depth -= 1;
                self.nl();
                self.out.push('}');
            }
            Expr::Seq(items) => {
                for (i, x) in items.iter().enumerate() {
                    if i > 0 {
                        self.out.push(',');
                    }
                    self.expr(x, 1);
                }
            }
            Expr::Spread(inner) => {
                self.out.push_str("...");
                self.expr(inner, 1);
            }
        }
        if paren {
            self.out.push(')');
        }
    }

    fn arrow_params(&mut self, params: &[String]) {
        if let [only] = params {
            self.out.push_str(only);
        } else {
            self.out.push('(');
            self.out.push_str(&params.join(","));
            self.out.push(')');
        }
    }
}

fn starts_with_sign(e: &Expr) -> bool {
    match e {
        Expr::Num(n) => *n < 0.0,
        Expr::Unary { op: UnOp::Neg, .. } => true,
        _ => false,
    }
}

fn is_ident_like(s: &str) -> bool {
    !s.is_empty()
        && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// The shortest spelling of a number that round-trips.
pub fn number(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "1/0".into() } else { "-1/0".into() };
    }
    if n == 0.0 && n.is_sign_negative() {
        return "-0".into();
    }
    let mut s = format!("{n}");
    // Shorten `0.5` to `.5`.
    if let Some(rest) = s.strip_prefix("0.") {
        s = format!(".{rest}");
    } else if let Some(rest) = s.strip_prefix("-0.") {
        s = format!("-.{rest}");
    }
    // Exponent form when it is shorter, which it is for anything with a long
    // run of zeroes on either side of the point.
    let exp = format!("{n:e}");
    if exp.len() < s.len() && exp.parse::<f64>().map(|v| v == n).unwrap_or(false) {
        s = exp;
    }
    s
}

pub fn quote(s: &str) -> String {
    // Pick whichever quote needs less escaping.
    let singles = s.matches('\'').count();
    let doubles = s.matches('"').count();
    let q = if singles <= doubles { '\'' } else { '"' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(q);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            '\0' => out.push_str("\\0"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

// ---------------------------------------------------------------------------
// Minification
// ---------------------------------------------------------------------------

pub struct MinifyOptions {
    pub mangle: bool,
    pub fold: bool,
    pub drop_unreachable: bool,
}

impl Default for MinifyOptions {
    fn default() -> Self {
        MinifyOptions { mangle: true, fold: true, drop_unreachable: true }
    }
}

/// Runs the optimisation passes over a whole program.
pub fn minify(stmts: Vec<Stmt>, roots: &[String], opts: &MinifyOptions) -> Vec<Stmt> {
    let mut stmts = stmts;
    if opts.fold {
        // Sharing a constant aggregate hides its contents from the folder:
        // `[3, 'x'][0]` is `3`, but `$k0[0]` is an array read. The table lets
        // the folder see through the declaration; whichever declarations
        // nothing needs afterwards are dropped below.
        let table = constant_table(&stmts);
        stmts = fold_block(stmts);
        // Folding turns branches into expressions, which leaves temporaries
        // nothing reads; cleanup removes them, which exposes more folding —
        // and cleanup is also what turns `const x = $k0; x[0]` into `$k0[0]`,
        // so reading through has to happen inside that loop rather than
        // before it. The three run together, per body, until none of them has
        // anything left to do.
        stmts = clean_locals(stmts, &table);
        // Last, so that two instances which only became the same through
        // folding and inlining are still seen as the same.
        stmts = merge_identical(stmts, roots);
    }
    if opts.drop_unreachable {
        stmts = eliminate_dead(stmts, roots);
    }
    if opts.mangle {
        stmts = mangle_program(stmts, roots);
    }
    stmts
}

// -- constant folding --------------------------------------------------------

/// Whether a `continue` in these statements targets *this* loop.
///
/// A `continue` inside a nested loop belongs to that loop, and one inside a
/// nested function or closure is not a `continue` at all, so neither is
/// descended into.
fn reaches_continue(body: &[Stmt]) -> bool {
    body.iter().any(|s| match s {
        Stmt::Continue => true,
        Stmt::If { then, else_, .. } => reaches_continue(then) || reaches_continue(else_),
        Stmt::Switch { cases, .. } => cases.iter().any(|(_, b)| reaches_continue(b)),
        Stmt::Block(b) => reaches_continue(b),
        _ => false,
    })
}

/// Whether a `break` in these statements targets *this* loop.
///
/// A `break` is not merely a loop exit: inside a `switch` it ends the case, and
/// the same `break` in a body that stopped being a loop would mean something
/// else. Rather than reason about which, the loop is only unwrapped when there
/// is no `break` to reason about — which is the case for every arm that
/// `return`s, and that is the shape this exists for.
fn reaches_break(body: &[Stmt]) -> bool {
    body.iter().any(|s| match s {
        Stmt::Break => true,
        Stmt::If { then, else_, .. } => reaches_break(then) || reaches_break(else_),
        Stmt::Block(b) => reaches_break(b),
        // A `break` inside a nested `switch` or loop belongs to that one.
        _ => false,
    })
}

/// Whether every path through these statements leaves — which is what makes a
/// `while (true)` with no `break` and no `continue` run exactly once rather
/// than forever.
fn always_exits(body: &[Stmt]) -> bool {
    match body.last() {
        Some(Stmt::Return(_) | Stmt::Throw(_)) => true,
        Some(Stmt::Block(b)) => always_exits(b),
        Some(Stmt::If { then, else_, .. }) => {
            !else_.is_empty() && always_exits(then) && always_exits(else_)
        }
        _ => false,
    }
}

fn fold_stmt(s: Stmt) -> Stmt {
    match s {
        Stmt::Var { kind, name, init } => Stmt::Var { kind, name, init: init.map(fold) },
        Stmt::Func { name, params, body } => {
            Stmt::Func { name, params, body: fold_block(body) }
        }
        Stmt::Return(e) => Stmt::Return(e.map(fold)),
        Stmt::If { cond, then, else_ } => {
            let cond = fold(cond);
            let then = fold_block(then);
            let else_ = fold_block(else_);
            // A branch whose condition is a known constant is not emitted.
            match &cond {
                Expr::Bool(true) => return Stmt::Block(then),
                Expr::Bool(false) if !else_.is_empty() => return Stmt::Block(else_),
                Expr::Bool(false) => return Stmt::Block(Vec::new()),
                _ => {}
            }
            // Two branches that differ only in a value are one expression.
            // Because this pass runs bottom-up, a chain collapses from the
            // inside out: the innermost `if`/`else` becomes a ternary, which
            // makes its parent's branch a single `return`, and so on.
            if let (Some(a), Some(b)) = (sole_return(&then), sole_return(&else_)) {
                return Stmt::Return(Some(Expr::cond(cond, a.clone(), b.clone())));
            }
            if let (Some((ta, a)), Some((tb, b))) =
                (sole_assignment(&then), sole_assignment(&else_))
            {
                if ta.same_as(tb) {
                    return Stmt::Expr(Expr::Assign {
                        target: Box::new(ta.clone()),
                        value: Box::new(Expr::cond(cond, a.clone(), b.clone())),
                    });
                }
            }
            Stmt::If { cond, then, else_ }
        }
        Stmt::While { cond, body } => {
            let cond = fold(cond);
            let body = fold_block(body);
            // A guarded match compiles to `while(true)` because an arm that
            // matches may still fall through to the next one. Where no arm
            // does — every one of them `return`s — nothing jumps back to the
            // top and nothing jumps out, so the loop runs exactly once and is
            // only a wrapper. Removing it is what lets `fold_block`'s
            // truncation-after-a-terminator and the `sole_return` rule above
            // reach the arms.
            //
            // A tail-call loop and a merged dispatch group both `continue`,
            // and a guarded match in statement position `break`s out with its
            // answer — where the `break` is doing real work and removing the
            // loop around it would run the arms after it as well. Both are
            // left exactly as they were.
            if matches!(cond, Expr::Bool(true))
                && !reaches_continue(&body)
                && !reaches_break(&body)
                && always_exits(&body)
            {
                return Stmt::Block(fold_block(body));
            }
            Stmt::While { cond, body }
        }
        Stmt::Switch { disc, cases } => Stmt::Switch {
            disc: fold(disc),
            cases: cases.into_iter().map(|(t, b)| (t.map(fold), fold_block(b))).collect(),
        },
        Stmt::Expr(e) => Stmt::Expr(fold(e)),
        Stmt::Throw(e) => Stmt::Throw(fold(e)),
        Stmt::Block(b) => Stmt::Block(fold_block(b)),
        Stmt::ExportDefault(e) => Stmt::ExportDefault(fold(e)),
        other => other,
    }
}

fn fold_block(body: Vec<Stmt>) -> Vec<Stmt> {
    let mut out = Vec::new();
    for s in body {
        let s = fold_stmt(s);
        // Flatten the empty and singleton blocks folding produces.
        match s {
            Stmt::Block(inner) if inner.is_empty() => continue,
            Stmt::Block(inner) if !inner.iter().any(is_declaration) => out.extend(inner),
            other => out.push(other),
        }
        // Nothing after `return`, `throw`, `break` or `continue` runs.
        if matches!(
            out.last(),
            Some(Stmt::Return(_) | Stmt::Throw(_) | Stmt::Break | Stmt::Continue)
        ) {
            break;
        }
    }
    out
}

fn is_declaration(s: &Stmt) -> bool {
    matches!(s, Stmt::Var { .. } | Stmt::Func { .. } | Stmt::RawDecl { .. })
}

/// A block that is exactly `return <expr>;`, and nothing else.
fn sole_return(body: &[Stmt]) -> Option<&Expr> {
    match body {
        [Stmt::Return(Some(e))] => Some(e),
        _ => None,
    }
}

/// A block that is exactly one assignment, as `<target> = <value>;`.
fn sole_assignment(body: &[Stmt]) -> Option<(&Expr, &Expr)> {
    match body {
        [Stmt::Expr(Expr::Assign { target, value })] => Some((target, value)),
        _ => None,
    }
}

fn fold(e: Expr) -> Expr {
    match e {
        // The simplifications themselves live in the smart constructors, so
        // that the backend gets them at construction and this pass gets them
        // again once its operands have folded.
        Expr::Unary { op, operand } => Expr::un(op, fold(*operand)),
        Expr::Binary { op, lhs, rhs } => Expr::bin(op, fold(*lhs), fold(*rhs)),
        Expr::Cond { test, cons, alt } => Expr::cond(fold(*test), fold(*cons), fold(*alt)),
        Expr::Call { callee, args } => {
            let callee = fold(*callee);
            let args: Vec<Expr> = args.into_iter().map(fold).collect();
            // `Math.trunc` of something already known. Integer division emits
            // one of these, so this is what finishes folding `18 / 3`.
            if let (Expr::Member { obj, prop }, [Expr::Num(n)]) = (&callee, &args[..]) {
                if prop == "trunc" && matches!(&**obj, Expr::Ident(m) if m == "Math") {
                    return Expr::Num(n.trunc());
                }
            }
            Expr::Call { callee: Box::new(callee), args }
        }
        Expr::New { callee, args } => Expr::New {
            callee: Box::new(fold(*callee)),
            args: args.into_iter().map(fold).collect(),
        },
        Expr::Member { obj, prop } => Expr::Member { obj: Box::new(fold(*obj)), prop },
        Expr::Index { obj, index } => {
            let obj = fold(*obj);
            let index = fold(*index);
            // Reading a known slot out of a value built right here. Structs,
            // tuples and enum payloads are all arrays, so this is what an
            // inlined accessor leaves behind once its argument has been moved
            // to where it is read.
            if let (Expr::Array(items), Expr::Num(i)) = (&obj, &index) {
                let n = *i as usize;
                // A negative or fractional index is not a slot, and casting
                // one to `usize` would land on a slot that is.
                let slot = items.get(n).filter(|_| *i >= 0.0 && i.fract() == 0.0);
                if let Some(item) = slot {
                    // Only a scalar comes out. Lifting an aggregate out of a
                    // literal replaces one read of an array that was being
                    // built anyway with a *fresh* array at every occurrence —
                    // `ctx[1]` became `[]`, allocating on every call to
                    // something that ignores the argument.
                    if item.is_pure_literal()
                        && items.iter().enumerate().all(|(k, x)| k == n || x.is_pure())
                    {
                        return item.clone();
                    }
                }
            }
            Expr::Index { obj: Box::new(obj), index: Box::new(index) }
        }
        Expr::Array(xs) => Expr::Array(xs.into_iter().map(fold).collect()),
        Expr::Object(fs) => Expr::Object(fs.into_iter().map(|(k, v)| (k, fold(v))).collect()),
        Expr::Assign { target, value } => {
            Expr::Assign { target: Box::new(fold(*target)), value: Box::new(fold(*value)) }
        }
        Expr::Arrow { params, body } => Expr::Arrow { params, body: Box::new(fold(*body)) },
        Expr::ArrowBlock { params, body } => {
            let body = fold_block(body);
            // An arrow whose body is a single `return e` is just `=> e`.
            if let [Stmt::Return(Some(e))] = body.as_slice() {
                return Expr::Arrow { params, body: Box::new(e.clone()) };
            }
            Expr::ArrowBlock { params, body }
        }
        Expr::Seq(xs) => Expr::Seq(xs.into_iter().map(fold).collect()),
        Expr::Spread(x) => Expr::Spread(Box::new(fold(*x))),
        other => other,
    }
}

// -- shared constants --------------------------------------------------------

/// Replaces a known slot of a shared constant with the value in it.
///
/// The backend shares constant aggregates so that writing `Shape.Empty` in a
/// loop does not allocate an array per iteration. That is a win at run time
/// and a loss at compile time: the folder can turn `[3, 'x'][0]` into `3` and
/// can do nothing with `$k0[0]`. This puts the contents back within reach,
/// leaving the sharing in place for the reads that are still reads.
fn constant_table(stmts: &[Stmt]) -> HashMap<String, Vec<Expr>> {
    let mut table: HashMap<String, Vec<Expr>> = HashMap::default();
    for s in stmts {
        if let Stmt::Var { kind: VarKind::Const, name, init: Some(Expr::Array(items)) } = s {
            if name.starts_with("$k") {
                table.insert(name.clone(), items.clone());
            }
        }
    }
    table
}

fn read_through_stmt(s: Stmt, table: &HashMap<String, Vec<Expr>>) -> Stmt {
    match s {
        // The declarations themselves are left alone: one of them may name
        // another, and rewriting a constant into itself is not the point.
        Stmt::Var { kind: VarKind::Const, ref name, .. } if name.starts_with("$k") => s,
        Stmt::Var { kind, name, init } => {
            Stmt::Var { kind, name, init: init.map(|e| read_through_expr(e, table)) }
        }
        Stmt::Func { name, params, body } => Stmt::Func {
            name,
            params,
            body: body.into_iter().map(|s| read_through_stmt(s, table)).collect(),
        },
        Stmt::Return(e) => Stmt::Return(e.map(|e| read_through_expr(e, table))),
        Stmt::If { cond, then, else_ } => Stmt::If {
            cond: read_through_expr(cond, table),
            then: then.into_iter().map(|s| read_through_stmt(s, table)).collect(),
            else_: else_.into_iter().map(|s| read_through_stmt(s, table)).collect(),
        },
        Stmt::While { cond, body } => Stmt::While {
            cond: read_through_expr(cond, table),
            body: body.into_iter().map(|s| read_through_stmt(s, table)).collect(),
        },
        Stmt::Switch { disc, cases } => Stmt::Switch {
            disc: read_through_expr(disc, table),
            cases: cases
                .into_iter()
                .map(|(t, b)| {
                    (
                        t.map(|t| read_through_expr(t, table)),
                        b.into_iter().map(|s| read_through_stmt(s, table)).collect(),
                    )
                })
                .collect(),
        },
        Stmt::Expr(e) => Stmt::Expr(read_through_expr(e, table)),
        Stmt::Throw(e) => Stmt::Throw(read_through_expr(e, table)),
        Stmt::ExportDefault(e) => Stmt::ExportDefault(read_through_expr(e, table)),
        Stmt::Block(b) => {
            Stmt::Block(b.into_iter().map(|s| read_through_stmt(s, table)).collect())
        }
        other => other,
    }
}

fn read_through_expr(e: Expr, table: &HashMap<String, Vec<Expr>>) -> Expr {
    // `$k0[2]`, where slot 2 holds something free to copy.
    if let Expr::Index { obj, index } = &e {
        if let (Expr::Ident(name), Expr::Num(i)) = (&**obj, &**index) {
            if let Some(items) = table.get(name) {
                let n = *i as usize;
                let slot = items.get(n).filter(|_| *i >= 0.0 && i.fract() == 0.0);
                // Only a literal: copying a nested aggregate out of a shared
                // one would undo the sharing.
                if let Some(item) = slot.filter(|x| x.is_pure_literal()) {
                    return item.clone();
                }
            }
        }
    }
    match e {
        Expr::Array(xs) => {
            Expr::Array(xs.into_iter().map(|x| read_through_expr(x, table)).collect())
        }
        Expr::Seq(xs) => {
            Expr::Seq(xs.into_iter().map(|x| read_through_expr(x, table)).collect())
        }
        Expr::Object(fs) => Expr::Object(
            fs.into_iter().map(|(k, v)| (k, read_through_expr(v, table))).collect(),
        ),
        Expr::Member { obj, prop } => {
            Expr::Member { obj: Box::new(read_through_expr(*obj, table)), prop }
        }
        Expr::Index { obj, index } => Expr::Index {
            obj: Box::new(read_through_expr(*obj, table)),
            index: Box::new(read_through_expr(*index, table)),
        },
        Expr::Call { callee, args } => Expr::Call {
            callee: Box::new(read_through_expr(*callee, table)),
            args: args.into_iter().map(|a| read_through_expr(a, table)).collect(),
        },
        Expr::New { callee, args } => Expr::New {
            callee: Box::new(read_through_expr(*callee, table)),
            args: args.into_iter().map(|a| read_through_expr(a, table)).collect(),
        },
        Expr::Unary { op, operand } => Expr::un(op, read_through_expr(*operand, table)),
        Expr::Binary { op, lhs, rhs } => Expr::bin(
            op,
            read_through_expr(*lhs, table),
            read_through_expr(*rhs, table),
        ),
        Expr::Cond { test, cons, alt } => Expr::cond(
            read_through_expr(*test, table),
            read_through_expr(*cons, table),
            read_through_expr(*alt, table),
        ),
        Expr::Assign { target, value } => Expr::Assign {
            target: Box::new(read_through_expr(*target, table)),
            value: Box::new(read_through_expr(*value, table)),
        },
        Expr::Arrow { params, body } => {
            Expr::Arrow { params, body: Box::new(read_through_expr(*body, table)) }
        }
        Expr::ArrowBlock { params, body } => Expr::ArrowBlock {
            params,
            body: body.into_iter().map(|s| read_through_stmt(s, table)).collect(),
        },
        Expr::Spread(x) => Expr::Spread(Box::new(read_through_expr(*x, table))),
        other => other,
    }
}

// -- switch generation -------------------------------------------------------
//
// A match compiles to a chain of `if`/`else if`, one test per arm, so reaching
// the sixth variant of a six-variant enum performs six comparisons. Where every
// test in a chain compares the *same* expression against a literal, the chain
// is a `switch` — one dispatch instead of a scan, and one mention of the
// discriminant instead of one per arm.
//
// Recognising the shape here rather than in the backend means it applies
// whatever produced the chain, and after the peepholes and local cleanup have
// had their say — by which point the discriminant is a settled expression like
// `s_0[0]` rather than a temporary that still has to be matched through.

/// A chain of at least this many literal tests is worth a `switch`. Two cases
/// print shorter as an `if`/`else`.
const MIN_SWITCH_CASES: usize = 3;

/// The discriminant and the literal, for a test of the form `x === 1`.
fn equality_test(e: &Expr) -> Option<(&Expr, &Expr)> {
    let Expr::Binary { op: BinOp::StrictEq, lhs, rhs } = e else { return None };
    if !rhs.is_pure_literal() || matches!(**rhs, Expr::Ident(_)) {
        return None;
    }
    // A discriminant is read once per dispatch, so it has to be free of
    // effects and cheap to name.
    if !lhs.is_pure() {
        return None;
    }
    Some((lhs, rhs))
}

/// What every `Some` out of [`case_labels`] holds: it answers with the labels
/// of a test it recognised, and a test it recognised has at least one.
const LABELS_NONEMPTY: &str = "case_labels answers `Some` only with a label in hand";

/// The literals one arm accepts: `x === 1` gives one, `x === 1 || x === 2`
/// gives two, all against the same discriminant.
fn case_labels<'a>(test: &'a Expr, disc: &mut Option<&'a Expr>) -> Option<Vec<&'a Expr>> {
    if let Expr::Binary { op: BinOp::Or, lhs, rhs } = test {
        let mut out = case_labels(lhs, disc)?;
        out.extend(case_labels(rhs, disc)?);
        return Some(out);
    }
    let (d, lit) = equality_test(test)?;
    match disc {
        Some(seen) if !seen.same_as(d) => None,
        Some(_) => Some(vec![lit]),
        None => {
            *disc = Some(d);
            Some(vec![lit])
        }
    }
}

/// Rewrites `return d === 1 ? a : d === 2 ? b : c` into a `switch`.
///
/// The same chain as below, after folding has turned each arm's `return` into
/// an arm of a conditional. It is compact, and it is a linear scan: ten arms
/// means up to ten comparisons where a `switch` is one dispatch. Only the
/// `return` form is taken, because there each case is a `return` and needs no
/// `break`, so the switch is barely longer than the chain it replaces.
fn cond_chain_to_switch(s: Stmt) -> Stmt {
    let Stmt::Return(Some(top @ Expr::Cond { .. })) = &s else { return s };

    let mut disc: Option<&Expr> = None;
    let mut arms: Vec<(Vec<&Expr>, &Expr)> = Vec::new();
    let mut cursor = top;
    let default: &Expr = loop {
        let Expr::Cond { test, cons, alt } = cursor else { break cursor };
        let Some(labels) = case_labels(test, &mut disc) else { return s };
        arms.push((labels, cons));
        cursor = alt;
    };
    if arms.len() < MIN_SWITCH_CASES {
        return s;
    }
    let Some(disc) = disc else { return s };
    if !distinct(&arms) {
        return s;
    }

    let mut cases: Vec<(Option<Expr>, Vec<Stmt>)> = Vec::new();
    for (labels, value) in &arms {
        let (last, rest) = labels.split_last().or_ice(LABELS_NONEMPTY);
        for l in rest {
            cases.push((Some((*l).clone()), Vec::new()));
        }
        cases.push((Some((*last).clone()), vec![Stmt::Return(Some((*value).clone()))]));
    }
    cases.push((None, vec![Stmt::Return(Some(default.clone()))]));
    Stmt::Switch { disc: disc.clone(), cases }
}

/// Whether no literal appears in two arms. A repeated `case` is a syntax
/// error, not a fallthrough.
fn distinct<T>(arms: &[(Vec<&Expr>, T)]) -> bool {
    let mut seen: Vec<&Expr> = Vec::new();
    for (labels, _) in arms {
        for l in labels {
            if seen.iter().any(|s| s.same_as(l)) {
                return false;
            }
            seen.push(l);
        }
    }
    true
}

/// Rewrites an `if`/`else if` chain over one discriminant into a `switch`.
fn to_switch(s: Stmt) -> Stmt {
    let Stmt::If { .. } = &s else { return cond_chain_to_switch(s) };

    // Walk the chain, collecting each arm's labels, and stop at whatever the
    // final `else` turns out to be.
    let mut disc: Option<&Expr> = None;
    let mut arms: Vec<(Vec<&Expr>, &Vec<Stmt>)> = Vec::new();
    let mut cursor = &s;
    let default: &[Stmt] = loop {
        match cursor {
            Stmt::If { cond, then, else_ } => {
                let Some(labels) = case_labels(cond, &mut disc) else { return s };
                arms.push((labels, then));
                match else_.as_slice() {
                    [next @ Stmt::If { .. }] => cursor = next,
                    rest => break rest,
                }
            }
            _ => break &[],
        }
    };
    if arms.len() < MIN_SWITCH_CASES {
        return s;
    }
    let Some(disc) = disc else { return s };

    if !distinct(&arms) {
        return s;
    }

    let mut cases: Vec<(Option<Expr>, Vec<Stmt>)> = Vec::new();
    for (labels, body) in &arms {
        let (last, rest) = labels.split_last().or_ice(LABELS_NONEMPTY);
        // Several literals reaching one body are several empty cases falling
        // through into it.
        for l in rest {
            cases.push((Some((*l).clone()), Vec::new()));
        }
        cases.push((Some((*last).clone()), switch_body(body)));
    }
    if !default.is_empty() {
        cases.push((None, switch_body(default)));
    }
    Stmt::Switch { disc: disc.clone(), cases: merge_equal_cases(cases) }
}

/// Turns runs of cases with the same body into one body reached by several
/// labels.
///
/// Arms that do the same thing are ordinary — a five-variant enum where four
/// variants are handled alike writes the same body four times, and each copy
/// is real output. Emptying all but the last makes them fall through to it,
/// which is what the source said in the first place.
///
/// Only a body ending in a jump may be shared this way. One that runs on would
/// reach the *next* case's body rather than its own once emptied, which is a
/// different program.
fn merge_equal_cases(
    cases: Vec<(Option<Expr>, Vec<Stmt>)>,
) -> Vec<(Option<Expr>, Vec<Stmt>)> {
    let printed: Vec<String> =
        cases.iter().map(|(_, b)| print(b, false)).collect();
    let mut out: Vec<(Option<Expr>, Vec<Stmt>)> = Vec::with_capacity(cases.len());
    for (i, (label, body)) in cases.into_iter().enumerate() {
        let same_as_next = printed
            .get(i.saturating_add(1))
            .is_some_and(|next| printed.get(i).is_some_and(|here| next == here));
        let shareable = !body.is_empty() && ends_in_jump(&body) && same_as_next;
        if shareable && label.is_some() {
            out.push((label, Vec::new()));
        } else {
            out.push((label, body));
        }
    }
    out
}

/// A case body, wrapped so it is its own scope and cannot run on.
///
/// Both halves matter. Cases share one lexical scope, so two of them declaring
/// the same name is a `SyntaxError` — and worse, `rename_scope` gives each case
/// a cloned map, so the mangler can produce that collision in a release build
/// while debug passes. And a case that does not end in a jump falls into the
/// next one.
fn switch_body(body: &[Stmt]) -> Vec<Stmt> {
    let mut out = vec![Stmt::Block(body.to_vec())];
    if !ends_in_jump(body) {
        out.push(Stmt::Break);
    }
    out
}

fn ends_in_jump(body: &[Stmt]) -> bool {
    match body.last() {
        Some(Stmt::Return(_) | Stmt::Throw(_) | Stmt::Break | Stmt::Continue) => true,
        Some(Stmt::Block(inner)) => ends_in_jump(inner),
        _ => false,
    }
}

/// Applies `to_switch` everywhere a statement can appear.
///
/// Conversion happens *before* the recursion, not after. A chain is a single
/// `if` whose `else` holds the next one, so rewriting the inside first turns
/// the tail into a `Switch` — and the head can no longer see a chain to join.
/// The visible symptom is a five-arm match compiling to two `if`s and a
/// three-case switch instead of one five-case switch.
fn switches(body: Vec<Stmt>) -> Vec<Stmt> {
    body.into_iter()
        .map(|s| match to_switch(s) {
            Stmt::If { cond, then, else_ } => {
                Stmt::If { cond, then: switches(then), else_: switches(else_) }
            }
            Stmt::While { cond, body } => Stmt::While { cond, body: switches(body) },
            Stmt::Block(b) => Stmt::Block(switches(b)),
            Stmt::Func { name, params, body } => {
                Stmt::Func { name, params, body: switches(body) }
            }
            Stmt::Switch { disc, cases } => Stmt::Switch {
                disc,
                cases: cases.into_iter().map(|(t, b)| (t, switches(b))).collect(),
            },
            other => other,
        })
        .collect()
}

// -- local cleanup -----------------------------------------------------------
//
// Dead code elimination below works on top-level declarations. This works
// inside a function body, where the backend leaves a temporary for every match
// scrutinee, every value-position branch and every pattern binding.
//
// A whole function body is treated as one scope. That is sound because the
// backend names locals from a per-function counter (`generate::local_name`) and
// temporaries from a per-function sequence, so no name is declared twice —
// which `declared` checks rather than assumes.

#[derive(Default)]
struct LocalFacts {
    /// How many times each name is *declared*. Anything but once disqualifies
    /// it: the pass reasons about one binding, not a name.
    declared: HashMap<String, usize>,
    /// How many times each name is assigned. Assigned exactly once, a
    /// declaration and its assignment are one binding written apart; assigned
    /// at all, a name is neither an alias nor removable and its declaration
    /// has to stay even when nothing reads it.
    ///
    /// There used to be a `HashSet` of assigned names beside this map, holding
    /// exactly its keys. The two gated different decisions — the set gated
    /// `depends_on_assigned`, which is the soundness condition for moving a
    /// value across a reassignment, and the map gated `merge_declarations` —
    /// so a name in one and not the other was the tail-call-loop miscompile
    /// this pass exists to avoid.
    assigns: HashMap<String, usize>,
    /// How many times each name is *read*.
    uses: HashMap<String, usize>,
    /// How many loops and closures enclose the point being walked.
    depth: usize,
    /// The depth each name was declared at, and the deepest any read of it
    /// sits. A value read deeper than it was bound would, if moved to its use,
    /// be computed once per iteration or once per call instead of once.
    decl_depth: HashMap<String, usize>,
    use_depth: HashMap<String, usize>,
    /// Set when something in the body cannot be reasoned about, in which case
    /// the body is left exactly as it was.
    opaque: bool,
}

impl LocalFacts {
    // `get_mut` before `insert` rather than `entry`, in all three of these: the
    // entry API needs an owned key whether or not the name is already there,
    // and these run over every identifier of every body, four times a body.
    // A name is spelled once here however often it occurs.
    fn read(&mut self, name: &str) {
        let d = self.depth;
        match self.uses.get_mut(name) {
            Some(n) => *n += 1,
            None => {
                self.uses.insert(name.to_string(), 1);
            }
        }
        match self.use_depth.get_mut(name) {
            Some(x) => *x = (*x).max(d),
            None => {
                self.use_depth.insert(name.to_string(), d);
            }
        }
    }

    fn declare(&mut self, name: &str) {
        let d = self.depth;
        match self.declared.get_mut(name) {
            Some(n) => *n += 1,
            None => {
                self.declared.insert(name.to_string(), 1);
            }
        }
        match self.decl_depth.get_mut(name) {
            Some(x) => *x = d,
            None => {
                self.decl_depth.insert(name.to_string(), d);
            }
        }
    }

    fn assign(&mut self, name: &str) {
        match self.assigns.get_mut(name) {
            Some(n) => *n += 1,
            None => {
                self.assigns.insert(name.to_string(), 1);
            }
        }
    }

    /// Whether every read of `name` sits no deeper than its binding.
    fn read_where_bound(&self, name: &str) -> bool {
        let decl = self.decl_depth.get(name).copied().unwrap_or(0);
        self.use_depth.get(name).copied().unwrap_or(0) <= decl
    }
}

/// The identifiers an expression reads. Used to check that nothing it depends
/// on is reassigned between where it is bound and where it would be moved to.
fn reads_of(e: &Expr, out: &mut HashSet<String>) {
    let mut f = LocalFacts::default();
    count_expr(e, &mut f);
    out.extend(f.uses.into_keys());
}

impl LocalFacts {
    /// Whether a name is ever the target of an assignment.
    fn is_assigned(&self, name: &str) -> bool {
        self.assigns.contains_key(name)
    }
}

fn count_expr(e: &Expr, f: &mut LocalFacts) {
    match e {
        Expr::Ident(name) => f.read(name),
        Expr::Assign { target, value } => {
            match &**target {
                // The target of a plain assignment is written, not read.
                Expr::Ident(name) => f.assign(name),
                other => count_expr(other, f),
            }
            count_expr(value, f);
        }
        Expr::Array(xs) | Expr::Seq(xs) => xs.iter().for_each(|x| count_expr(x, f)),
        Expr::Object(fs) => fs.iter().for_each(|(_, v)| count_expr(v, f)),
        Expr::Member { obj, .. } => count_expr(obj, f),
        Expr::Index { obj, index } => {
            count_expr(obj, f);
            count_expr(index, f);
        }
        Expr::Call { callee, args } | Expr::New { callee, args } => {
            count_expr(callee, f);
            args.iter().for_each(|a| count_expr(a, f));
        }
        Expr::Unary { operand, .. } => count_expr(operand, f),
        Expr::Binary { lhs, rhs, .. } => {
            count_expr(lhs, f);
            count_expr(rhs, f);
        }
        Expr::Cond { test, cons, alt } => {
            count_expr(test, f);
            count_expr(cons, f);
            count_expr(alt, f);
        }
        // A closure body runs an unknown number of times, so anything read
        // inside it counts as read deeper than the enclosing code.
        Expr::Arrow { params, body } => {
            params.iter().for_each(|p| f.declare(p));
            f.depth += 1;
            count_expr(body, f);
            f.depth -= 1;
        }
        Expr::ArrowBlock { params, body } => {
            params.iter().for_each(|p| f.declare(p));
            f.depth += 1;
            body.iter().for_each(|s| count_stmt(s, f));
            f.depth -= 1;
        }
        Expr::Spread(x) => count_expr(x, f),
        _ => {}
    }
}

fn count_stmt(s: &Stmt, f: &mut LocalFacts) {
    match s {
        Stmt::Var { name, init, .. } => {
            f.declare(name);
            if let Some(e) = init {
                count_expr(e, f);
            }
        }
        Stmt::Func { name, params, body } => {
            f.declare(name);
            params.iter().for_each(|p| f.declare(p));
            body.iter().for_each(|s| count_stmt(s, f));
        }
        Stmt::Return(e) => {
            if let Some(e) = e {
                count_expr(e, f);
            }
        }
        Stmt::If { cond, then, else_ } => {
            count_expr(cond, f);
            then.iter().for_each(|s| count_stmt(s, f));
            else_.iter().for_each(|s| count_stmt(s, f));
        }
        Stmt::While { cond, body } => {
            count_expr(cond, f);
            f.depth += 1;
            body.iter().for_each(|s| count_stmt(s, f));
            f.depth -= 1;
        }
        Stmt::Switch { disc, cases } => {
            count_expr(disc, f);
            for (t, b) in cases {
                if let Some(t) = t {
                    count_expr(t, f);
                }
                b.iter().for_each(|s| count_stmt(s, f));
            }
        }
        Stmt::Expr(e) | Stmt::Throw(e) | Stmt::ExportDefault(e) => count_expr(e, f),
        Stmt::Block(b) => b.iter().for_each(|s| count_stmt(s, f)),
        // Verbatim source names identifiers this pass cannot see, so a body
        // holding any is left exactly as it was rather than guessed at.
        Stmt::Raw(_) | Stmt::RawDecl { .. } => f.opaque = true,
        Stmt::Break | Stmt::Continue => {}
    }
}

/// Where a substitution's values come from, and whether pasting one consumes
/// it.
///
/// A value bound to a name that is read once may be *moved* to that read rather
/// than copied there. Copying is what a substitution normally has to do — a
/// name read twice needs its value twice — so the moving form carries the set of
/// names its caller has proved single-read, and every other name is still
/// copied.
enum Subst<'a> {
    Copying(&'a HashMap<String, Expr>),
    Moving { values: &'a mut HashMap<String, Expr>, movable: &'a HashSet<String> },
}

impl Subst<'_> {
    fn value(&mut self, name: &str) -> Option<Expr> {
        match self {
            Subst::Copying(map) => map.get(name).cloned(),
            Subst::Moving { values, movable } => {
                if movable.contains(name) {
                    values.remove(name)
                } else {
                    values.get(name).cloned()
                }
            }
        }
    }
}

fn subst_stmt(s: Stmt, map: &HashMap<String, Expr>) -> Stmt {
    subst_stmt_with(s, &mut Subst::Copying(map))
}

fn subst_expr_with(e: Expr, map: &mut Subst) -> Expr {
    match e {
        Expr::Ident(name) => match map.value(&name) {
            Some(v) => v,
            None => Expr::Ident(name),
        },
        Expr::Assign { target, value } => {
            // The target names a storage location, not a value, so it is never
            // rewritten — only what is assigned to it.
            let target = match *target {
                Expr::Ident(n) => Expr::Ident(n),
                other => subst_expr_with(other, map),
            };
            Expr::Assign { target: Box::new(target), value: Box::new(subst_expr_with(*value, map)) }
        }
        Expr::Array(xs) => Expr::Array(xs.into_iter().map(|x| subst_expr_with(x, map)).collect()),
        Expr::Seq(xs) => Expr::Seq(xs.into_iter().map(|x| subst_expr_with(x, map)).collect()),
        Expr::Object(fs) => {
            Expr::Object(fs.into_iter().map(|(k, v)| (k, subst_expr_with(v, map))).collect())
        }
        Expr::Member { obj, prop } => Expr::Member { obj: Box::new(subst_expr_with(*obj, map)), prop },
        Expr::Index { obj, index } => Expr::Index {
            obj: Box::new(subst_expr_with(*obj, map)),
            index: Box::new(subst_expr_with(*index, map)),
        },
        Expr::Call { callee, args } => Expr::Call {
            callee: Box::new(subst_expr_with(*callee, map)),
            args: args.into_iter().map(|a| subst_expr_with(a, map)).collect(),
        },
        Expr::New { callee, args } => Expr::New {
            callee: Box::new(subst_expr_with(*callee, map)),
            args: args.into_iter().map(|a| subst_expr_with(a, map)).collect(),
        },
        Expr::Unary { op, operand } => Expr::un(op, subst_expr_with(*operand, map)),
        Expr::Binary { op, lhs, rhs } => {
            Expr::bin(op, subst_expr_with(*lhs, map), subst_expr_with(*rhs, map))
        }
        Expr::Cond { test, cons, alt } => Expr::cond(
            subst_expr_with(*test, map),
            subst_expr_with(*cons, map),
            subst_expr_with(*alt, map),
        ),
        Expr::Arrow { params, body } => {
            Expr::Arrow { params, body: Box::new(subst_expr_with(*body, map)) }
        }
        Expr::ArrowBlock { params, body } => {
            let body: Vec<Stmt> = body.into_iter().map(|s| subst_stmt_with(s, map)).collect();
            // Rebuilding may leave a body that is one `return`, which is the
            // concise form.
            if let [Stmt::Return(Some(e))] = &body[..] {
                return Expr::Arrow { params, body: Box::new(e.clone()) };
            }
            Expr::ArrowBlock { params, body }
        }
        Expr::Spread(x) => Expr::Spread(Box::new(subst_expr_with(*x, map))),
        other => other,
    }
}

fn subst_stmt_with(s: Stmt, map: &mut Subst) -> Stmt {
    match s {
        Stmt::Var { kind, name, init } => {
            Stmt::Var { kind, name, init: init.map(|e| subst_expr_with(e, map)) }
        }
        Stmt::Func { name, params, body } => Stmt::Func {
            name,
            params,
            body: body.into_iter().map(|s| subst_stmt_with(s, map)).collect(),
        },
        Stmt::Return(e) => Stmt::Return(e.map(|e| subst_expr_with(e, map))),
        Stmt::If { cond, then, else_ } => Stmt::If {
            cond: subst_expr_with(cond, map),
            then: then.into_iter().map(|s| subst_stmt_with(s, map)).collect(),
            else_: else_.into_iter().map(|s| subst_stmt_with(s, map)).collect(),
        },
        Stmt::While { cond, body } => Stmt::While {
            cond: subst_expr_with(cond, map),
            body: body.into_iter().map(|s| subst_stmt_with(s, map)).collect(),
        },
        Stmt::Switch { disc, cases } => Stmt::Switch {
            disc: subst_expr_with(disc, map),
            cases: cases
                .into_iter()
                .map(|(t, b)| {
                    (
                        t.map(|t| subst_expr_with(t, map)),
                        b.into_iter().map(|s| subst_stmt_with(s, map)).collect(),
                    )
                })
                .collect(),
        },
        Stmt::Expr(e) => Stmt::Expr(subst_expr_with(e, map)),
        Stmt::Throw(e) => Stmt::Throw(subst_expr_with(e, map)),
        Stmt::ExportDefault(e) => Stmt::ExportDefault(subst_expr_with(e, map)),
        Stmt::Block(b) => Stmt::Block(b.into_iter().map(|s| subst_stmt_with(s, map)).collect()),
        other => other,
    }
}

/// Removes the declarations `drop` names, keeping any value that still has
/// work to do as a bare expression statement.
fn drop_bindings(body: Vec<Stmt>, drop: &HashSet<String>) -> Vec<Stmt> {
    let mut out = Vec::new();
    for s in body {
        let s = match s {
            Stmt::Var { name, init, .. } if drop.contains(&name) => match init {
                Some(e) if !e.is_pure() => Stmt::Expr(drop_in_expr(e, drop)),
                _ => continue,
            },
            Stmt::Var { kind, name, init } => {
                Stmt::Var { kind, name, init: init.map(|e| drop_in_expr(e, drop)) }
            }
            Stmt::If { cond, then, else_ } => Stmt::If {
                cond: drop_in_expr(cond, drop),
                then: drop_bindings(then, drop),
                else_: drop_bindings(else_, drop),
            },
            Stmt::While { cond, body } => Stmt::While {
                cond: drop_in_expr(cond, drop),
                body: drop_bindings(body, drop),
            },
            Stmt::Switch { disc, cases } => Stmt::Switch {
                disc: drop_in_expr(disc, drop),
                cases: cases.into_iter().map(|(t, b)| (t, drop_bindings(b, drop))).collect(),
            },
            Stmt::Block(b) => Stmt::Block(drop_bindings(b, drop)),
            Stmt::Func { name, params, body } => {
                Stmt::Func { name, params, body: drop_bindings(body, drop) }
            }
            Stmt::Return(e) => Stmt::Return(e.map(|e| drop_in_expr(e, drop))),
            Stmt::Expr(e) => Stmt::Expr(drop_in_expr(e, drop)),
            Stmt::Throw(e) => Stmt::Throw(drop_in_expr(e, drop)),
            Stmt::ExportDefault(e) => Stmt::ExportDefault(drop_in_expr(e, drop)),
            other => other,
        };
        out.push(s);
    }
    out
}

/// The same removal, inside the closures an expression holds — which is where
/// `collect_cleanup` now also looks, and a binding it marked dead there has to
/// actually go.
fn drop_in_expr(e: Expr, drop: &HashSet<String>) -> Expr {
    let go = |x: Box<Expr>| Box::new(drop_in_expr(*x, drop));
    match e {
        Expr::ArrowBlock { params, body } => {
            let body = drop_bindings(body, drop);
            // Rebuilding may leave a body that is one `return`, which is the
            // concise form — the same collapse `subst_expr` makes.
            if let [Stmt::Return(Some(x))] = &body[..] {
                return Expr::Arrow { params, body: Box::new(x.clone()) };
            }
            Expr::ArrowBlock { params, body }
        }
        Expr::Arrow { params, body } => Expr::Arrow { params, body: go(body) },
        Expr::Array(xs) => Expr::Array(xs.into_iter().map(|x| drop_in_expr(x, drop)).collect()),
        Expr::Seq(xs) => Expr::Seq(xs.into_iter().map(|x| drop_in_expr(x, drop)).collect()),
        Expr::Object(fs) => {
            Expr::Object(fs.into_iter().map(|(k, v)| (k, drop_in_expr(v, drop))).collect())
        }
        Expr::Member { obj, prop } => Expr::Member { obj: go(obj), prop },
        Expr::Index { obj, index } => Expr::Index { obj: go(obj), index: go(index) },
        Expr::Call { callee, args } => Expr::Call {
            callee: go(callee),
            args: args.into_iter().map(|a| drop_in_expr(a, drop)).collect(),
        },
        Expr::New { callee, args } => Expr::New {
            callee: go(callee),
            args: args.into_iter().map(|a| drop_in_expr(a, drop)).collect(),
        },
        Expr::Unary { op, operand } => Expr::Unary { op, operand: go(operand) },
        Expr::Binary { op, lhs, rhs } => Expr::Binary { op, lhs: go(lhs), rhs: go(rhs) },
        Expr::Cond { test, cons, alt } => {
            Expr::Cond { test: go(test), cons: go(cons), alt: go(alt) }
        }
        Expr::Assign { target, value } => Expr::Assign { target, value: go(value) },
        Expr::Spread(x) => Expr::Spread(go(x)),
        other => other,
    }
}

fn collect_cleanup(
    body: &[Stmt],
    facts: &LocalFacts,
    map: &mut HashMap<String, Expr>,
    dead: &mut HashSet<String>,
) {
    for s in body {
        match s {
            Stmt::Var { name, init, .. } => {
                // A name declared twice, or ever assigned, is not one binding
                // and nothing here applies to it.
                if facts.declared.get(name).copied() != Some(1)
                    || facts.is_assigned(name)
                {
                    continue;
                }
                let uses = facts.uses.get(name).copied().unwrap_or(0);
                match init {
                    // Never read: the binding goes, and the value with it when
                    // it has nothing to do.
                    _ if uses == 0 => {
                        dead.insert(name.clone());
                    }
                    // An alias to a name that is never reassigned denotes the
                    // same value everywhere the alias does, however often it
                    // is read. This is the `const t = x;` the backend emits
                    // before every match.
                    Some(Expr::Ident(src))
                        if !facts.is_assigned(src)
                            && facts.declared.get(src).copied().unwrap_or(1) == 1 =>
                    {
                        map.insert(name.clone(), Expr::Ident(src.clone()));
                        dead.insert(name.clone());
                    }
                    // Read exactly once, so moving the value to its use
                    // duplicates nothing. Three conditions make the move
                    // legal:
                    //
                    //  * the value is pure, so running it later — or, if the
                    //    use is under a branch, not at all — is unobservable;
                    //  * nothing it reads is ever reassigned, which is what
                    //    stops a tail-call loop's rebinding from being read
                    //    after the parameter it names has moved on;
                    //  * the use is no deeper than the binding, so the work
                    //    does not move inside a loop or a closure.
                    Some(e)
                        if uses == 1
                            && e.is_pure()
                            && facts.read_where_bound(name)
                            && !depends_on_assigned(e, facts) =>
                    {
                        map.insert(name.clone(), e.clone());
                        dead.insert(name.clone());
                    }
                    _ => {}
                }
            }
            Stmt::If { then, else_, .. } => {
                collect_cleanup(then, facts, map, dead);
                collect_cleanup(else_, facts, map, dead);
            }
            Stmt::While { body, .. } => collect_cleanup(body, facts, map, dead),
            Stmt::Switch { cases, .. } => {
                for (_, b) in cases {
                    collect_cleanup(b, facts, map, dead);
                }
            }
            Stmt::Block(b) => collect_cleanup(b, facts, map, dead),
            Stmt::Func { body, .. } => collect_cleanup(body, facts, map, dead),
            _ => {}
        }
        // A lambda's body is a statement list like any other, and it is where
        // inlining leaves the most bindings behind — but it hangs off an
        // *expression*, so a walk over statements alone never reaches it. The
        // facts were collected over the whole body, including the closures, so
        // everything decided here is decided on the same information.
        stmt_exprs(s, &mut |e| expr_cleanup(e, facts, map, dead));
    }
}

/// Applies `f` to the expressions a statement holds directly. The statements
/// nested in it are the caller's business.
fn stmt_exprs(s: &Stmt, f: &mut impl FnMut(&Expr)) {
    match s {
        Stmt::Var { init: Some(e), .. }
        | Stmt::Return(Some(e))
        | Stmt::Expr(e)
        | Stmt::Throw(e)
        | Stmt::ExportDefault(e) => f(e),
        Stmt::If { cond, .. } | Stmt::While { cond, .. } => f(cond),
        Stmt::Switch { disc, cases } => {
            f(disc);
            for (label, _) in cases {
                if let Some(l) = label {
                    f(l);
                }
            }
        }
        _ => {}
    }
}

/// Finds the closures in an expression and cleans up inside them.
fn expr_cleanup(
    e: &Expr,
    facts: &LocalFacts,
    map: &mut HashMap<String, Expr>,
    dead: &mut HashSet<String>,
) {
    let mut go = |x: &Expr| expr_cleanup(x, facts, map, dead);
    match e {
        Expr::ArrowBlock { body, .. } => collect_cleanup(body, facts, map, dead),
        Expr::Arrow { body, .. } => go(body),
        Expr::Array(xs) | Expr::Seq(xs) => xs.iter().for_each(go),
        Expr::Object(fs) => fs.iter().for_each(|(_, v)| go(v)),
        Expr::Member { obj, .. } => go(obj),
        Expr::Index { obj, index } => {
            expr_cleanup(obj, facts, map, dead);
            expr_cleanup(index, facts, map, dead);
        }
        Expr::Call { callee, args } | Expr::New { callee, args } => {
            expr_cleanup(callee, facts, map, dead);
            args.iter().for_each(|a| expr_cleanup(a, facts, map, dead));
        }
        Expr::Unary { operand, .. } => go(operand),
        Expr::Binary { lhs, rhs, .. } => {
            expr_cleanup(lhs, facts, map, dead);
            expr_cleanup(rhs, facts, map, dead);
        }
        Expr::Cond { test, cons, alt } => {
            expr_cleanup(test, facts, map, dead);
            expr_cleanup(cons, facts, map, dead);
            expr_cleanup(alt, facts, map, dead);
        }
        Expr::Assign { target, value } => {
            expr_cleanup(target, facts, map, dead);
            expr_cleanup(value, facts, map, dead);
        }
        Expr::Spread(x) => go(x),
        _ => {}
    }
}

/// Whether any name this expression reads is reassigned somewhere in the body.
///
/// This is what makes moving a value to its use safe in a tail-call loop:
/// `const t = n - 1; n = t;` must not become `n = n - 1` if any read of `t`
/// sits after the assignment.
fn depends_on_assigned(e: &Expr, facts: &LocalFacts) -> bool {
    let mut names = HashSet::default();
    reads_of(e, &mut names);
    names.iter().any(|n| facts.is_assigned(n))
}

/// Substitutes the map into its own values, so one pass over the body
/// suffices.
///
/// This has to reach *inside* a value, not merely follow `a -> b -> c`. Both
/// halves of
///
/// ```js
/// const t = o;          // t -> o
/// const v = t[1];       // v -> t[1]
/// ```
///
/// are collected in one round, and `v` must resolve to `o[1]`: substituting
/// `t[1]` verbatim would name a binding that this same round removes.
/// A resolved value is *moved* to the one place that reads it rather than
/// copied there, wherever that is provably the only place left.
///
/// A binding read once in the whole body, from inside another binding's value,
/// has no reader left once that value has been pasted: its own declaration is
/// in the same round's `dead` set and goes with the rest, and its one read went
/// with the declaration that held it. Copying instead would make a chain of `n`
/// such bindings — which is what a long `let` chain lowers to — cost `n²` nodes
/// to build and `n²` more to drop, for a result that is `n` nodes long.
///
/// The names this holds of are exactly the ones a resolution is allowed to take
/// out of `done` rather than clone, and they are the ones absent from the map
/// it leaves behind. That absence is what the caller wants: substituting them
/// back into the body would paste into declarations the same round removes.
fn movable_values(
    map: &HashMap<String, Expr>,
    uses: &HashMap<String, usize>,
) -> HashSet<String> {
    let mut refs: HashMap<&str, usize> = HashMap::default();
    for value in map.values() {
        let mut reads = HashSet::default();
        reads_of(value, &mut reads);
        for r in reads {
            if let Some((name, _)) = map.get_key_value(&r) {
                *refs.entry(name.as_str()).or_insert(0) += 1;
            }
        }
    }
    refs.into_iter()
        .filter(|&(name, n)| n == 1 && uses.get(name).copied() == Some(1))
        .map(|(name, _)| name.to_string())
        .collect()
}

fn resolve_map(map: &mut HashMap<String, Expr>, uses: &HashMap<String, usize>) {
    let names: Vec<String> = map.keys().cloned().collect();
    let movable = movable_values(map, uses);
    let mut done: HashMap<String, Expr> = HashMap::default();
    let mut taken: HashSet<String> = HashSet::default();
    let mut visiting: HashSet<String> = HashSet::default();
    for name in &names {
        resolve_one(name, map, &movable, &mut done, &mut taken, &mut visiting);
    }
    *map = done;
}

/// Leaves `done[name]` holding `name`'s resolved value, unless `name` is
/// movable and its one reader has already taken it.
fn resolve_one(
    name: &str,
    map: &HashMap<String, Expr>,
    movable: &HashSet<String>,
    done: &mut HashMap<String, Expr>,
    taken: &mut HashSet<String>,
    visiting: &mut HashSet<String>,
) {
    if done.contains_key(name) || taken.contains(name) {
        return;
    }
    let Some(raw) = map.get(name).cloned() else {
        return;
    };
    // A binding cannot name itself — that would not have run — but a bound is
    // cheaper than trusting it.
    if !visiting.insert(name.to_string()) {
        return;
    }

    let mut reads = HashSet::default();
    reads_of(&raw, &mut reads);
    // Sorted, so the order values are resolved in cannot depend on hash order.
    let mut reads: Vec<String> = reads.into_iter().filter(|r| map.contains_key(r)).collect();
    reads.sort();
    for r in &reads {
        resolve_one(r, map, movable, done, taken, visiting);
    }
    visiting.remove(name);

    // A movable value leaves `done` for the one place that reads it, so the
    // name it was under is settled and must not be resolved a second time.
    // Everything else stays where it is and is copied, as a substitution
    // normally must.
    for r in &reads {
        if movable.contains(r) && done.contains_key(r) {
            taken.insert(r.clone());
        }
    }
    let out = if reads.is_empty() {
        raw
    } else {
        subst_expr_with(raw, &mut Subst::Moving { values: done, movable })
    };
    done.insert(name.to_string(), out);
}

/// `let x; x = e;` is `const x = e`.
///
/// The backend declares a slot before a branch and assigns it inside, because
/// the branches are statements. Once folding has turned a branch into an
/// expression — or removed all but one of them — the two halves sit next to
/// each other, and rejoining them is what lets everything else here apply:
/// until it happens the name is "assigned", which disqualifies it from every
/// rule above.
fn merge_declarations(body: &mut Vec<Stmt>, facts: &LocalFacts) -> bool {
    let mut changed = false;
    for s in body.iter_mut() {
        changed |= match s {
            Stmt::If { then, else_, .. } => {
                merge_declarations(then, facts) | merge_declarations(else_, facts)
            }
            Stmt::While { body, .. } => merge_declarations(body, facts),
            Stmt::Block(b) => merge_declarations(b, facts),
            Stmt::Func { body, .. } => merge_declarations(body, facts),
            Stmt::Switch { cases, .. } => {
                cases.iter_mut().fold(false, |a, (_, b)| a | merge_declarations(b, facts))
            }
            _ => false,
        };
    }

    let mut i = 0;
    while i + 1 < body.len() {
        let next = i + 1;
        let Some(Stmt::Var { kind: VarKind::Let, name, init: None }) = body.get(i) else {
            i = next;
            continue;
        };
        let Some(Stmt::Expr(Expr::Assign { target, .. })) = body.get(next) else {
            i = next;
            continue;
        };
        // Assigned anywhere else and the slot is genuinely a slot.
        if !matches!(&**target, Expr::Ident(t) if t == name)
            || facts.assigns.get(name).copied() != Some(1)
        {
            i = next;
            continue;
        }
        let name = name.clone();
        let Stmt::Expr(Expr::Assign { value, .. }) = body.remove(next) else {
            crate::ice!("the statement removed here was matched as an assignment a line above")
        };
        *body.get_mut(i).or_ice("the declaration at this index was matched a line above") =
            Stmt::Var { kind: VarKind::Const, name, init: Some(*value) };
        changed = true;
        i = next;
    }
    changed
}

/// Renames a slot: both the reads of it and the assignments to it.
///
/// `subst_expr` deliberately leaves assignment targets alone, because it
/// substitutes *values*. This renames the storage itself.
fn rename_slot(body: Vec<Stmt>, from: &str, to: &str) -> Vec<Stmt> {
    let mut map = HashMap::default();
    map.insert(from.to_string(), Expr::ident(to.to_string()));
    body.into_iter()
        .map(|s| retarget_stmt(subst_stmt(s, &map), from, to))
        .collect()
}

fn retarget_stmt(s: Stmt, from: &str, to: &str) -> Stmt {
    match s {
        // The declaration is the slot, so it is renamed too. Where the caller
        // has already removed it this matches nothing.
        Stmt::Var { kind, name, init } if name == from => {
            Stmt::Var { kind, name: to.to_string(), init }
        }
        Stmt::Expr(Expr::Assign { target, value }) => {
            let target = match *target {
                Expr::Ident(n) if n == from => Expr::ident(to.to_string()),
                other => other,
            };
            Stmt::Expr(Expr::Assign { target: Box::new(target), value })
        }
        Stmt::If { cond, then, else_ } => Stmt::If {
            cond,
            then: then.into_iter().map(|s| retarget_stmt(s, from, to)).collect(),
            else_: else_.into_iter().map(|s| retarget_stmt(s, from, to)).collect(),
        },
        Stmt::While { cond, body } => Stmt::While {
            cond,
            body: body.into_iter().map(|s| retarget_stmt(s, from, to)).collect(),
        },
        Stmt::Switch { disc, cases } => Stmt::Switch {
            disc,
            cases: cases
                .into_iter()
                .map(|(t, b)| {
                    (t, b.into_iter().map(|s| retarget_stmt(s, from, to)).collect())
                })
                .collect(),
        },
        Stmt::Block(b) => {
            Stmt::Block(b.into_iter().map(|s| retarget_stmt(s, from, to)).collect())
        }
        other => other,
    }
}

/// Collapses `let a; <branches assigning a>; b = a;` into branches that assign
/// `b` directly.
///
/// A value-position `match` inside another one produces exactly this: the
/// inner match fills its own slot, and the outer one then copies it across.
/// Neither slot is anything but a temporary, so there is no reason for two.
fn coalesce_copies(body: &mut Vec<Stmt>, facts: &LocalFacts) -> bool {
    for s in body.iter_mut() {
        let changed = match s {
            Stmt::If { then, else_, .. } => {
                coalesce_copies(then, facts) | coalesce_copies(else_, facts)
            }
            Stmt::While { body, .. } => coalesce_copies(body, facts),
            Stmt::Block(b) => coalesce_copies(b, facts),
            Stmt::Func { body, .. } => coalesce_copies(body, facts),
            Stmt::Switch { cases, .. } => {
                cases.iter_mut().fold(false, |a, (_, b)| a | coalesce_copies(b, facts))
            }
            _ => false,
        };
        if changed {
            return true;
        }
    }

    for i in 0..body.len() {
        let Some(Stmt::Var { kind: VarKind::Let, name: from, init: None }) = body.get(i) else {
            continue;
        };
        // Read exactly once, and written at least once, or there is nothing to
        // move.
        if facts.uses.get(from).copied() != Some(1)
            || facts.assigns.get(from).copied().unwrap_or(0) == 0
        {
            continue;
        }
        // The one read has to be the copy, and it has to sit in this same list
        // so that everything assigning `from` is between the two. The copy is
        // either into a slot that already exists, or into a fresh binding —
        // `?` produces the second, taking the value a `match` just filled in.
        //
        // The destination is read off the same statement that identifies the
        // copy, so there is no second match to keep in step with this one.
        let found = body.iter().enumerate().skip(i + 1).find_map(|(j, s)| match s {
            Stmt::Expr(Expr::Assign { target, value }) => match (&**target, &**value) {
                (Expr::Ident(t), Expr::Ident(v)) if v == from => Some((j, t.clone(), false)),
                _ => None,
            },
            Stmt::Var { name, init: Some(Expr::Ident(v)), .. }
                if v == from && name.starts_with('$') =>
            {
                Some((j, name.clone(), true))
            }
            _ => None,
        });
        let Some((j, to, declares)) = found else {
            continue;
        };
        // Only a temporary the backend made, never a name from the source.
        if !to.starts_with('$') {
            continue;
        }
        // The writes to `from` move up to where they are, so nothing between
        // the declaration and the copy may read or write the destination. What
        // happens to it outside that stretch is unaffected: every write being
        // moved still precedes the point the copy stood at.
        let mut region = LocalFacts::default();
        for s in body.get(i..j).unwrap_or_default() {
            count_stmt(s, &mut region);
        }
        if region.uses.contains_key(&to) || region.is_assigned(&to) {
            continue;
        }
        // A copy that declares its destination must be the only declaration of
        // it, since the renamed `let` takes that role.
        if declares && facts.declared.get(&to).copied() != Some(1) {
            continue;
        }
        let from = from.clone();

        let mut rest: Vec<Stmt> = body.drain(i..).collect();
        rest.remove(j - i); // the copy
        if !declares {
            // The destination already has a declaration of its own, so this
            // one goes; otherwise it is kept and renamed into that role.
            rest.remove(0);
        }
        body.extend(rename_slot(rest, &from, &to));
        return true;
    }
    false
}

/// One pass of local cleanup over a function body. Returns `true` when
/// anything changed, so the caller can run it again.
fn clean_body(body: &mut Vec<Stmt>) -> bool {
    let mut facts = LocalFacts::default();
    body.iter().for_each(|s| count_stmt(s, &mut facts));
    if facts.opaque {
        return false;
    }
    if merge_declarations(body, &facts) {
        return true;
    }
    if coalesce_copies(body, &facts) {
        return true;
    }
    if read_through_locals(body, &facts) {
        return true;
    }

    let mut map: HashMap<String, Expr> = HashMap::default();
    let mut dead: HashSet<String> = HashSet::default();
    collect_cleanup(body, &facts, &mut map, &mut dead);
    if map.is_empty() && dead.is_empty() {
        return false;
    }
    // A value may name a binding this same round removes, so the map is
    // substituted into itself before it is applied to the body.
    //
    // Resolution can turn a one-use substitution into a many-use one: `x` is
    // read once, but only to initialise an alias that is itself read five
    // times, and following the chain would paste `x`'s value at all five. That
    // is legal — the value is pure — but it is five allocations where there
    // was one. Only something free to copy may end up read more than once.
    //
    // Withdrawing such an entry has to happen *before* resolution rather than
    // after it, because resolution also pastes its value inside other entries:
    // dropping `t -> <expr>` while some `v -> t[1]` had already resolved to
    // `<expr>[1]` keeps the copy it was meant to prevent. So the two run
    // together until no entry overshoots — each pass withdraws at least one,
    // so it terminates.
    loop {
        let mut resolved = map.clone();
        resolve_map(&mut resolved, &facts.uses);
        let overshot: Vec<String> = resolved
            .iter()
            .filter(|(name, value)| {
                !value.is_pure_literal()
                    && facts.uses.get(*name).copied().unwrap_or(0) > 1
            })
            .map(|(name, _)| name.clone())
            .collect();
        if overshot.is_empty() {
            map = resolved;
            break;
        }
        for name in overshot {
            map.remove(&name);
            dead.remove(&name);
        }
    }
    if map.is_empty() && dead.is_empty() {
        return false;
    }

    let taken = std::mem::take(body);
    let next: Vec<Stmt> = taken.into_iter().map(|s| subst_stmt(s, &map)).collect();
    *body = drop_bindings(next, &dead);
    true
}

// -- reading a local aggregate through to its uses -----------------------------
//
// A functional update leaves the aggregate it was built from behind:
//
//     const $t1=[n_1,2,'first'];
//     const m_3=[$t1[0],$t1[1],'second'];
//
// `$t1` exists only to be taken apart again, so where every use of it is a
// constant-index read the elements can go straight to those uses and the array
// never has to be built. That is one allocation per functional update, in a
// language where a functional update is the only way to change a field.
//
// Three conditions, and each of them is load-bearing:
//
//  * every use is `t[k]` with a literal `k` in range — a use of `t` itself
//    would need the array to exist;
//  * an element that is not a literal is extracted at most once, or one
//    allocation would become several — the same trap `clean_body`'s `overshot`
//    loop guards against;
//  * every read sits at the binding's own depth, so nothing moves inside a
//    loop or a closure. This is what keeps a *context* — an array the runtime
//    writes through — from being copied per call: a context read once, at its
//    own depth, is still exactly one array.

/// A table, printed, so two of them can be compared for a fixed point. The
/// order has to be stable or `builds_are_reproducible` would depend on how a
/// hash map happened to iterate.
fn print_table(table: &HashMap<String, Vec<Expr>>) -> String {
    let mut names: Vec<&String> = table.keys().collect();
    names.sort();
    let mut out = String::new();
    for n in names {
        out.push_str(n);
        out.push('=');
        out.push_str(&print(
            &[Stmt::Expr(Expr::Array(table[n].clone()))],
            false,
        ));
        out.push(';');
    }
    out
}

/// What is known about a candidate aggregate after walking the body.
///
/// Escaping *settles* the matter, so the reads counted before it are no longer
/// worth anything. As a `bool` beside the map, "escaped, and here are its
/// reads" was representable and every consumer had to test both.
#[derive(Default)]
enum AggregateUse {
    /// How many times each slot is read, by slot.
    #[default]
    Unused,
    Reads(HashMap<usize, usize>),
    /// A use that is not a constant-index read.
    Escaped,
}

impl AggregateUse {
    /// The per-slot read counts, for an aggregate that has not escaped.
    fn reads(&self) -> Option<&HashMap<usize, usize>> {
        match self {
            AggregateUse::Reads(r) => Some(r),
            _ => None,
        }
    }

    fn record_read(&mut self, slot: usize) {
        match self {
            AggregateUse::Escaped => {}
            AggregateUse::Unused => {
                *self = AggregateUse::Reads([(slot, 1)].into_iter().collect());
            }
            AggregateUse::Reads(r) => *r.entry(slot).or_insert(0) += 1,
        }
    }

    /// Escaping is final: once the array itself is named, no later read makes
    /// it shareable again.
    fn escape(&mut self) {
        *self = AggregateUse::Escaped;
    }
}

fn read_through_locals(body: &mut Vec<Stmt>, facts: &LocalFacts) -> bool {
    let mut cands: HashMap<String, Vec<Expr>> = HashMap::default();
    collect_aggregates(body, facts, &mut cands);
    if cands.is_empty() {
        return false;
    }

    let mut uses: HashMap<String, AggregateUse> = HashMap::default();
    for name in cands.keys() {
        uses.insert(name.clone(), AggregateUse::default());
    }
    survey_aggregates(body, &mut uses);

    let mut table: HashMap<String, Vec<Expr>> = cands
        .into_iter()
        .filter(|(name, items)| {
            let Some(reads) = uses.get(name).and_then(|u| u.reads()) else { return false };
            if reads.is_empty() {
                return false;
            }
            // Every read accounted for: `uses` counts bare reads of the name,
            // and each of ours is one, so a mismatch means something else read
            // it in a shape the survey did not recognise.
            if facts.uses.get(name).copied().unwrap_or(0) != reads.values().sum::<usize>() {
                return false;
            }
            reads
                .iter()
                .all(|(k, n)| items.get(*k).is_some_and(|e| e.is_pure_literal() || *n == 1))
        })
        .collect();
    if table.is_empty() {
        return false;
    }

    // A functional update reads one aggregate to build the next, so one
    // candidate's elements routinely read another — and the declarations all
    // go together. The values are resolved against each other first, or a
    // pasted element would name a binding that is about to disappear. A `const`
    // can only read what came before it, so this cannot cycle.
    for _ in 0..table.len() {
        let snapshot = table.clone();
        let next: HashMap<String, Vec<Expr>> = snapshot
            .iter()
            .map(|(name, items)| {
                let items = items
                    .iter()
                    .map(|e| extract_expr(e.clone(), &snapshot))
                    .collect::<Vec<_>>();
                (name.clone(), items)
            })
            .collect();
        if print_table(&next) == print_table(&table) {
            break;
        }
        table = next;
    }

    let taken = std::mem::take(body);
    *body = taken.into_iter().map(|s| extract_stmt(s, &table)).collect();
    let names: HashSet<String> = table.into_keys().collect();
    *body = drop_bindings(std::mem::take(body), &names);
    true
}

/// The `const t = [<pure>]` bindings that could be read through.
fn collect_aggregates(
    body: &[Stmt],
    facts: &LocalFacts,
    out: &mut HashMap<String, Vec<Expr>>,
) {
    for s in body {
        if let Stmt::Var { kind: VarKind::Const, name, init: Some(Expr::Array(items)) } = s {
            let single = facts.declared.get(name).copied() == Some(1)
                && !facts.is_assigned(name)
                && facts.read_where_bound(name);
            // Everything the array holds has to be free to move to a use, and
            // everything *not* moved has to be free to drop.
            if single && items.iter().all(|e| e.is_pure()) && !items.is_empty() {
                out.insert(name.clone(), items.clone());
            }
        }
        match s {
            Stmt::If { then, else_, .. } => {
                collect_aggregates(then, facts, out);
                collect_aggregates(else_, facts, out);
            }
            Stmt::While { body, .. } => collect_aggregates(body, facts, out),
            Stmt::Switch { cases, .. } => {
                for (_, b) in cases {
                    collect_aggregates(b, facts, out);
                }
            }
            Stmt::Block(b) | Stmt::Func { body: b, .. } => collect_aggregates(b, facts, out),
            _ => {}
        }
    }
}

fn survey_aggregates(body: &[Stmt], out: &mut HashMap<String, AggregateUse>) {
    for s in body {
        stmt_exprs(s, &mut |e| survey_aggregate_expr(e, out));
        match s {
            Stmt::If { then, else_, .. } => {
                survey_aggregates(then, out);
                survey_aggregates(else_, out);
            }
            Stmt::While { body, .. } => survey_aggregates(body, out),
            Stmt::Switch { cases, .. } => {
                for (_, b) in cases {
                    survey_aggregates(b, out);
                }
            }
            Stmt::Block(b) | Stmt::Func { body: b, .. } => survey_aggregates(b, out),
            _ => {}
        }
    }
}

fn survey_aggregate_expr(e: &Expr, out: &mut HashMap<String, AggregateUse>) {
    // `t[k]`, the only shape that does not need the array itself.
    if let Expr::Index { obj, index } = e {
        if let (Expr::Ident(name), Expr::Num(i)) = (&**obj, &**index) {
            if let Some(u) = out.get_mut(name) {
                if *i >= 0.0 && i.fract() == 0.0 {
                    u.record_read(*i as usize);
                } else {
                    u.escape();
                }
                return;
            }
        }
    }
    if let Expr::Ident(name) = e {
        if let Some(u) = out.get_mut(name) {
            u.escape();
        }
        return;
    }
    let mut go = |x: &Expr| survey_aggregate_expr(x, out);
    match e {
        Expr::Array(xs) | Expr::Seq(xs) => xs.iter().for_each(go),
        Expr::Object(fs) => fs.iter().for_each(|(_, v)| go(v)),
        Expr::Member { obj, .. } => go(obj),
        Expr::Index { obj, index } => {
            survey_aggregate_expr(obj, out);
            survey_aggregate_expr(index, out);
        }
        Expr::Call { callee, args } | Expr::New { callee, args } => {
            survey_aggregate_expr(callee, out);
            args.iter().for_each(|a| survey_aggregate_expr(a, out));
        }
        Expr::Unary { operand, .. } => go(operand),
        Expr::Binary { lhs, rhs, .. } => {
            survey_aggregate_expr(lhs, out);
            survey_aggregate_expr(rhs, out);
        }
        Expr::Cond { test, cons, alt } => {
            survey_aggregate_expr(test, out);
            survey_aggregate_expr(cons, out);
            survey_aggregate_expr(alt, out);
        }
        Expr::Assign { target, value } => {
            survey_aggregate_expr(target, out);
            survey_aggregate_expr(value, out);
        }
        Expr::Arrow { body, .. } => go(body),
        Expr::ArrowBlock { body, .. } => survey_aggregates(body, out),
        Expr::Spread(x) => go(x),
        _ => {}
    }
}

fn extract_stmt(s: Stmt, table: &HashMap<String, Vec<Expr>>) -> Stmt {
    match s {
        // The declarations themselves are left alone; `drop_bindings` removes
        // them, and rewriting an aggregate into itself is not the point.
        Stmt::Var { kind: VarKind::Const, ref name, .. } if table.contains_key(name) => s,
        Stmt::Var { kind, name, init } => {
            Stmt::Var { kind, name, init: init.map(|e| extract_expr(e, table)) }
        }
        Stmt::Func { name, params, body } => Stmt::Func {
            name,
            params,
            body: body.into_iter().map(|s| extract_stmt(s, table)).collect(),
        },
        Stmt::Return(e) => Stmt::Return(e.map(|e| extract_expr(e, table))),
        Stmt::If { cond, then, else_ } => Stmt::If {
            cond: extract_expr(cond, table),
            then: then.into_iter().map(|s| extract_stmt(s, table)).collect(),
            else_: else_.into_iter().map(|s| extract_stmt(s, table)).collect(),
        },
        Stmt::While { cond, body } => Stmt::While {
            cond: extract_expr(cond, table),
            body: body.into_iter().map(|s| extract_stmt(s, table)).collect(),
        },
        Stmt::Switch { disc, cases } => Stmt::Switch {
            disc: extract_expr(disc, table),
            cases: cases
                .into_iter()
                .map(|(t, b)| {
                    (
                        t.map(|t| extract_expr(t, table)),
                        b.into_iter().map(|s| extract_stmt(s, table)).collect(),
                    )
                })
                .collect(),
        },
        Stmt::Expr(e) => Stmt::Expr(extract_expr(e, table)),
        Stmt::Throw(e) => Stmt::Throw(extract_expr(e, table)),
        Stmt::ExportDefault(e) => Stmt::ExportDefault(extract_expr(e, table)),
        Stmt::Block(b) => Stmt::Block(b.into_iter().map(|s| extract_stmt(s, table)).collect()),
        other => other,
    }
}

fn extract_expr(e: Expr, table: &HashMap<String, Vec<Expr>>) -> Expr {
    if let Expr::Index { obj, index } = &e {
        if let (Expr::Ident(name), Expr::Num(i)) = (&**obj, &**index) {
            if let Some(items) = table.get(name) {
                if let Some(item) = items.get(*i as usize) {
                    return item.clone();
                }
            }
        }
    }
    match e {
        Expr::Array(xs) => Expr::Array(xs.into_iter().map(|x| extract_expr(x, table)).collect()),
        Expr::Seq(xs) => Expr::Seq(xs.into_iter().map(|x| extract_expr(x, table)).collect()),
        Expr::Object(fs) => {
            Expr::Object(fs.into_iter().map(|(k, v)| (k, extract_expr(v, table))).collect())
        }
        Expr::Member { obj, prop } => {
            Expr::Member { obj: Box::new(extract_expr(*obj, table)), prop }
        }
        Expr::Index { obj, index } => Expr::Index {
            obj: Box::new(extract_expr(*obj, table)),
            index: Box::new(extract_expr(*index, table)),
        },
        Expr::Call { callee, args } => Expr::Call {
            callee: Box::new(extract_expr(*callee, table)),
            args: args.into_iter().map(|a| extract_expr(a, table)).collect(),
        },
        Expr::New { callee, args } => Expr::New {
            callee: Box::new(extract_expr(*callee, table)),
            args: args.into_iter().map(|a| extract_expr(a, table)).collect(),
        },
        Expr::Unary { op, operand } => Expr::un(op, extract_expr(*operand, table)),
        Expr::Binary { op, lhs, rhs } => {
            Expr::bin(op, extract_expr(*lhs, table), extract_expr(*rhs, table))
        }
        Expr::Cond { test, cons, alt } => Expr::cond(
            extract_expr(*test, table),
            extract_expr(*cons, table),
            extract_expr(*alt, table),
        ),
        Expr::Assign { target, value } => {
            Expr::Assign { target, value: Box::new(extract_expr(*value, table)) }
        }
        Expr::Arrow { params, body } => {
            Expr::Arrow { params, body: Box::new(extract_expr(*body, table)) }
        }
        Expr::ArrowBlock { params, body } => Expr::ArrowBlock {
            params,
            body: body.into_iter().map(|s| extract_stmt(s, table)).collect(),
        },
        Expr::Spread(x) => Expr::Spread(Box::new(extract_expr(*x, table))),
        other => other,
    }
}

/// How many times the local passes may run over one body.
///
/// The bound matters for more than termination: `builds_are_reproducible`
/// compares bytes, so the number of rounds has to be a property of the input
/// rather than of anything that varies between runs.
const CLEANUP_ROUNDS: usize = 4;

/// Runs folding and local cleanup over every function body, to a fixed point.
fn clean_locals(stmts: Vec<Stmt>, table: &HashMap<String, Vec<Expr>>) -> Vec<Stmt> {
    stmts
        .into_iter()
        .map(|s| match s {
            Stmt::Func { name, params, mut body } => {
                for _ in 0..CLEANUP_ROUNDS {
                    if !table.is_empty() {
                        body =
                            body.into_iter().map(|s| read_through_stmt(s, table)).collect();
                    }
                    body = fold_block(body);
                    if !clean_body(&mut body) {
                        break;
                    }
                }
                // Last, so the chains it looks for have already been folded
                // and their discriminants have already settled.
                Stmt::Func { name, params, body: switches(body) }
            }
            other => other,
        })
        .collect()
}

// -- merging functions that came out the same ---------------------------------

/// Points every reference to a function at the first function with that body.
///
/// Monomorphization produces one instance per `(function, type arguments)`
/// pair, and the code for two instances is frequently the same: nothing about
/// `fold` over a list of integers differs from `fold` over a list of strings
/// once the types are gone. In one conformance artifact 123 of 546 functions
/// had a byte-identical twin.
///
/// Nothing is removed here — the references move, and `eliminate_dead` drops
/// whatever that leaves unreachable. A name is only ever the *target* of a
/// merge if it is safe to keep, and only ever the *source* if it is safe to
/// lose: a root or a name mentioned in verbatim source (the entry epilogue
/// names its function as text) is neither dropped nor rewritten.
fn merge_identical(stmts: Vec<Stmt>, roots: &[String]) -> Vec<Stmt> {
    let mut pinned: HashSet<String> = roots.iter().cloned().collect();
    for s in &stmts {
        match s {
            Stmt::Raw(src) => collect_idents_raw(src, &mut pinned),
            Stmt::RawDecl { src, .. } => collect_idents_raw(src, &mut pinned),
            _ => {}
        }
    }

    // First by emission order wins, so the result does not depend on hash
    // order — build output is compared byte for byte.
    let mut first: HashMap<String, String> = HashMap::default();
    let mut alias: HashMap<String, Expr> = HashMap::default();
    for s in &stmts {
        let Stmt::Func { name, params, body } = s else { continue };
        if pinned.contains(name) {
            continue;
        }
        let key = print(
            &[Stmt::Func { name: String::new(), params: params.clone(), body: body.clone() }],
            false,
        );
        match first.get(&key) {
            Some(winner) => {
                alias.insert(name.clone(), Expr::ident(winner.clone()));
            }
            None => {
                first.insert(key, name.clone());
            }
        }
    }
    if alias.is_empty() {
        return stmts;
    }
    stmts.into_iter().map(|s| subst_stmt(s, &alias)).collect()
}

// -- dead code elimination ---------------------------------------------------

/// Drops top-level declarations nothing reachable from `roots` names. Since
/// the backend emits one top-level function per reachable instance, this is
/// what removes the parts of `core/*` a program does not use.
fn eliminate_dead(stmts: Vec<Stmt>, roots: &[String]) -> Vec<Stmt> {
    let mut deps: HashMap<String, HashSet<String>> = HashMap::default();
    let mut declared: HashMap<String, usize> = HashMap::default();
    for (i, s) in stmts.iter().enumerate() {
        match s {
            Stmt::Func { name, body, params } => {
                let mut used = HashSet::default();
                for st in body {
                    collect_idents_stmt(st, &mut used);
                }
                for p in params {
                    used.remove(p);
                }
                declared.insert(name.clone(), i);
                deps.insert(name.clone(), used);
            }
            Stmt::Var { name, init, .. } => {
                let mut used = HashSet::default();
                if let Some(e) = init {
                    collect_idents(e, &mut used);
                }
                declared.insert(name.clone(), i);
                deps.insert(name.clone(), used);
            }
            Stmt::RawDecl { name, src } => {
                let mut used = HashSet::default();
                collect_idents_raw(src, &mut used);
                used.remove(name);
                declared.insert(name.clone(), i);
                deps.insert(name.clone(), used);
            }
            _ => {}
        }
    }

    let mut live: HashSet<String> = HashSet::default();
    let mut stack: Vec<String> = roots.to_vec();
    // Anything a non-declaration statement mentions is a root too: those run
    // for their effect and are never dropped.
    for s in &stmts {
        if !is_declaration(s) {
            let mut used = HashSet::default();
            collect_idents_stmt(s, &mut used);
            stack.extend(used);
        }
    }
    while let Some(name) = stack.pop() {
        if !live.insert(name.clone()) {
            continue;
        }
        if let Some(used) = deps.get(&name) {
            stack.extend(used.iter().cloned());
        }
    }

    stmts
        .into_iter()
        .enumerate()
        .filter(|(i, s)| match s {
            Stmt::Func { name, .. }
            | Stmt::Var { name, .. }
            | Stmt::RawDecl { name, .. } => {
                live.contains(name) && declared.get(name) == Some(i)
            }
            _ => true,
        })
        .map(|(_, s)| s)
        .collect()
}

fn collect_idents_stmt(s: &Stmt, out: &mut HashSet<String>) {
    match s {
        Stmt::Var { init, .. } => {
            if let Some(e) = init {
                collect_idents(e, out);
            }
        }
        Stmt::Func { body, .. } => body.iter().for_each(|x| collect_idents_stmt(x, out)),
        Stmt::Return(e) => {
            if let Some(e) = e {
                collect_idents(e, out);
            }
        }
        Stmt::If { cond, then, else_ } => {
            collect_idents(cond, out);
            then.iter().for_each(|x| collect_idents_stmt(x, out));
            else_.iter().for_each(|x| collect_idents_stmt(x, out));
        }
        Stmt::While { cond, body } => {
            collect_idents(cond, out);
            body.iter().for_each(|x| collect_idents_stmt(x, out));
        }
        Stmt::Switch { disc, cases } => {
            collect_idents(disc, out);
            for (t, b) in cases {
                if let Some(t) = t {
                    collect_idents(t, out);
                }
                b.iter().for_each(|x| collect_idents_stmt(x, out));
            }
        }
        Stmt::Expr(e) | Stmt::Throw(e) | Stmt::ExportDefault(e) => collect_idents(e, out),
        Stmt::Block(b) => b.iter().for_each(|x| collect_idents_stmt(x, out)),
        Stmt::Raw(src) => collect_idents_raw(src, out),
        Stmt::RawDecl { src, .. } => collect_idents_raw(src, out),
        Stmt::Break | Stmt::Continue => {}
    }
}

/// The runtime arrives as `Raw`, so its cross-references are found by scanning
/// for identifier-shaped runs rather than by parsing.
fn collect_idents_raw(src: &str, out: &mut HashSet<String>) {
    let bytes = src.as_bytes();
    let mut i = 0;
    while let Some(c) = bytes.get(i) {
        if c.is_ascii_alphabetic() || *c == b'_' || *c == b'$' {
            let start = i;
            while bytes
                .get(i)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'$')
            {
                i += 1;
            }
            // A run of identifier bytes is ASCII throughout, so both ends are
            // character boundaries however the rest of the source is encoded.
            if let Some(name) = src.get(start..i) {
                out.insert(name.to_string());
            }
        } else {
            i += 1;
        }
    }
}

fn collect_idents(e: &Expr, out: &mut HashSet<String>) {
    match e {
        Expr::Ident(name) => {
            out.insert(name.clone());
        }
        Expr::Array(xs) | Expr::Seq(xs) => xs.iter().for_each(|x| collect_idents(x, out)),
        Expr::Object(fs) => fs.iter().for_each(|(_, v)| collect_idents(v, out)),
        Expr::Member { obj, .. } => collect_idents(obj, out),
        Expr::Index { obj, index } => {
            collect_idents(obj, out);
            collect_idents(index, out);
        }
        Expr::Call { callee, args } | Expr::New { callee, args } => {
            collect_idents(callee, out);
            args.iter().for_each(|a| collect_idents(a, out));
        }
        Expr::Unary { operand, .. } => collect_idents(operand, out),
        Expr::Binary { lhs, rhs, .. } | Expr::Assign { target: lhs, value: rhs } => {
            collect_idents(lhs, out);
            collect_idents(rhs, out);
        }
        Expr::Cond { test, cons, alt } => {
            collect_idents(test, out);
            collect_idents(cons, out);
            collect_idents(alt, out);
        }
        Expr::Arrow { body, .. } => collect_idents(body, out),
        Expr::ArrowBlock { body, .. } => body.iter().for_each(|s| collect_idents_stmt(s, out)),
        Expr::Spread(x) => collect_idents(x, out),
        _ => {}
    }
}

// -- mangling ----------------------------------------------------------------

/// Short names, in the order a base-54 counter produces them.
#[expect(
    clippy::indexing_slicing,
    reason = "each index is a remainder by the length of the very table being indexed"
)]
pub fn short_name(mut n: usize) -> String {
    const FIRST: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ_$";
    const REST: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ_$0123456789";
    let mut out = String::new();
    out.push(FIRST[n % FIRST.len()] as char);
    n /= FIRST.len();
    while n > 0 {
        n -= 1;
        out.push(REST[n % REST.len()] as char);
        n /= REST.len();
    }
    out
}

const RESERVED: &[&str] = &[
    "do", "if", "in", "for", "let", "new", "try", "var", "case", "else", "enum", "eval", "null",
    "this", "true", "void", "with", "await", "break", "catch", "class", "const", "false",
    "super", "throw", "while", "yield", "delete", "export", "import", "return", "switch",
    "typeof", "default", "extends", "finally", "continue", "function", "instanceof", "of",
];

/// Renames every declaration the program owns. Top-level names are mangled
/// globally; a function's parameters and locals are mangled per scope, so
/// short names are reused freely between functions.
fn mangle_program(stmts: Vec<Stmt>, roots: &[String]) -> Vec<Stmt> {
    // Names the runtime reaches by string, plus anything the host needs to
    // see, must not be renamed.
    let keep: HashSet<String> = roots.iter().cloned().collect();

    let mut globals: Vec<String> = Vec::new();
    for s in &stmts {
        match s {
            Stmt::Func { name, .. } | Stmt::Var { name, .. } => globals.push(name.clone()),
            _ => {}
        }
    }
    // Anything verbatim source mentions is out of reach of the renamer: the
    // runtime is text, not a tree, so its own names stay as they are.
    let mut untouchable: HashSet<String> = keep;
    for s in &stmts {
        match s {
            Stmt::Raw(src) => collect_idents_raw(src, &mut untouchable),
            Stmt::RawDecl { name, src } => {
                untouchable.insert(name.clone());
                collect_idents_raw(src, &mut untouchable);
            }
            _ => {}
        }
    }

    let mut map: HashMap<String, String> = HashMap::default();
    let mut counter = 0usize;
    for g in &globals {
        if untouchable.contains(g) {
            continue;
        }
        loop {
            let candidate = short_name(counter);
            counter += 1;
            if RESERVED.contains(&candidate.as_str()) || untouchable.contains(&candidate) {
                continue;
            }
            map.insert(g.clone(), candidate);
            break;
        }
    }

    // The counter above hands each global a name of its own, so this is a
    // bijection, and it answers "which global holds this short name" in
    // constant time — the question every local's search asks, once per
    // candidate it rejects.
    let by_short: HashMap<String, String> =
        map.iter().map(|(k, v)| (v.clone(), k.clone())).collect();
    let scope = Scope::global(&map, &by_short);
    let mut pool = Pool::default();
    stmts.into_iter().map(|s| rename_stmt(s, &scope, &mut pool)).collect()
}

/// The names in scope while one body is renamed.
///
/// Layered rather than one map per scope. The whole program's globals used to
/// be copied into every function, and again into every block, loop, `switch`
/// case and arrow inside it — one copy per scope of a map with an entry per
/// declaration in the program — and the search for a free local name then
/// scanned that copy once per candidate it rejected. Locals are the only part
/// that differs between scopes, so they are the only part that is copied.
struct Scope<'a> {
    globals: &'a HashMap<String, String>,
    /// Which global each global short name stands for.
    by_short: &'a HashMap<String, String>,
    locals: HashMap<String, String>,
    /// The short names this scope's locals hold, and how many hold each — a
    /// count rather than a set because rebinding a name releases the one it
    /// held.
    local_names: HashMap<String, usize>,
    /// Set once a local has taken a global's *original* name, which releases
    /// that global's short name for the rest of this scope. It is what makes
    /// `Pool`'s answers — which know nothing of locals — no longer apply.
    shadows_global: bool,
}

impl<'a> Scope<'a> {
    fn global(globals: &'a HashMap<String, String>, by_short: &'a HashMap<String, String>) -> Scope<'a> {
        Scope {
            globals,
            by_short,
            locals: HashMap::default(),
            local_names: HashMap::default(),
            shadows_global: false,
        }
    }

    /// A nested scope: everything this one binds, and whatever it binds itself
    /// stays inside it.
    fn nested(&self) -> Scope<'a> {
        Scope {
            globals: self.globals,
            by_short: self.by_short,
            locals: self.locals.clone(),
            local_names: self.local_names.clone(),
            shadows_global: self.shadows_global,
        }
    }

    fn get(&self, name: &str) -> Option<&String> {
        self.locals.get(name).or_else(|| self.globals.get(name))
    }

    /// Whether a short name is one this scope already stands for — a local's,
    /// or a global's that no local has shadowed.
    fn holds(&self, short: &str) -> bool {
        if self.local_names.contains_key(short) {
            return true;
        }
        self.by_short.get(short).is_some_and(|g| !self.locals.contains_key(g))
    }

    fn bind(&mut self, original: &str, short: String) {
        if self.globals.contains_key(original) {
            self.shadows_global = true;
        }
        *self.local_names.entry(short.clone()).or_insert(0) += 1;
        if let Some(old) = self.locals.insert(original.to_string(), short) {
            if let Some(n) = self.local_names.get_mut(&old) {
                *n -= 1;
                if *n == 0 {
                    self.local_names.remove(&old);
                }
            }
        }
    }
}

/// Where a scope's search for a free local name has reached: an index into
/// [`Pool`], and the raw counter that index stands for.
#[derive(Default, Clone)]
struct Counter {
    index: usize,
    raw: usize,
}

/// The candidate names, in counter order, that no global holds.
///
/// Every scope numbers its locals from zero, so every one of them walks the
/// same prefix of the counter before it reaches a name the globals have left
/// free. Walking it once and keeping the answers is what turns that prefix
/// from a scan of the whole program, per local, per scope, into an index.
#[derive(Default)]
struct Pool {
    /// Each free candidate, with the counter value that follows it.
    names: Vec<(String, usize)>,
    /// Where the walk that built `names` stopped.
    next: usize,
}

impl Pool {
    fn at(&mut self, i: usize, by_short: &HashMap<String, String>) -> Option<(String, usize)> {
        while self.names.len() <= i {
            let candidate = short_name(self.next);
            self.next += 1;
            if RESERVED.contains(&candidate.as_str()) || by_short.contains_key(&candidate) {
                continue;
            }
            self.names.push((candidate, self.next));
        }
        self.names.get(i).cloned()
    }
}

fn rename_stmt(s: Stmt, scope: &Scope, pool: &mut Pool) -> Stmt {
    match s {
        Stmt::Var { kind, name, init } => Stmt::Var {
            kind,
            name: scope.get(&name).cloned().unwrap_or(name),
            init: init.map(|e| rename(e, scope, pool)),
        },
        Stmt::Func { name, params, body } => {
            // Parameters and locals get their own short names, reused across
            // functions because each is a fresh scope.
            let mut local = scope.nested();
            let mut counter = Counter::default();
            let params: Vec<String> = params
                .into_iter()
                .map(|p| fresh_local(&p, &mut local, &mut counter, pool))
                .collect();
            let body = rename_scope(body, &mut local, &mut counter, pool);
            Stmt::Func { name: scope.get(&name).cloned().unwrap_or(name), params, body }
        }
        Stmt::Return(e) => Stmt::Return(e.map(|x| rename(x, scope, pool))),
        Stmt::If { cond, then, else_ } => Stmt::If {
            cond: rename(cond, scope, pool),
            then: then.into_iter().map(|x| rename_stmt(x, scope, pool)).collect(),
            else_: else_.into_iter().map(|x| rename_stmt(x, scope, pool)).collect(),
        },
        Stmt::While { cond, body } => Stmt::While {
            cond: rename(cond, scope, pool),
            body: body.into_iter().map(|x| rename_stmt(x, scope, pool)).collect(),
        },
        Stmt::Switch { disc, cases } => Stmt::Switch {
            disc: rename(disc, scope, pool),
            cases: cases
                .into_iter()
                .map(|(t, b)| {
                    (
                        t.map(|x| rename(x, scope, pool)),
                        b.into_iter().map(|x| rename_stmt(x, scope, pool)).collect(),
                    )
                })
                .collect(),
        },
        Stmt::Expr(e) => Stmt::Expr(rename(e, scope, pool)),
        Stmt::Throw(e) => Stmt::Throw(rename(e, scope, pool)),
        Stmt::Block(b) => {
            Stmt::Block(b.into_iter().map(|x| rename_stmt(x, scope, pool)).collect())
        }
        Stmt::ExportDefault(e) => Stmt::ExportDefault(rename(e, scope, pool)),
        other => other,
    }
}

fn fresh_local(
    original: &str,
    scope: &mut Scope,
    counter: &mut Counter,
    pool: &mut Pool,
) -> String {
    let candidate = loop {
        // A local may not shadow a global the body still needs, which is what
        // the pool has already settled — unless a local has taken a global's
        // name, in which case the search runs against the scope itself.
        if !scope.shadows_global {
            if let Some((candidate, after)) = pool.at(counter.index, scope.by_short) {
                counter.index += 1;
                counter.raw = after;
                if scope.local_names.contains_key(&candidate) {
                    continue;
                }
                break candidate;
            }
        }
        let candidate = short_name(counter.raw);
        counter.raw += 1;
        counter.index += 1;
        if RESERVED.contains(&candidate.as_str()) || scope.holds(&candidate) {
            continue;
        }
        break candidate;
    };
    scope.bind(original, candidate.clone());
    candidate
}

fn rename_scope(
    body: Vec<Stmt>,
    scope: &mut Scope,
    counter: &mut Counter,
    pool: &mut Pool,
) -> Vec<Stmt> {
    let mut out = Vec::new();
    for s in body {
        match s {
            Stmt::Var { kind, name, init } => {
                let init = init.map(|e| rename(e, scope, pool));
                let renamed = fresh_local(&name, scope, counter, pool);
                out.push(Stmt::Var { kind, name: renamed, init });
            }
            Stmt::If { cond, then, else_ } => {
                let cond = rename(cond, scope, pool);
                let then = rename_scope(then, &mut scope.nested(), &mut counter.clone(), pool);
                let else_ =
                    rename_scope(else_, &mut scope.nested(), &mut counter.clone(), pool);
                out.push(Stmt::If { cond, then, else_ });
            }
            Stmt::While { cond, body } => {
                let cond = rename(cond, scope, pool);
                let body = rename_scope(body, &mut scope.nested(), &mut counter.clone(), pool);
                out.push(Stmt::While { cond, body });
            }
            Stmt::Block(b) => {
                out.push(Stmt::Block(rename_scope(
                    b,
                    &mut scope.nested(),
                    &mut counter.clone(),
                    pool,
                )));
            }
            Stmt::Switch { disc, cases } => {
                let disc = rename(disc, scope, pool);
                let cases = cases
                    .into_iter()
                    .map(|(t, b)| {
                        (
                            t.map(|x| rename(x, scope, pool)),
                            rename_scope(b, &mut scope.nested(), &mut counter.clone(), pool),
                        )
                    })
                    .collect();
                out.push(Stmt::Switch { disc, cases });
            }
            other => out.push(rename_stmt(other, scope, pool)),
        }
    }
    out
}

fn rename(e: Expr, scope: &Scope, pool: &mut Pool) -> Expr {
    match e {
        Expr::Ident(name) => Expr::Ident(scope.get(&name).cloned().unwrap_or(name)),
        Expr::Array(xs) => {
            Expr::Array(xs.into_iter().map(|x| rename(x, scope, pool)).collect())
        }
        Expr::Seq(xs) => Expr::Seq(xs.into_iter().map(|x| rename(x, scope, pool)).collect()),
        Expr::Object(fs) => {
            Expr::Object(fs.into_iter().map(|(k, v)| (k, rename(v, scope, pool))).collect())
        }
        Expr::Member { obj, prop } => {
            Expr::Member { obj: Box::new(rename(*obj, scope, pool)), prop }
        }
        Expr::Index { obj, index } => Expr::Index {
            obj: Box::new(rename(*obj, scope, pool)),
            index: Box::new(rename(*index, scope, pool)),
        },
        Expr::Call { callee, args } => Expr::Call {
            callee: Box::new(rename(*callee, scope, pool)),
            args: args.into_iter().map(|a| rename(a, scope, pool)).collect(),
        },
        Expr::New { callee, args } => Expr::New {
            callee: Box::new(rename(*callee, scope, pool)),
            args: args.into_iter().map(|a| rename(a, scope, pool)).collect(),
        },
        Expr::Unary { op, operand } => {
            Expr::Unary { op, operand: Box::new(rename(*operand, scope, pool)) }
        }
        Expr::Binary { op, lhs, rhs } => Expr::Binary {
            op,
            lhs: Box::new(rename(*lhs, scope, pool)),
            rhs: Box::new(rename(*rhs, scope, pool)),
        },
        Expr::Assign { target, value } => Expr::Assign {
            target: Box::new(rename(*target, scope, pool)),
            value: Box::new(rename(*value, scope, pool)),
        },
        Expr::Cond { test, cons, alt } => Expr::Cond {
            test: Box::new(rename(*test, scope, pool)),
            cons: Box::new(rename(*cons, scope, pool)),
            alt: Box::new(rename(*alt, scope, pool)),
        },
        Expr::Arrow { params, body } => {
            let mut local = scope.nested();
            let mut counter = Counter::default();
            let params: Vec<String> = params
                .into_iter()
                .map(|p| fresh_local(&p, &mut local, &mut counter, pool))
                .collect();
            Expr::Arrow { params, body: Box::new(rename(*body, &local, pool)) }
        }
        Expr::ArrowBlock { params, body } => {
            let mut local = scope.nested();
            let mut counter = Counter::default();
            let params: Vec<String> = params
                .into_iter()
                .map(|p| fresh_local(&p, &mut local, &mut counter, pool))
                .collect();
            Expr::ArrowBlock {
                params,
                body: rename_scope(body, &mut local, &mut counter, pool),
            }
        }
        Expr::Spread(x) => Expr::Spread(Box::new(rename(*x, scope, pool))),
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Whitespace stripping, for the hand-written runtime
// ---------------------------------------------------------------------------

/// Splits the runtime into one entry per top-level declaration, so dead code
/// elimination can drop the ones a program does not reach. Brace depth is
/// tracked with the same tokenizer `strip` uses, so a brace inside a string or
/// a comment does not end a declaration.
pub fn split_declarations(src: &str) -> Vec<(String, String)> {
    let b = src.as_bytes();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut i = 0usize;
    let mut start = 0usize;
    let mut depth = 0i32;
    // The name, and whether it is a `function` — which ends at its closing
    // brace, where a `const` or `let` ends at its semicolon.
    let mut current: Option<(String, bool)> = None;

    while let Some(&c) = b.get(i) {
        // Skip over anything whose contents must not be scanned for braces.
        match c {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while b.get(i).is_some_and(|c| *c != b'\n') {
                    i += 1;
                }
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b.get(i) == Some(&b'*') && b.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i = (i + 2).min(b.len());
                continue;
            }
            b'"' | b'\'' | b'`' => {
                let q = c;
                i += 1;
                while let Some(&c) = b.get(i) {
                    if c == b'\\' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    if c == q {
                        break;
                    }
                }
                continue;
            }
            b'{' => depth += 1,
            // A `function` ends at the brace that closes it.
            b'}' if depth == 1 && matches!(current, Some((_, true))) => {
                depth -= 1;
                if let Some((name, _)) = current.take() {
                    i += 1;
                    out.push((name, src.get(start..i).unwrap_or_default().to_string()));
                    start = i;
                    continue;
                }
            }
            b'}' => depth -= 1,
            b';' if depth == 0 && matches!(current, Some((_, false))) => {
                if let Some((name, _)) = current.take() {
                    i += 1;
                    out.push((name, src.get(start..i).unwrap_or_default().to_string()));
                    start = i;
                    continue;
                }
            }
            _ => {}
        }

        if depth == 0 && current.is_none() {
            for kw in [b"function $".as_slice(), b"const $".as_slice(), b"let $".as_slice()] {
                if b.get(i..).is_some_and(|rest| rest.starts_with(kw)) {
                    // The keyword ends with the `$` that begins the name.
                    let ns = i + kw.len() - 1;
                    let mut j = ns;
                    while b
                        .get(j)
                        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'$')
                    {
                        j += 1;
                    }
                    if let Some(Ok(name)) = b.get(ns..j).map(std::str::from_utf8) {
                        current = Some((name.to_string(), kw.starts_with(b"function")));
                        start = i;
                    }
                    break;
                }
            }
        }
        i += 1;
    }
    out
}

/// Removes comments and unnecessary whitespace from JavaScript source, with a
/// tokenizer that understands strings, template literals, and both comment
/// forms. This is what compacts the runtime; generated code goes through the
/// AST printer instead.
pub fn strip(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        match c {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while b.get(i).is_some_and(|c| *c != b'\n') {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b.get(i) == Some(&b'*') && b.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i = (i + 2).min(b.len());
            }
            b'"' | b'\'' | b'`' => {
                let quote = c;
                out.push(c as char);
                i += 1;
                while let Some(&c) = b.get(i) {
                    if c == b'\\' {
                        out.push('\\');
                        i += 1;
                        i += copy_char(src, i, &mut out);
                        continue;
                    }
                    i += copy_char(src, i, &mut out);
                    if c == quote {
                        break;
                    }
                }
            }
            b' ' | b'\t' | b'\r' | b'\n' => {
                // Keep one space only where removing it would join two tokens.
                let prev = out.as_bytes().last().copied().unwrap_or(0);
                let mut j = i;
                while b.get(j).is_some_and(|c| matches!(c, b' ' | b'\t' | b'\r' | b'\n')) {
                    j += 1;
                }
                let next = b.get(j).copied().unwrap_or(0);
                if word_char(prev) && word_char(next) {
                    out.push(' ');
                }
                i = j;
            }
            _ => i += copy_char(src, i, &mut out),
        }
    }
    out
}

/// Copies the character at `i` to `out` and answers its width in bytes.
///
/// The tokenizer above decides everything from ASCII bytes, so this is the
/// only place a multi-byte character is reached — and it is reached whole,
/// rather than a byte at a time, which is what keeps a `·` in a string
/// literal a `·`.
fn copy_char(src: &str, i: usize, out: &mut String) -> usize {
    match src.get(i..).and_then(|rest| rest.chars().next()) {
        Some(c) => {
            out.push(c);
            c.len_utf8()
        }
        // `i` is past the end, or inside a character whose start the caller
        // has already stepped over.
        None => 1,
    }
}

fn word_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(e: Expr) -> String {
        print(&[Stmt::Expr(e)], false)
    }

    #[test]
    fn precedence_adds_only_the_parentheses_it_needs() {
        // Named operands, not literals: `Expr::bin` folds what it can, and a
        // folded tree has no precedence left to test.
        let e = Expr::bin(
            BinOp::Mul,
            Expr::bin(BinOp::Add, Expr::ident("a"), Expr::ident("b")),
            Expr::ident("c"),
        );
        assert_eq!(p(e), "(a+b)*c;");
        let e = Expr::bin(
            BinOp::Add,
            Expr::ident("a"),
            Expr::bin(BinOp::Mul, Expr::ident("b"), Expr::ident("c")),
        );
        assert_eq!(p(e), "a+b*c;");
    }

    #[test]
    fn subtraction_is_left_associative() {
        let e = Expr::bin(
            BinOp::Sub,
            Expr::ident("a"),
            Expr::bin(BinOp::Sub, Expr::ident("b"), Expr::ident("c")),
        );
        assert_eq!(p(e), "a-(b-c);");
    }

    #[test]
    fn negative_operands_keep_their_space() {
        let e = Expr::bin(BinOp::Sub, Expr::ident("a"), Expr::un(UnOp::Neg, Expr::ident("b")));
        assert_eq!(p(e), "a- -b;");
    }

    #[test]
    fn strings_pick_the_cheaper_quote() {
        assert_eq!(quote("it's"), "\"it's\"");
        assert_eq!(quote("say \"hi\""), "'say \"hi\"'");
    }

    #[test]
    fn numbers_are_short() {
        assert_eq!(number(0.5), ".5");
        assert_eq!(number(-0.25), "-.25");
        assert_eq!(number(1e21), "1e21");
    }

    #[test]
    fn constant_folding_removes_dead_branches() {
        let s = Stmt::If {
            cond: Expr::bin(BinOp::Lt, Expr::Num(1.0), Expr::Num(2.0)),
            then: vec![Stmt::Return(Some(Expr::Num(1.0)))],
            else_: vec![Stmt::Return(Some(Expr::Num(2.0)))],
        };
        let out = print(&fold_block(vec![s]), false);
        assert_eq!(out, "return 1;");
    }

    /// Printing a folded expression, for the peephole cases below.
    fn f(e: Expr) -> String {
        let s = print(&[Stmt::Expr(e)], false);
        s.trim_end_matches(';').to_string()
    }

    fn a() -> Expr {
        Expr::ident("a")
    }
    fn b() -> Expr {
        Expr::ident("b")
    }

    /// `a === b` — a boolean the peepholes are allowed to reason about.
    fn cmp() -> Expr {
        Expr::bin(BinOp::StrictEq, a(), b())
    }

    #[test]
    fn a_negated_equality_flips_its_operator() {
        assert_eq!(f(Expr::un(UnOp::Not, cmp())), "a!==b");
        assert_eq!(f(Expr::un(UnOp::Not, Expr::un(UnOp::Not, cmp()))), "a===b");
    }

    /// `!(a < b)` is not `a >= b`: NaN compares false both ways. The rewrite
    /// exists for equality alone, and this is what says so.
    #[test]
    fn a_negated_relational_operator_is_left_alone() {
        assert_eq!(f(Expr::un(UnOp::Not, Expr::bin(BinOp::Lt, a(), b()))), "!(a<b)");
    }

    /// `!!e` is `e` for a boolean and `Boolean(e)` for anything else, so the
    /// rewrite has to see that `e` is one.
    #[test]
    fn double_negation_only_collapses_on_a_boolean() {
        let twice = Expr::un(UnOp::Not, Expr::un(UnOp::Not, a()));
        assert_eq!(f(twice), "!!a");
    }

    #[test]
    fn a_boolean_compared_against_a_literal_is_itself() {
        assert_eq!(f(Expr::bin(BinOp::StrictEq, cmp(), Expr::Bool(true))), "a===b");
        assert_eq!(f(Expr::bin(BinOp::StrictEq, cmp(), Expr::Bool(false))), "a!==b");
    }

    /// `1 && true` is `true`, not `1`, so the identity needs a boolean.
    #[test]
    fn short_circuit_identities_need_a_boolean() {
        assert_eq!(f(Expr::bin(BinOp::And, cmp(), Expr::Bool(true))), "a===b");
        assert_eq!(f(Expr::bin(BinOp::And, a(), Expr::Bool(true))), "a&&true");
        assert_eq!(f(Expr::bin(BinOp::Or, cmp(), Expr::Bool(false))), "a===b");
    }

    #[test]
    fn a_test_repeated_is_the_test() {
        assert_eq!(f(Expr::bin(BinOp::And, cmp(), cmp())), "a===b");
    }

    /// The shape an or-pattern's alternatives collapse to once each has been
    /// narrowed by the branch it sits under.
    #[test]
    fn one_value_cannot_equal_two_constants() {
        let is1 = Expr::bin(BinOp::StrictEq, a(), Expr::Num(1.0));
        let is2 = Expr::bin(BinOp::StrictEq, a(), Expr::Num(2.0));
        assert_eq!(f(Expr::bin(BinOp::And, is1, is2)), "false");
    }

    #[test]
    fn a_conditional_yielding_booleans_is_its_own_test() {
        assert_eq!(f(Expr::cond(cmp(), Expr::Bool(true), Expr::Bool(false))), "a===b");
        assert_eq!(f(Expr::cond(cmp(), Expr::Bool(false), Expr::Bool(true))), "a!==b");
    }

    #[test]
    fn a_conditional_with_one_answer_needs_no_test() {
        assert_eq!(f(Expr::cond(cmp(), a(), a())), "a");
    }

    #[test]
    fn a_constant_branch_becomes_a_short_circuit() {
        assert_eq!(f(Expr::cond(cmp(), Expr::Bool(true), Expr::ident("x"))), "a===b||x");
        assert_eq!(f(Expr::cond(cmp(), Expr::ident("x"), Expr::Bool(false))), "a===b&&x");
        assert_eq!(f(Expr::cond(cmp(), Expr::Bool(false), Expr::ident("x"))), "a!==b&&x");
        assert_eq!(f(Expr::cond(cmp(), Expr::ident("x"), Expr::Bool(true))), "a!==b||x");
    }

    /// `5 ? true : x` is `true`, where `5 || x` is `5`. Without a boolean test
    /// the short circuit is a different program.
    #[test]
    fn a_constant_branch_needs_a_boolean_test() {
        assert_eq!(f(Expr::cond(a(), Expr::Bool(true), Expr::ident("x"))), "a?true:x");
    }

    #[test]
    fn conditionals_that_share_an_arm_merge_into_one_test() {
        let inner = Expr::cond(Expr::ident("p"), Expr::ident("x"), Expr::ident("y"));
        // `c ? (p ? x : y) : y`  ->  `c && p ? x : y`
        assert_eq!(
            f(Expr::cond(cmp(), inner.clone(), Expr::ident("y"))),
            "a===b&&p?x:y"
        );
        // `c ? x : (p ? x : y)`  ->  `c || p ? x : y`
        assert_eq!(
            f(Expr::cond(cmp(), Expr::ident("x"), inner)),
            "a===b||p?x:y"
        );
    }

    #[test]
    fn two_branches_that_only_return_become_one_expression() {
        let s = Stmt::If {
            cond: cmp(),
            then: vec![Stmt::Return(Some(Expr::Num(1.0)))],
            else_: vec![Stmt::Return(Some(Expr::Num(2.0)))],
        };
        assert_eq!(print(&fold_block(vec![s]), false), "return a===b?1:2;");
    }

    /// A chain collapses from the inside out, because folding is bottom-up.
    #[test]
    fn a_chain_of_returning_branches_collapses_entirely() {
        let inner = Stmt::If {
            cond: Expr::bin(BinOp::Gt, a(), Expr::Num(10.0)),
            then: vec![Stmt::Return(Some(Expr::Str("big".into())))],
            else_: vec![Stmt::Return(Some(Expr::Str("small".into())))],
        };
        let outer = Stmt::If {
            cond: Expr::bin(BinOp::Lt, a(), Expr::Num(0.0)),
            then: vec![Stmt::Return(Some(Expr::Str("neg".into())))],
            else_: vec![inner],
        };
        assert_eq!(
            print(&fold_block(vec![outer]), false),
            "return a<0?'neg':a>10?'big':'small';"
        );
    }

    #[test]
    fn two_branches_assigning_one_target_become_one_assignment() {
        let s = Stmt::If {
            cond: cmp(),
            then: vec![Stmt::Expr(Expr::Assign {
                target: Box::new(Expr::ident("r")),
                value: Box::new(Expr::Num(1.0)),
            })],
            else_: vec![Stmt::Expr(Expr::Assign {
                target: Box::new(Expr::ident("r")),
                value: Box::new(Expr::Num(2.0)),
            })],
        };
        assert_eq!(print(&fold_block(vec![s]), false), "r=a===b?1:2;");
    }

    #[test]
    fn dead_code_elimination_keeps_only_what_is_reachable() {
        let stmts = vec![
            Stmt::Func { name: "used".into(), params: vec![], body: vec![] },
            Stmt::Func { name: "unused".into(), params: vec![], body: vec![] },
            Stmt::Func {
                name: "main".into(),
                params: vec![],
                body: vec![Stmt::Expr(Expr::call(Expr::ident("used"), vec![]))],
            },
        ];
        let kept = eliminate_dead(stmts, &["main".to_string()]);
        let names: Vec<String> = kept
            .iter()
            .filter_map(|s| match s {
                Stmt::Func { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["used", "main"]);
    }

    #[test]
    fn mangling_shortens_globals_and_locals() {
        let stmts = vec![Stmt::Func {
            name: "someLongName".into(),
            params: vec!["parameterOne".into()],
            body: vec![Stmt::Return(Some(Expr::ident("parameterOne")))],
        }];
        let out = print(&minify(stmts, &[], &MinifyOptions::default()), false);
        assert!(!out.contains("someLongName"), "{out}");
        assert!(!out.contains("parameterOne"), "{out}");
    }

    #[test]
    fn roots_are_never_renamed() {
        let stmts = vec![Stmt::Func { name: "main".into(), params: vec![], body: vec![] }];
        let out = print(&minify(stmts, &["main".to_string()], &MinifyOptions::default()), false);
        assert!(out.contains("function main("), "{out}");
    }

    #[test]
    fn stripping_keeps_tokens_apart() {
        assert_eq!(strip("let  a  =  1 ;"), "let a=1;");
        assert_eq!(strip("a /* c */ + b"), "a+b");
        assert_eq!(strip("// gone\nx"), "x");
        assert_eq!(strip("return  x"), "return x");
        assert_eq!(strip("f('a  b')"), "f('a  b')");
    }

    #[test]
    fn short_names_are_distinct() {
        let mut seen = HashSet::default();
        for i in 0..2000 {
            assert!(seen.insert(short_name(i)), "collision at {i}");
        }
    }
}

#[cfg(test)]
mod shared_constant_tests {
    use super::*;

    #[test]
    fn a_known_slot_of_a_shared_constant_reads_through() {
        let stmts = vec![
            Stmt::Var {
                kind: VarKind::Const,
                name: "$k0".into(),
                init: Some(Expr::Array(vec![Expr::Num(3.0), Expr::Str("x".into())])),
            },
            Stmt::Func {
                name: "f".into(),
                params: vec![],
                body: vec![Stmt::Return(Some(Expr::index(
                    Expr::ident("$k0"),
                    Expr::Num(0.0),
                )))],
            },
        ];
        let table = constant_table(&stmts);
        let out: Vec<Stmt> =
            stmts.into_iter().map(|s| read_through_stmt(s, &table)).collect();
        let printed = print(&out, false);
        assert!(printed.contains("return 3;"), "did not read through: {printed}");
    }
}
