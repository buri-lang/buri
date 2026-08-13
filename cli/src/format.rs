//! The source formatter.
//!
//! No options and no configuration file: a formatter with options is a
//! formatter whose output is a repository decision. It re-prints the parsed
//! tree, keeping doc comments and ordinary comments with the declaration
//! beneath them.

use crate::ast::*;
use crate::diag::FileId;
use crate::lex::{lex, Tok};
use std::fmt::Write as _;

const WIDTH: usize = 88;

/// The formatter without its safety check, for the toolchain's own tests.
pub fn source_unchecked(text: &str) -> String {
    let parsed = crate::parse::parse(text, FileId(0));
    let comments = leading_comments(text);
    let mut f = Fmt { out: String::new(), depth: 0, comments };
    f.module(&parsed.module, text);
    f.out
}

/// Returns `None` when the file does not parse, in which case it is left
/// exactly as it is.
pub fn source(text: &str) -> Option<String> {
    let parsed = crate::parse::parse(text, FileId(0));
    if !parsed.errors.is_empty() {
        return None;
    }
    let comments = leading_comments(text);
    let mut f = Fmt { out: String::new(), depth: 0, comments };
    f.module(&parsed.module, text);
    let out = f.out;

    // A formatter that produces something that does not parse is worse than no
    // formatter, so the output is checked before it is offered.
    let check = crate::parse::parse(&out, FileId(0));
    if !check.errors.is_empty() {
        return None;
    }
    Some(out)
}

/// Byte offset of a declaration -> the comment lines above it.
fn leading_comments(text: &str) -> Vec<(u32, Vec<String>, Vec<String>, bool)> {
    let lexed = lex(text, FileId(0));
    lexed
        .tokens
        .iter()
        .filter(|t| !t.comments.is_empty() || !t.docs.is_empty() || t.blank_before)
        .map(|t| (t.span.start, t.comments.clone(), t.docs.clone(), t.blank_before))
        .collect()
}

struct Fmt {
    out: String,
    depth: usize,
    comments: Vec<(u32, Vec<String>, Vec<String>, bool)>,
}

impl Fmt {
    fn pad(&mut self) {
        for _ in 0..self.depth {
            self.out.push_str("  ");
        }
    }

