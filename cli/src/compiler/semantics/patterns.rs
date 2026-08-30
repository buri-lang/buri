//! Pattern checking.
//!
//! A bare identifier is always a binding. `None` as a pattern binds a variable
//! named `None`; it does not match the `None` variant. That is a real
//! ergonomic cost, and it is what removes name resolution from the parser —
//! `Foo` versus `Foo(x)` versus `Foo { .. }` is decided by the token after
//! `Foo`, never by what `Foo` means (SPEC 7.2).

use crate::compiler::semantics::inference::{Infer, LitCheck, OrScope};
use crate::compiler::semantics::resolve::Sym;
use crate::compiler::semantics::typed;
use crate::compiler::semantics::types::*;
use crate::diagnostics::{Invariant as _, Span};
use crate::parsing::flat::{Location, PatId, PatPayloadData, PatView as P};

impl<'a, 'b> Infer<'a, 'b> {
    pub(crate) fn check_pattern(&mut self, p: PatId, ty: &Ty) -> typed::Pattern {
        let ty = self.resolve(ty);
        let t = self.tree();
        let span = t.pspan(p);
        let kind = match t.pat(p) {
            P::Wild { .. } => typed::PatKind::Wild,

            P::Bind { name, name_span, sub, .. } => {
                // Each name a pattern binds is bound once. `(a, a)` is a
                // mistake, not a shorthand for "equal".
                if self.or_scope.is_none() && self.bound_already(name) {
                    let bound = name.to_string();
                    self.templated("duplicate-pattern-binding", name_span).bind("name", bound);
                }
                self.pattern_names.push(name.to_string());
                let local = self.shared_local(name, &ty, name_span);
                self.bind(name, local);
                self.note_capture_risk(local, &ty);
                let sub = sub.map(|s| Box::new(self.check_pattern(s, &ty)));
                typed::PatKind::Bind { local, sub }
            }

            P::LitInt { value, negative, raw, .. } => {
                let lit = self.subst.fresh_num(NumClass::Int, span);
                self.unify_at(span, &lit, &ty, "the scrutinee");
                self.lit_checks.push(LitCheck {
                    value,
                    negative,
                    raw: raw.to_string(),
                    ty: lit,
                    span,
                });
                typed::PatKind::Int(value, negative)
            }
            P::LitFloat { value, negative, .. } => {
                let lit = self.subst.fresh_num(NumClass::Float, span);
                self.unify_at(span, &lit, &ty, "the scrutinee");
                typed::PatKind::Float(if negative { -value } else { value })
            }
            P::LitStr { value, .. } => {
                let s = self.prim(Prim::Str);
                self.unify_at(span, &s, &ty, "the scrutinee");
                typed::PatKind::Str(value.to_string())
            }
            P::LitChar { value, .. } => {
                let c = self.prim(Prim::Char);
                self.unify_at(span, &c, &ty, "the scrutinee");
                typed::PatKind::Char(value)
            }
            P::LitBool { value, .. } => {
                let b = self.prim(Prim::Bool);
                self.unify_at(span, &b, &ty, "the scrutinee");
                typed::PatKind::Bool(value)
            }

            P::Unit { .. } => {
                self.unify_at(span, &Ty::Unit, &ty, "the scrutinee");
                typed::PatKind::Unit
            }

            P::Tuple { elems, .. } => {
                let elem_types = match &ty {
                    Ty::Tuple(ts) if ts.len() == elems.len() => ts.clone(),
                    Ty::Error => vec![Ty::Error; elems.len()],
                    other => {
                        let shown = self.show_ty(other);
                        let n = elems.len();
                        self.templated("pattern-not-a-tuple", span)
                            .bind("type", shown.clone())
                            .bind("arity", n.to_string())
                            .fix(format!("match the shape of `{shown}`, not a {n}-tuple"));
                        vec![Ty::Error; elems.len()]
                    }
                };
                typed::PatKind::Tuple(
                    elems
                        .iter()
                        .zip(&elem_types)
                        .map(|(e, t)| self.check_pattern(*e, t))
                        .collect(),
                )
            }

            P::Array { elems, rest, .. } => {
                let elem_ty = match &ty {
                    Ty::Array(e) => (**e).clone(),
                    Ty::Error => Ty::Error,
                    other => {
                        let shown = self.show_ty(other);
                        self.templated("pattern-not-an-array", span)
                            .bind("type", shown.clone())
                            .fix(format!("an array pattern matches `[T]`, not `{shown}`"));
                        Ty::Error
                    }
                };
                let checked: Vec<typed::Pattern> =
                    elems.iter().map(|e| self.check_pattern(*e, &elem_ty)).collect();
                let rest_local = match rest {
                    None => typed::ArrayRest::None,
                    Some(None) => typed::ArrayRest::Ignored,
                    Some(Some(n)) => {
                        let dup = t.text(n);
                        let dup_span = t.span_of(n);
                        if self.or_scope.is_none() && self.bound_already(dup) {
                            let bound = dup.to_string();
                            self.templated("duplicate-pattern-binding", dup_span)
                                .bind("name", bound);
                        }
                        self.pattern_names.push(dup.to_string());
                        let arr = Ty::Array(Box::new(elem_ty.clone()));
                        let l = self.shared_local(dup, &arr, dup_span);
                        self.bind(dup, l);
                        typed::ArrayRest::Bound(l)
                    }
                };
                typed::PatKind::Array { elems: checked, rest: rest_local }
            }

            P::Or { alts, .. } => {
                let mut out = Vec::new();
                // Entered and left as one value, so a nested or-pattern
                // restores everything the outer one had.
                let saved = self.or_scope.replace(OrScope::default());
                let mut names: Option<Vec<String>> = None;
                for alt in alts {
                    // Each alternative starts from the same set, so a name
                    // bound by the first is reused rather than redeclared.
                    if let Some(scope) = self.or_scope.as_mut() {
                        scope.current = scope.first.clone().unwrap_or_default();
                    }
                    let checked = self.check_pattern(*alt, &ty);
                    let bound = self
                        .or_scope
                        .as_ref()
                        .map(|s| s.current.clone())
                        .unwrap_or_default();
                    let mut these: Vec<String> = bound.keys().cloned().collect();
                    these.sort();
                    match &names {
                        None => {
                            names = Some(these);
                            if let Some(scope) = self.or_scope.as_mut() {
                                scope.first = Some(bound);
                            }
                        }
                        Some(first) if first != &these => {
                            let missing: Vec<&String> =
                                first.iter().filter(|n| !these.contains(n)).collect();
                            let extra: Vec<&String> =
                                these.iter().filter(|n| !first.contains(*n)).collect();
                            let mut note = String::new();
                            if !missing.is_empty() {
                                let missing: Vec<String> =
                                    missing.iter().map(|s| s.to_string()).collect();
                                note.push_str(&format!(
                                    "this alternative does not bind {}",
                                    crate::diagnostics::names(&missing)
                                ));
                            }
                            if !extra.is_empty() {
                                if !note.is_empty() {
                                    note.push_str("; ");
                                }
                                let extra: Vec<String> =
                                    extra.iter().map(|s| s.to_string()).collect();
                                note.push_str(&format!(
                                    "it binds {}, which the others do not",
                                    crate::diagnostics::names(&extra)
                                ));
                            }
                            let at = t.pspan(*alt);
                            self.templated("or-pattern-bindings", at).notes.push(note);
                            names = Some(these);
                        }
                        _ => {}
                    }
                    out.push(checked);
                }
                self.or_scope = saved;
                typed::PatKind::Or(out)
            }

            P::Path { path, dotted, payload, .. } => {
                self.check_path_pattern(path, dotted, payload, &ty, span)
            }
        };
        typed::Pattern { kind, ty, span }
    }

