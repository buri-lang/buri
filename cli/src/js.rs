//! The JavaScript AST, its printer, and the minifier.
//!
//! The backend builds this tree rather than strings, so minification is a set
//! of passes over a real structure: dead code elimination by reachability,
//! constant folding, dead-branch removal, and identifier mangling with proper
//! scope analysis. The printer emits no whitespace it does not need and
//! inserts parentheses only where precedence requires them.

use std::collections::{HashMap, HashSet};

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

    pub fn bin(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
        Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }
    }

    pub fn un(op: UnOp, operand: Expr) -> Expr {
        Expr::Unary { op, operand: Box::new(operand) }
    }

    pub fn cond(test: Expr, cons: Expr, alt: Expr) -> Expr {
        Expr::Cond { test: Box::new(test), cons: Box::new(cons), alt: Box::new(alt) }
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
                    if else_.len() == 1 && matches!(else_[0], Stmt::If { .. }) {
                        self.out.push(' ');
                        let saved = std::mem::take(&mut self.out);
                        self.stmt(&else_[0]);
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
        if params.len() == 1 {
            self.out.push_str(&params[0]);
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
        stmts = fold_block(stmts);
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
            Stmt::If { cond, then, else_ }
        }
        Stmt::While { cond, body } => Stmt::While { cond: fold(cond), body: fold_block(body) },
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

fn fold(e: Expr) -> Expr {
    match e {
        Expr::Unary { op, operand } => {
            let operand = fold(*operand);
            match (op, &operand) {
                (UnOp::Not, Expr::Bool(b)) => Expr::Bool(!b),
                (UnOp::Neg, Expr::Num(n)) => Expr::Num(-n),
                _ => Expr::Unary { op, operand: Box::new(operand) },
            }
        }
        Expr::Binary { op, lhs, rhs } => {
            let lhs = fold(*lhs);
            let rhs = fold(*rhs);
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
            match (op, &lhs, &rhs) {
                (BinOp::And, Expr::Bool(true), _) => return rhs,
                (BinOp::And, Expr::Bool(false), _) => return Expr::Bool(false),
                (BinOp::Or, Expr::Bool(true), _) => return Expr::Bool(true),
                (BinOp::Or, Expr::Bool(false), _) => return rhs,
                _ => {}
            }
            Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }
        }
        Expr::Cond { test, cons, alt } => {
            let test = fold(*test);
            let cons = fold(*cons);
            let alt = fold(*alt);
            match test {
                Expr::Bool(true) => cons,
                Expr::Bool(false) => alt,
                t => Expr::Cond { test: Box::new(t), cons: Box::new(cons), alt: Box::new(alt) },
            }
        }
        Expr::Call { callee, args } => Expr::Call {
            callee: Box::new(fold(*callee)),
            args: args.into_iter().map(fold).collect(),
        },
        Expr::New { callee, args } => Expr::New {
            callee: Box::new(fold(*callee)),
            args: args.into_iter().map(fold).collect(),
        },
        Expr::Member { obj, prop } => Expr::Member { obj: Box::new(fold(*obj)), prop },
        Expr::Index { obj, index } => {
            Expr::Index { obj: Box::new(fold(*obj)), index: Box::new(fold(*index)) }
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
            if body.len() == 1 {
                if let Stmt::Return(Some(e)) = &body[0] {
                    return Expr::Arrow { params, body: Box::new(e.clone()) };
                }
            }
            Expr::ArrowBlock { params, body }
        }
        Expr::Seq(xs) => Expr::Seq(xs.into_iter().map(fold).collect()),
        Expr::Spread(x) => Expr::Spread(Box::new(fold(*x))),
        other => other,
    }
}

// -- dead code elimination ---------------------------------------------------

/// Drops top-level declarations nothing reachable from `roots` names. Since
/// the backend emits one top-level function per reachable instance, this is
/// what removes the parts of `core/*` a program does not use.
fn eliminate_dead(stmts: Vec<Stmt>, roots: &[String]) -> Vec<Stmt> {
    let mut deps: HashMap<String, HashSet<String>> = HashMap::new();
    let mut declared: HashMap<String, usize> = HashMap::new();
    for (i, s) in stmts.iter().enumerate() {
        match s {
            Stmt::Func { name, body, params } => {
                let mut used = HashSet::new();
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
                let mut used = HashSet::new();
                if let Some(e) = init {
                    collect_idents(e, &mut used);
                }
                declared.insert(name.clone(), i);
                deps.insert(name.clone(), used);
            }
            Stmt::RawDecl { name, src } => {
                let mut used = HashSet::new();
                collect_idents_raw(src, &mut used);
                used.remove(name);
                declared.insert(name.clone(), i);
                deps.insert(name.clone(), used);
            }
            _ => {}
        }
    }

    let mut live: HashSet<String> = HashSet::new();
    let mut stack: Vec<String> = roots.to_vec();
    // Anything a non-declaration statement mentions is a root too: those run
    // for their effect and are never dropped.
    for s in &stmts {
        if !is_declaration(s) {
            let mut used = HashSet::new();
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
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$')
            {
                i += 1;
            }
            out.insert(src[start..i].to_string());
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

    let mut map: HashMap<String, String> = HashMap::new();
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

    stmts.into_iter().map(|s| rename_stmt(s, &map)).collect()
}

fn rename_stmt(s: Stmt, map: &HashMap<String, String>) -> Stmt {
    match s {
        Stmt::Var { kind, name, init } => Stmt::Var {
            kind,
            name: map.get(&name).cloned().unwrap_or(name),
            init: init.map(|e| rename(e, map)),
        },
        Stmt::Func { name, params, body } => {
            // Parameters and locals get their own short names, reused across
            // functions because each is a fresh scope.
            let mut local = map.clone();
            let mut counter = 0usize;
            let params: Vec<String> =
                params.into_iter().map(|p| fresh_local(&p, &mut local, &mut counter)).collect();
            let body = rename_scope(body, &mut local, &mut counter);
            Stmt::Func { name: map.get(&name).cloned().unwrap_or(name), params, body }
        }
        Stmt::Return(e) => Stmt::Return(e.map(|x| rename(x, map))),
        Stmt::If { cond, then, else_ } => Stmt::If {
            cond: rename(cond, map),
            then: then.into_iter().map(|x| rename_stmt(x, map)).collect(),
            else_: else_.into_iter().map(|x| rename_stmt(x, map)).collect(),
        },
        Stmt::While { cond, body } => Stmt::While {
            cond: rename(cond, map),
            body: body.into_iter().map(|x| rename_stmt(x, map)).collect(),
        },
        Stmt::Switch { disc, cases } => Stmt::Switch {
            disc: rename(disc, map),
            cases: cases
                .into_iter()
                .map(|(t, b)| {
                    (
                        t.map(|x| rename(x, map)),
                        b.into_iter().map(|x| rename_stmt(x, map)).collect(),
                    )
                })
                .collect(),
        },
        Stmt::Expr(e) => Stmt::Expr(rename(e, map)),
        Stmt::Throw(e) => Stmt::Throw(rename(e, map)),
        Stmt::Block(b) => Stmt::Block(b.into_iter().map(|x| rename_stmt(x, map)).collect()),
        Stmt::ExportDefault(e) => Stmt::ExportDefault(rename(e, map)),
        other => other,
    }
}

fn fresh_local(
    original: &str,
    map: &mut HashMap<String, String>,
    counter: &mut usize,
) -> String {
    loop {
        let candidate = short_name(*counter);
        *counter += 1;
        if RESERVED.contains(&candidate.as_str()) {
            continue;
        }
        // A local may not shadow a global the body still needs.
        if map.values().any(|v| v == &candidate) {
            continue;
        }
        map.insert(original.to_string(), candidate.clone());
        return candidate;
    }
}

fn rename_scope(
    body: Vec<Stmt>,
    map: &mut HashMap<String, String>,
    counter: &mut usize,
) -> Vec<Stmt> {
    let mut out = Vec::new();
    for s in body {
        match s {
            Stmt::Var { kind, name, init } => {
                let init = init.map(|e| rename(e, map));
                let renamed = fresh_local(&name, map, counter);
                out.push(Stmt::Var { kind, name: renamed, init });
            }
            Stmt::If { cond, then, else_ } => {
                let cond = rename(cond, map);
                let then = rename_scope(then, &mut map.clone(), &mut counter.clone());
                let else_ = rename_scope(else_, &mut map.clone(), &mut counter.clone());
                out.push(Stmt::If { cond, then, else_ });
            }
            Stmt::While { cond, body } => {
                let cond = rename(cond, map);
                let body = rename_scope(body, &mut map.clone(), &mut counter.clone());
                out.push(Stmt::While { cond, body });
            }
            Stmt::Block(b) => {
                out.push(Stmt::Block(rename_scope(b, &mut map.clone(), &mut counter.clone())));
            }
            Stmt::Switch { disc, cases } => {
                let disc = rename(disc, map);
                let cases = cases
                    .into_iter()
                    .map(|(t, b)| {
                        (
                            t.map(|x| rename(x, map)),
                            rename_scope(b, &mut map.clone(), &mut counter.clone()),
                        )
                    })
                    .collect();
                out.push(Stmt::Switch { disc, cases });
            }
            other => out.push(rename_stmt(other, map)),
        }
    }
    out
}

fn rename(e: Expr, map: &HashMap<String, String>) -> Expr {
    match e {
        Expr::Ident(name) => Expr::Ident(map.get(&name).cloned().unwrap_or(name)),
        Expr::Array(xs) => Expr::Array(xs.into_iter().map(|x| rename(x, map)).collect()),
        Expr::Seq(xs) => Expr::Seq(xs.into_iter().map(|x| rename(x, map)).collect()),
        Expr::Object(fs) => {
            Expr::Object(fs.into_iter().map(|(k, v)| (k, rename(v, map))).collect())
        }
        Expr::Member { obj, prop } => Expr::Member { obj: Box::new(rename(*obj, map)), prop },
        Expr::Index { obj, index } => Expr::Index {
            obj: Box::new(rename(*obj, map)),
            index: Box::new(rename(*index, map)),
        },
        Expr::Call { callee, args } => Expr::Call {
            callee: Box::new(rename(*callee, map)),
            args: args.into_iter().map(|a| rename(a, map)).collect(),
        },
        Expr::New { callee, args } => Expr::New {
            callee: Box::new(rename(*callee, map)),
            args: args.into_iter().map(|a| rename(a, map)).collect(),
        },
        Expr::Unary { op, operand } => {
            Expr::Unary { op, operand: Box::new(rename(*operand, map)) }
        }
        Expr::Binary { op, lhs, rhs } => Expr::Binary {
            op,
            lhs: Box::new(rename(*lhs, map)),
            rhs: Box::new(rename(*rhs, map)),
        },
        Expr::Assign { target, value } => Expr::Assign {
            target: Box::new(rename(*target, map)),
            value: Box::new(rename(*value, map)),
        },
        Expr::Cond { test, cons, alt } => Expr::Cond {
            test: Box::new(rename(*test, map)),
            cons: Box::new(rename(*cons, map)),
            alt: Box::new(rename(*alt, map)),
        },
        Expr::Arrow { params, body } => {
            let mut local = map.clone();
            let mut counter = 0usize;
            let params: Vec<String> =
                params.into_iter().map(|p| fresh_local(&p, &mut local, &mut counter)).collect();
            Expr::Arrow { params, body: Box::new(rename(*body, &local)) }
        }
        Expr::ArrowBlock { params, body } => {
            let mut local = map.clone();
            let mut counter = 0usize;
            let params: Vec<String> =
                params.into_iter().map(|p| fresh_local(&p, &mut local, &mut counter)).collect();
            Expr::ArrowBlock { params, body: rename_scope(body, &mut local, &mut counter) }
        }
        Expr::Spread(x) => Expr::Spread(Box::new(rename(*x, map))),
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

    while i < b.len() {
        // Skip over anything whose contents must not be scanned for braces.
        match b[i] {
            b'/' if i + 1 < b.len() && b[i + 1] == b'/' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if i + 1 < b.len() && b[i + 1] == b'*' => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(b.len());
                continue;
            }
            b'"' | b'\'' | b'`' => {
                let q = b[i];
                i += 1;
                while i < b.len() {
                    if b[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if b[i] == q {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    if matches!(current, Some((_, true))) {
                        let (name, _) = current.take().unwrap();
                        i += 1;
                        out.push((name, src[start..i].to_string()));
                        start = i;
                        continue;
                    }
                }
            }
            b';' if depth == 0 => {
                if matches!(current, Some((_, false))) {
                    let (name, _) = current.take().unwrap();
                    i += 1;
                    out.push((name, src[start..i].to_string()));
                    start = i;
                    continue;
                }
            }
            _ => {}
        }

        if depth == 0 && current.is_none() {
            for kw in [b"function $".as_slice(), b"const $".as_slice(), b"let $".as_slice()] {
                if b[i..].starts_with(kw) {
                    let ns = i + kw.len() - 1;
                    let mut j = ns;
                    while j < b.len()
                        && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b'$')
                    {
                        j += 1;
                    }
                    if let Ok(name) = std::str::from_utf8(&b[ns..j]) {
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
    while i < b.len() {
        let c = b[i];
        match c {
            b'/' if i + 1 < b.len() && b[i + 1] == b'/' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < b.len() && b[i + 1] == b'*' => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(b.len());
            }
            b'"' | b'\'' | b'`' => {
                let quote = c;
                out.push(c as char);
                i += 1;
                while i < b.len() {
                    if b[i] == b'\\' {
                        out.push(b[i] as char);
                        if i + 1 < b.len() {
                            out.push(b[i + 1] as char);
                        }
                        i += 2;
                        continue;
                    }
                    out.push_str(&src[i..i + 1]);
                    if b[i] == quote {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b' ' | b'\t' | b'\r' | b'\n' => {
                // Keep one space only where removing it would join two tokens.
                let prev = out.as_bytes().last().copied().unwrap_or(0);
                let mut j = i;
                while j < b.len() && matches!(b[j], b' ' | b'\t' | b'\r' | b'\n') {
                    j += 1;
                }
                let next = b.get(j).copied().unwrap_or(0);
                if word_char(prev) && word_char(next) {
                    out.push(' ');
                }
                i = j;
            }
            _ => {
                out.push_str(&src[i..i + 1]);
                i += 1;
            }
        }
    }
    out
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
        let e = Expr::bin(
            BinOp::Mul,
            Expr::bin(BinOp::Add, Expr::Num(1.0), Expr::Num(2.0)),
            Expr::Num(3.0),
        );
        // Folding is a separate pass; the printer alone keeps the structure.
        assert_eq!(p(e), "(1+2)*3;");
        let e = Expr::bin(
            BinOp::Add,
            Expr::Num(1.0),
            Expr::bin(BinOp::Mul, Expr::Num(2.0), Expr::Num(3.0)),
        );
        assert_eq!(p(e), "1+2*3;");
    }

    #[test]
    fn subtraction_is_left_associative() {
        let e = Expr::bin(
            BinOp::Sub,
            Expr::Num(1.0),
            Expr::bin(BinOp::Sub, Expr::Num(2.0), Expr::Num(3.0)),
        );
        assert_eq!(p(e), "1-(2-3);");
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
        let mut seen = HashSet::new();
        for i in 0..2000 {
            assert!(seen.insert(short_name(i)), "collision at {i}");
        }
    }
}