    fn line(&mut self, s: &str) {
        self.pad();
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn module(&mut self, m: &Module, _text: &str) {
        for (i, item) in m.items.iter().enumerate() {
            self.emit_trivia(item.span().start, i == 0);
            self.item(item);
        }
    }

    /// Comments and blank lines above a declaration come back where they were.
    fn emit_trivia(&mut self, at: u32, first: bool) {
        let Some((_, comments, docs, blank)) =
            self.comments.iter().find(|(o, _, _, _)| *o == at).cloned()
        else {
            if !first && !self.out.is_empty() {
                self.out.push('\n');
            }
            return;
        };
        if (blank || !comments.is_empty() || !docs.is_empty()) && !first && !self.out.is_empty() {
            self.out.push('\n');
        }
        for c in &comments {
            for l in c.lines() {
                self.line(l.trim_end());
            }
        }
        for d in &docs {
            let d = d.clone();
            self.line(&format!("/// {d}").trim_end().to_string());
        }
    }

    fn item(&mut self, item: &Item) {
        match item {
            Item::Import(i) => {
                let clause = match &i.clause {
                    ImportClause::Namespace(n) => format!("* as {}", n.name),
                    ImportClause::Named(specs) => {
                        let inner = specs
                            .iter()
                            .map(|s| match &s.alias {
                                Some(a) => format!("{} as {}", s.name.name, a.name),
                                None => s.name.name.clone(),
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("{{ {inner} }}")
                    }
                };
                let line = format!("from \"{}\" import {clause};", i.path);
                self.line(&line);
            }
            Item::ReExport(r) => {
                let inner = r
                    .specs
                    .iter()
                    .map(|s| match &s.alias {
                        Some(a) => format!("{} as {}", s.name.name, a.name),
                        None => s.name.name.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                self.line(&format!("from \"{}\" export {{ {inner} }};", r.path));
            }
            Item::Fn(d) => self.fn_decl(d, d.exported),
            Item::Struct(d) => self.struct_decl(d),
            Item::Enum(d) => self.enum_decl(d),
            Item::TypeAlias(d) => {
                let ex = if d.exported { "export " } else { "" };
                let g = generics(&d.generics);
                self.line(&format!("{ex}type {}{g} = {};", d.name.name, ty(&d.ty)));
            }
            Item::Const(d) => {
                let ex = if d.exported { "export " } else { "" };
                self.line(&format!(
                    "{ex}const {}: {} = {};",
                    d.name.name,
                    ty(&d.ty),
                    expr(&d.value)
                ));
            }
            Item::Trait(d) => {
                let ex = if d.exported { "export " } else { "" };
                let kw = if d.is_effect { "effect" } else { "trait" };
                let g = generics(&d.generics);
                if d.methods.is_empty() {
                    self.line(&format!("{ex}{kw} {}{g} {{}}", d.name.name));
                    return;
                }
                self.line(&format!("{ex}{kw} {}{g} {{", d.name.name));
                self.depth += 1;
                for m in &d.methods {
                    self.line(&format!("{};", signature(m)));
                }
                self.depth -= 1;
                self.line("}");
            }
            Item::Impl(d) => {
                let g = generics(&d.generics);
                self.line(&format!(
                    "impl{g} {} for {} {{",
                    ty(&d.trait_ty),
                    ty(&d.self_ty)
                ));
                self.depth += 1;
                for (i, m) in d.methods.iter().enumerate() {
                    if i > 0 {
                        self.out.push('\n');
                    }
                    self.fn_decl(m, false);
                }
                self.depth -= 1;
                self.line("}");
            }
            Item::Derive(d) => {
                let traits =
                    d.traits.iter().map(ty).collect::<Vec<_>>().join(", ");
                self.line(&format!("derive {traits} for {};", ty(&d.self_ty)));
            }
            Item::Context(d) => {
                let ex = if d.exported { "export " } else { "" };
                self.line(&format!("{ex}context {} {{", d.name.name));
                self.depth += 1;
                self.context_body(&d.body);
                self.depth -= 1;
                self.line("}");
            }
            Item::Test(d) => {
                self.line(&format!("test {} {{", quote(&d.name)));
                self.depth += 1;
                self.block_inner(&d.body);
                self.depth -= 1;
                self.line("}");
            }
        }
    }

    fn context_body(&mut self, body: &ContextBody) {
        if let Some(s) = &body.spread {
            self.line(&format!("..{},", expr(s)));
        }
        // Aligned values, which is how every context in the spec is written.
        let width = body
            .bindings
            .iter()
            .map(|b| ty(&b.effect).len())
            .max()
            .unwrap_or(0);
        for b in &body.bindings {
            let name = ty(&b.effect);
            let pad = " ".repeat(width - name.len());
            self.line(&format!("{name}:{pad} {},", expr(&b.value)));
        }
    }

    fn fn_decl(&mut self, d: &FnDecl, exported: bool) {
        let head = format!("{}{}", if exported { "export " } else { "" }, signature(d));
        match &d.body {
            None => self.line(&format!("{head};")),
            Some(b) => {
                // A one-expression body stays on one line when it fits.
                if b.stmts.is_empty() {
                    if let Some(tail) = &b.tail {
                        let one = format!("{head} {{ {} }}", expr(tail));
                        if one.len() + self.depth * 2 <= WIDTH && !one.contains('\n') {
                            self.line(&one);
                            return;
                        }
                    }
                }
                self.line(&format!("{head} {{"));
                self.depth += 1;
                self.block_inner(b);
                self.depth -= 1;
                self.line("}");
            }
        }
    }

    fn struct_decl(&mut self, d: &StructDecl) {
        let ex = if d.exported { "export " } else { "" };
        let g = generics(&d.generics);
        match &d.body {
            StructBody::Tuple(fields) => {
                let inner = fields
                    .iter()
                    .map(|f| {
                        format!("{}{}", if f.exported { "export " } else { "" }, ty(&f.ty))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                self.line(&format!("{ex}struct {}{g}({inner});", d.name.name));
            }
            StructBody::Record(fields) => {
                if fields.is_empty() {
                    self.line(&format!("{ex}struct {}{g} {{}}", d.name.name));
                    return;
                }
                let one = format!(
                    "{ex}struct {}{g} {{ {} }}",
                    d.name.name,
                    fields
                        .iter()
                        .map(|f| field_decl(f))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                if one.len() + self.depth * 2 <= WIDTH && fields.iter().all(|f| f.docs.is_empty()) {
                    self.line(&one);
                    return;
                }
                self.line(&format!("{ex}struct {}{g} {{", d.name.name));
                self.depth += 1;
                for f in fields {
                    for doc in &f.docs {
                        let doc = doc.clone();
                        self.line(&format!("/// {doc}"));
                    }
                    self.line(&format!("{},", field_decl(f)));
                }
                self.depth -= 1;
                self.line("}");
            }
        }
    }

    fn enum_decl(&mut self, d: &EnumDecl) {
        let ex = if d.exported { "export " } else { "" };
        let g = generics(&d.generics);
        if d.variants.is_empty() {
            self.line(&format!("{ex}enum {}{g} {{}}", d.name.name));
            return;
        }
        let rendered: Vec<String> = d.variants.iter().map(variant).collect();
        let one = format!("{ex}enum {}{g} {{ {} }}", d.name.name, rendered.join(", "));
        if one.len() + self.depth * 2 <= WIDTH && d.variants.iter().all(|v| v.docs.is_empty()) {
            self.line(&one);
            return;
        }
        self.line(&format!("{ex}enum {}{g} {{", d.name.name));
        self.depth += 1;
        for (v, r) in d.variants.iter().zip(&rendered) {
            for doc in &v.docs {
                let doc = doc.clone();
                self.line(&format!("/// {doc}"));
            }
            self.line(&format!("{r},"));
        }
        self.depth -= 1;
        self.line("}");
    }

    fn block_inner(&mut self, b: &Block) {
        for s in &b.stmts {
            match s {
                Stmt::Let { pattern, ty: t, value, is_ctx, .. } => {
                    let name = if *is_ctx { "ctx".to_string() } else { pattern_str(pattern) };
                    let ann = t.as_ref().map(|x| format!(": {}", ty(x))).unwrap_or_default();
                    let v = expr(value);
                    let line = format!("let {name}{ann} = {v};");
                    if line.len() + self.depth * 2 <= WIDTH && !line.contains('\n') {
                        self.line(&line);
                    } else {
                        self.line(&format!("let {name}{ann} ="));
                        self.depth += 1;
                        for l in v.lines() {
                            self.line(l);
                        }
                        self.depth -= 1;
                        let last = self.out.trim_end().len();
                        self.out.truncate(last);
                        self.out.push_str(";\n");
                    }
                }
                Stmt::Expr { expr: e, .. } => {
                    self.line(&format!("{};", expr(e)));
                }
            }
        }
        if let Some(t) = &b.tail {
            let rendered = expr(t);
            for l in rendered.lines() {
                self.line(l);
            }
        }
    }
}

fn field_decl(f: &FieldDecl) -> String {
    format!(
        "{}{}: {}",
        if f.exported { "export " } else { "" },
        f.name.name,
        ty(&f.ty)
    )
}

fn variant(v: &Variant) -> String {
    let ex = if v.exported { "export " } else { "" };
    match &v.payload {
        VariantPayload::None => format!("{ex}{}", v.name.name),
        VariantPayload::Tuple(ts) => format!(
            "{ex}{}({})",
            v.name.name,
            ts.iter().map(ty).collect::<Vec<_>>().join(", ")
        ),
        VariantPayload::Record(fs) => format!(
            "{ex}{} {{ {} }}",
            v.name.name,
            fs.iter().map(field_decl).collect::<Vec<_>>().join(", ")
        ),
    }
}

fn signature(d: &FnDecl) -> String {
    let params = d
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name.name, ty(&p.ty)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "fn {}{}({params}): {}",
        d.name.name,
        generics(&d.generics),
        ty(&d.ret)
    )
}

fn generics(g: &[GenericParam]) -> String {
    if g.is_empty() {
        return String::new();
    }
    let inner = g
        .iter()
        .map(|p| {
            if p.bounds.is_empty() {
                p.name.name.clone()
            } else {
                format!(
                    "{}: {}",
                    p.name.name,
                    p.bounds.iter().map(ty).collect::<Vec<_>>().join(" + ")
                )
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("<{inner}>")
}

pub fn ty(t: &TypeExpr) -> String {
    match t {
        TypeExpr::Named { path, args, .. } => {
            let base = path.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(".");
            if args.is_empty() {
                base
            } else {
                format!("{base}<{}>", args.iter().map(ty).collect::<Vec<_>>().join(", "))
            }
        }
        TypeExpr::SelfType { .. } => "Self".into(),
        TypeExpr::Unit { .. } => "()".into(),
        TypeExpr::Tuple { elems, .. } => {
            format!("({})", elems.iter().map(ty).collect::<Vec<_>>().join(", "))
        }
        TypeExpr::Array { elem, .. } => format!("[{}]", ty(elem)),
        TypeExpr::Fn { params, ret, .. } => format!(
            "fn({}) => {}",
            params.iter().map(ty).collect::<Vec<_>>().join(", "),
            ty(ret)
        ),
    }
}

fn quote_char(c: char) -> String {
    match c {
        '\'' => "'\\''".into(),
        '\\' => "'\\\\'".into(),
        '\n' => "'\\n'".into(),
        '\r' => "'\\r'".into(),
        '\t' => "'\\t'".into(),
        '\0' => "'\\0'".into(),
        c if (c as u32) < 0x20 => format!("'\\u{{{:x}}}'", c as u32),
        c => format!("'{c}'"),
    }
}

fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn pattern_str(p: &Pattern) -> String {
    match p {
        Pattern::Wild { .. } => "_".into(),
        Pattern::Bind { name, sub, .. } => match sub {
            Some(s) => format!("{} @ {}", name.name, pattern_str(s)),
            None => name.name.clone(),
        },
        Pattern::LitInt { raw, negative, .. } => {
            format!("{}{raw}", if *negative { "-" } else { "" })
        }
        Pattern::LitFloat { raw, negative, .. } => {
            format!("{}{raw}", if *negative { "-" } else { "" })
        }
        Pattern::LitStr { value, .. } => quote(value),
        Pattern::LitChar { value, .. } => quote_char(*value),
        Pattern::LitBool { value, .. } => value.to_string(),
        Pattern::Unit { .. } => "()".into(),
        Pattern::Tuple { elems, .. } => {
            format!("({})", elems.iter().map(pattern_str).collect::<Vec<_>>().join(", "))
        }
        Pattern::Array { elems, rest, .. } => {
            let mut parts: Vec<String> = elems.iter().map(pattern_str).collect();
            if let Some(r) = rest {
                parts.push(match r {
                    Some(n) => format!("..{}", n.name),
                    None => "..".into(),
                });
            }
            format!("[{}]", parts.join(", "))
        }
        Pattern::Or { alts, .. } => {
            alts.iter().map(pattern_str).collect::<Vec<_>>().join(" | ")
        }
        Pattern::Path { path, dotted, payload, .. } => {
            let base = if *dotted {
                format!(".{}", path[0].name)
            } else {
                path.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(".")
            };
            match payload {
                None => base,
                Some(PatPayload::Tuple(ps)) => format!(
                    "{base}({})",
                    ps.iter().map(pattern_str).collect::<Vec<_>>().join(", ")
                ),
                Some(PatPayload::Record { fields, rest }) => {
                    let mut parts: Vec<String> = fields
                        .iter()
                        .map(|f| match &f.pattern {
                            Some(p) => format!("{}: {}", f.name.name, pattern_str(p)),
                            None => f.name.name.clone(),
                        })
                        .collect();
                    if *rest {
                        parts.push("..".into());
                    }
                    format!("{base} {{ {} }}", parts.join(", "))
                }
            }
        }
    }
}

/// Expressions are rendered on one line; the statement printer wraps when the
/// result does not fit.
pub fn expr(e: &Expr) -> String {
    let mut out = String::new();
    write_expr(&mut out, e, 0);
    out
}

/// The precedence ladder of SPEC 6.1, lowest to highest. Parentheses are
/// printed only where they change the parse — the source's own are not in the
/// tree, so re-adding all of them would grow the file every time it is
/// formatted.
fn binop_prec(op: BinOp) -> u8 {
    match op {
        BinOp::Or => 1,
        BinOp::Coalesce => 2,
        BinOp::And => 3,
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 4,
        BinOp::BitOr => 5,
        BinOp::BitXor => 6,
        BinOp::BitAnd => 7,
        BinOp::Add | BinOp::Sub => 8,
        BinOp::Mul | BinOp::Div | BinOp::Rem => 9,
    }
}

fn expr_prec(e: &Expr) -> u8 {
    match e {
        // A lambda and `crash` extend maximally to the right, so they are
        // never a bare operand (SPEC 12.11).
        Expr::Lambda { .. } | Expr::Crash { .. } => 0,
        Expr::Binary { op, .. } => binop_prec(*op),
        Expr::Unary { .. } => 10,
        _ => 11,
    }
}

fn write_expr(out: &mut String, e: &Expr, indent: usize) {
    match e {
        Expr::Int { raw, .. } | Expr::Float { raw, .. } => out.push_str(raw),
        Expr::Str { value, .. } => out.push_str(&quote(value)),
        Expr::Char { value, .. } => out.push_str(&quote_char(*value)),
        Expr::Bool { value, .. } => out.push_str(&value.to_string()),
        Expr::Unit { .. } => out.push_str("()"),
        Expr::Ident { name, .. } => out.push_str(name),
        Expr::SelfValue { .. } => out.push_str("self"),
        Expr::Ctx { .. } => out.push_str("ctx"),
        Expr::DotVariant { name, .. } => {
            let _ = write!(out, ".{}", name.name);
        }
        Expr::Template { parts, .. } => {
            out.push('"');
            for p in parts {
                match p {
                    TemplatePart::Text(t) => {
                        for c in t.chars() {
                            match c {
                                '"' => out.push_str("\\\""),
                                '\\' => out.push_str("\\\\"),
                                '\n' => out.push_str("\\n"),
                                '\t' => out.push_str("\\t"),
                                '$' => out.push_str("\\$"),
                                c => out.push(c),
                            }
                        }
                    }
                    TemplatePart::Hole(h) => {
                        out.push_str("${");
                        write_expr(out, h, indent);
                        out.push('}');
                    }
                }
            }
            out.push('"');
        }
        Expr::Array { elems, .. } => {
            out.push('[');
            for (i, x) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_expr(out, x, indent);
            }
            out.push(']');
        }
        Expr::Tuple { elems, .. } => {
            out.push('(');
            for (i, x) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_expr(out, x, indent);
            }
            out.push(')');
        }
        Expr::Block(b) => {
            if b.stmts.is_empty() {
                if let Some(t) = &b.tail {
                    out.push_str("{ ");
                    write_expr(out, t, indent);
                    out.push_str(" }");
                    return;
                }
            }
            out.push_str("{\n");
            let mut f = Fmt { out: String::new(), depth: indent + 1, comments: Vec::new() };
            f.block_inner(b);
            out.push_str(&f.out);
            out.push_str(&"  ".repeat(indent));
            out.push('}');
        }
        Expr::If { cond, then, else_, .. } => {
            out.push_str("if (");
            write_expr(out, cond, indent);
            out.push_str(") ");
            write_expr(out, &Expr::Block(then.clone()), indent);
            out.push_str(" else ");
            write_expr(out, else_, indent);
        }
        Expr::Match { scrutinee, arms, .. } => {
            out.push_str("match (");
            write_expr(out, scrutinee, indent);
            out.push_str(") {\n");
            for a in arms {
                out.push_str(&"  ".repeat(indent + 1));
                out.push_str(&pattern_str(&a.pattern));
                if let Some(g) = &a.guard {
                    out.push_str(" if ");
                    write_expr(out, g, indent + 1);
                }
                out.push_str(" => ");
                write_expr(out, &a.body, indent + 1);
                out.push_str(",\n");
            }
            out.push_str(&"  ".repeat(indent));
            out.push('}');
        }
        Expr::ContextExpr { body, .. } => {
            out.push_str("context {\n");
            let mut f = Fmt { out: String::new(), depth: indent + 1, comments: Vec::new() };
            f.context_body(body);
            out.push_str(&f.out);
            out.push_str(&"  ".repeat(indent));
            out.push('}');
        }
        Expr::Lambda { params, ret, body, .. } => {
            out.push_str("fn(");
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&p.name.name);
                if let Some(t) = &p.ty {
                    let _ = write!(out, ": {}", ty(t));
                }
            }
            out.push(')');
            if let Some(r) = ret {
                let _ = write!(out, ": {}", ty(r));
            }
            out.push_str(" => ");
            write_expr(out, body, indent);
        }
        Expr::Crash { message, .. } => {
            out.push_str("crash ");
            write_expr(out, message, indent);
        }
        Expr::Unary { op, operand, .. } => {
            out.push_str(op.text());
            write_expr(out, operand, indent);
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            let p = binop_prec(*op);
            write_at(out, lhs, indent, p);
            let _ = write!(out, " {} ", op.text());
            // Left-associative, and comparison is non-associative, so the
            // right operand binds one level tighter.
            write_at(out, rhs, indent, p + 1);
        }
        Expr::Field { base, name, .. } => {
            write_operand(out, base, indent);
            let _ = write!(out, ".{}", name.name);
        }
        Expr::TupleIndex { base, index, .. } => {
            // `t.0.1` lexes as `t` `.` `0.1`, so a nested tuple index keeps
            // its parentheses: `(t.0).1`. A known lexical wart, accepted
            // because it is what lets `pair.0` lex at all (grammar.ebnf).
            if matches!(&**base, Expr::TupleIndex { .. }) {
                out.push('(');
                write_expr(out, base, indent);
                out.push(')');
            } else {
                write_operand(out, base, indent);
            }
            let _ = write!(out, ".{index}");
        }
        Expr::Call { callee, args, .. } => {
            write_operand(out, callee, indent);
            out.push('(');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_expr(out, a, indent);
            }
            out.push(')');
        }
        Expr::Index { base, index, .. } => {
            write_operand(out, base, indent);
            out.push('[');
            write_expr(out, index, indent);
            out.push(']');
        }
        Expr::Try { base, .. } => {
            write_operand(out, base, indent);
            out.push('?');
        }
        Expr::TurboFish { base, args, .. } => {
            write_operand(out, base, indent);
            let _ = write!(out, "::<{}>", args.iter().map(ty).collect::<Vec<_>>().join(", "));
        }
        Expr::StructLit { head, spread, fields, .. } => {
            write_operand(out, head, indent);
            out.push_str(" {");
            let mut first = true;
            if let Some(s) = spread {
                out.push_str(" ..");
                write_expr(out, s, indent);
                first = false;
            }
            for f in fields {
                out.push_str(if first { " " } else { ", " });
                first = false;
                out.push_str(&f.name.name);
                if let Some(v) = &f.value {
                    out.push_str(": ");
                    write_expr(out, v, indent);
                }
            }
            out.push_str(" }");
        }
    }
}

/// Parenthesizes only where precedence requires it.
fn write_at(out: &mut String, e: &Expr, indent: usize, parent: u8) {
    if expr_prec(e) < parent {
        out.push('(');
        write_expr(out, e, indent);
        out.push(')');
    } else {
        write_expr(out, e, indent);
    }
}

/// The head of a postfix chain. A block-like expression may not head one
/// (SPEC 12.13), so it gets parentheses; so does anything that binds looser
/// than a postfix operator.
fn write_operand(out: &mut String, e: &Expr, indent: usize) {
    let needs = expr_prec(e) < 11
        || (e.is_block_like() && !matches!(e, Expr::Block(_)));
    if needs {
        out.push('(');
        write_expr(out, e, indent);
        out.push(')');
    } else {
        write_expr(out, e, indent);
    }
}

/// Reads the tokens of a file, so the formatter can tell whether it changed
/// anything meaningful.
pub fn token_shape(text: &str) -> Vec<Tok> {
    lex(text, FileId(0)).tokens.into_iter().map(|t| t.tok).collect()
}