    /// Whether the pattern being checked has already bound this name.
    ///
    /// `pattern_names` holds `String`s and the name is now a slice of the
    /// source, so the comparison is on the text rather than on the container.
    fn bound_already(&self, name: &str) -> bool {
        self.pattern_names.iter().any(|n| n == name)
    }

    fn or_alternative_local(&self, name: &str) -> Option<LocalId> {
        self.or_scope.as_ref()?.current.get(name).copied()
    }

    fn record_or_binding(&mut self, name: &str, local: LocalId) {
        if let Some(scope) = self.or_scope.as_mut() {
            scope.current.insert(name.to_string(), local);
        }
    }

    /// The local this name binds to, shared across an or-pattern's
    /// alternatives.
    ///
    /// Or-alternatives bind the same names at the same types, so the later
    /// ones reuse the first's local instead of minting one of their own: the
    /// arm body reads a single slot, and lowering has one place to join them.
    /// Every binding form goes through here — a sub-pattern, a field
    /// shorthand, an array rest — because a form that mints its own local
    /// silently miscompiles rather than failing to compile.
    fn shared_local(&mut self, name: &str, ty: &Ty, span: Span) -> LocalId {
        match self.or_alternative_local(name) {
            Some(existing) => {
                let existing_ty = self.local_ty(existing);
                self.unify_at(span, ty, &existing_ty, "the other alternative");
                existing
            }
            None => {
                let local = self.new_local(name, ty.clone(), span);
                self.record_or_binding(name, local);
                local
            }
        }
    }

    fn check_path_pattern(
        &mut self,
        path: &[Location],
        dotted: bool,
        payload: Option<PatPayloadData>,
        ty: &Ty,
        span: Span,
    ) -> typed::PatKind {
        let t = self.tree();
        let [head, rest @ ..] = path else {
            crate::ice!("the parser builds every path pattern from at least one identifier")
        };
        let head = t.text(*head);
        // `.Variant` — the scrutinee's type supplies the enum.
        if dotted {
            let Ty::Con(con, args) = ty else {
                if !ty.is_error() {
                    let shown = self.show_ty(ty);
                    self.templated("not-an-enum", span)
                        .bind("type", shown)
                        .fix("a `.Variant` pattern matches an enum; match this value another way");
                }
                return typed::PatKind::Error;
            };
            let Some(index) = self.c.tables.variant_index(*con, head) else {
                self.report_no_variant(*con, head, span);
                return typed::PatKind::Error;
            };
            return self.variant_pattern(*con, index, args.clone(), payload, span);
        }

        // `Enum.Variant`, `mod.Enum.Variant`, `Struct { .. }`, `Tuple(x)`.
        let module = self.module;
        let resolved = match rest.first() {
            None => self.c.scope(module).names.get(head).cloned(),
            Some(second) => {
                match self.c.scope(module).namespaces.get(head).copied() {
                    Some(ns) => self.c.lookup_export(ns, t.text(*second)),
                    None => self.c.scope(module).names.get(head).cloned(),
                }
            }
        };

        match resolved {
            Some(Sym::Ty(con)) => {
                let variant_name = rest.last().map(|last| t.text(*last));
                let is_enum = matches!(self.c.tables.tycon(con).def, TyDef::Enum { .. });
                let args = match ty {
                    Ty::Con(c, a) if *c == con => a.clone(),
                    Ty::Error => vec![Ty::Error; self.c.tables.tycon(con).arity()],
                    other => {
                        let want = self.c.tables.tycon(con).name.clone();
                        let shown = self.show_ty(other);
                        self.templated("pattern-type-mismatch", span)
                            .bind("expected", shown.clone())
                            .bind("found", want.clone())
                            .mismatch(format!("`{shown}`"), format!("a `{want}` pattern"))
                            .fix(format!("match the shape of `{shown}`"));
                        vec![Ty::Error; self.c.tables.tycon(con).arity()]
                    }
                };
                if is_enum {
                    let Some(vname) = variant_name else {
                        let n = self.c.tables.tycon(con).name.clone();
                        self.templated("enum-without-a-variant", span)
                            .bind("type", n.clone())
                            .fix(format!("write `{n}.Variant` or `.Variant`"));
                        return typed::PatKind::Error;
                    };
                    let Some(index) = self.c.tables.variant_index(con, vname) else {
                        self.report_no_variant(con, vname, span);
                        return typed::PatKind::Error;
                    };
                    self.variant_pattern(con, index, args, payload, span)
                } else {
                    let fields = self.struct_field_patterns(con, &args, payload, span);
                    typed::PatKind::Struct { con, fields }
                }
            }
            _ => {
                let shown = path.iter().map(|i| t.text(*i)).collect::<Vec<_>>().join(".");
                self.templated("unresolved-type-in-pattern", span).bind("name", shown);
                typed::PatKind::Error
            }
        }
    }

    fn report_no_variant(&mut self, con: TyConId, name: &str, span: Span) {
        let ty = self.c.tables.tycon(con).name.clone();
        let note = no_variant_note(&self.c.tables, con, name);
        let d = self
            .templated("no-such-variant", span)
            .bind("type", ty)
            .bind("variant", name.to_string());
        d.notes.extend(note);
    }

    fn variant_pattern(
        &mut self,
        con: TyConId,
        index: usize,
        args: Vec<Ty>,
        payload: Option<PatPayloadData>,
        span: Span,
    ) -> typed::PatKind {
        let variant = self
            .c
            .tables
            .tycon(con)
            .variants()
            .get(index)
            .or_ice("the index is a position in this same variant list")
            .clone();
        // A variant is exported exactly when its enum is, so this fires only
        // where a private enum reached another module through a signature.
        let owner = self.c.tables.tycon(con).module;
        if owner != self.module && owner.0 != u32::MAX && !variant.exported {
            let t = self.c.tables.tycon(con).name.clone();
            let v = variant.name.clone();
            self.templated("private-to-module", span)
                .bind("declaration", format!("variant `{v}` of `{t}`"))
                .fix(format!("add `export` to `{t}`, or match through a function `{t}`'s module provides"));
        }
        let fields = self.payload_patterns(&variant.fields, &args, payload, span, &variant.name);
        typed::PatKind::Variant { con, variant: index, fields }
    }

    fn struct_field_patterns(
        &mut self,
        con: TyConId,
        args: &[Ty],
        payload: Option<PatPayloadData>,
        span: Span,
    ) -> Vec<typed::FieldPat> {
        let decl = self.c.tables.tycon(con).fields().to_vec();
        let name = self.c.tables.tycon(con).name.clone();
        // Check visibility once for the pattern as a whole: a struct with any
        // private field cannot be destructured outside its module.
        let owner = self.c.tables.tycon(con).module;
        if owner != self.module && owner.0 != u32::MAX {
            for f in &decl {
                if !f.exported {
                    let fname = f.name.clone();
                    self.templated("private-to-module", span)
                        .bind("declaration", format!("field `{fname}` of `{name}`"))
                        .fix(format!("add `export` to `{fname}`, or read it through a method"));
                    break;
                }
            }
        }
        self.payload_patterns(&decl, args, payload, span, &name)
    }

    fn payload_patterns(
        &mut self,
        decl: &[FieldInfo],
        args: &[Ty],
        payload: Option<PatPayloadData>,
        span: Span,
        what: &str,
    ) -> Vec<typed::FieldPat> {
        match payload {
            None => {
                if !decl.is_empty() {
                    self.templated("missing-payload-pattern", span)
                        .bind("name", what.to_string())
                        .fix(format!("write `.{what}(..)`, or name each field"));
                }
                Vec::new()
            }
            Some(p) if !p.record => {
                let ps = self.tree().pkids_at(p.start, p.len);
                if ps.len() != decl.len() {
                    let want = decl.len();
                    let have = ps.len();
                    self.templated("wrong-matched-value-count", span)
                        .bind("name", what.to_string())
                        .bind("expected", want.to_string())
                        .bind("matched", have.to_string())
                        .mismatch(want.to_string(), have.to_string());
                }
                // A payload with more patterns than the declaration has
                // fields is already reported above; the extra ones have no
                // field to check against, so `zip` stops at the shorter.
                ps.iter()
                    .zip(decl)
                    .enumerate()
                    .map(|(i, (p, d))| {
                        let ty = substitute(&d.ty, args, None);
                        typed::FieldPat { index: i, pattern: self.check_pattern(*p, &ty) }
                    })
                    .collect()
            }
            Some(p) => {
                let t = self.tree();
                let fields = t.fpats_at(p.start, p.len);
                let rest = p.rest;
                let mut out = Vec::new();
                let mut seen = Vec::new();
                for f in fields {
                    let fname = t.text(f.name);
                    let fspan = t.span_of(f.name);
                    let Some((i, d)) = decl.iter().enumerate().find(|(_, d)| d.name == fname)
                    else {
                        self.templated("no-such-field", fspan)
                            .bind("type", what.to_string())
                            .bind("field", fname.to_string());
                        continue;
                    };
                    seen.push(i);
                    let ty = substitute(&d.ty, args, None);
                    let pattern = match t.opt_pat(f.pattern) {
                        Some(p) => self.check_pattern(p, &ty),
                        // Field shorthand: `User { id, name }` binds both.
                        None => {
                            if self.or_scope.is_none() && self.bound_already(fname) {
                                let bound = fname.to_string();
                                self.templated("duplicate-pattern-binding", fspan)
                                    .bind("name", bound);
                            }
                            self.pattern_names.push(fname.to_string());
                            let local = self.shared_local(fname, &ty, fspan);
                            self.bind(fname, local);
                            typed::Pattern {
                                kind: typed::PatKind::Bind { local, sub: None },
                                ty,
                                span: fspan,
                            }
                        }
                    };
                    out.push(typed::FieldPat { index: i, pattern });
                }
                // Without a `..`, a struct pattern must mention every field.
                if !rest {
                    let missing: Vec<String> = decl
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| !seen.contains(i))
                        .map(|(_, d)| d.name.clone())
                        .collect();
                    if !missing.is_empty() {
                        let missing = crate::diagnostics::names(&missing);
                        self.templated("missing-field-pattern", span).bind("fields", missing);
                    }
                }
                out
            }
        }
    }
}

/// The words for `.Nmae` on an enum with no such variant: the message, and the
/// note that either suggests the near miss or lists what is there.
///
/// One function because a pattern and an expression are the same mistake and
/// deserve the same sentence. They had two spellings, and the two disagreed
/// about whether the variant names are quoted. The span stays the caller's,
/// because it is the only thing that genuinely differs.
pub(crate) fn no_variant_note(tables: &Tables, con: TyConId, name: &str) -> Option<String> {
    let variants: Vec<String> =
        tables.tycon(con).variants().iter().map(|v| v.name.clone()).collect();
    let refs: Vec<&str> = variants.iter().map(|s| s.as_str()).collect();
    match crate::build::buildfile::nearest(name, &refs) {
        Some(x) => Some(format!("did you mean `.{x}`?")),
        None if !variants.is_empty() => {
            Some(format!("its variants are {}", crate::diagnostics::names(&variants)))
        }
        None => None,
    }
}
