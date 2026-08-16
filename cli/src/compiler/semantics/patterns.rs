//! Pattern checking.
//!
//! A bare identifier is always a binding. `None` as a pattern binds a variable
//! named `None`; it does not match the `None` variant. That is a real
//! ergonomic cost, and it is what removes name resolution from the parser —
//! `Foo` versus `Foo(x)` versus `Foo { .. }` is decided by the token after
//! `Foo`, never by what `Foo` means (SPEC 7.2).

use crate::compiler::semantics::inference::{Infer, LitCheck};
use crate::compiler::semantics::resolve::Sym;
use crate::compiler::semantics::typed;
use crate::compiler::semantics::types::*;
use crate::diagnostics::Span;
use crate::parsing::tree;
use std::collections::HashMap;

impl<'a, 'b> Infer<'a, 'b> {
    pub(crate) fn check_pattern(&mut self, p: &tree::Pattern, ty: &Ty) -> typed::Pattern {
        let ty = self.resolve(ty);
        let span = p.span();
        let kind = match p {
            tree::Pattern::Wild { .. } => typed::PatKind::Wild,

            tree::Pattern::Bind { name, sub, .. } => {
                // Each name a pattern binds is bound once. `(a, a)` is a
                // mistake, not a shorthand for "equal".
                if self.or_bindings.is_none() && self.pattern_names.contains(&name.name) {
                    let n = name.name.clone();
                    self.err(name.span, format!("`{n}` is bound twice in this pattern")).code("duplicate-bound")
                        .fix("rename one of them, or bind one and compare the other in a guard")
                        .note("a name a pattern binds is bound once; to require two positions to be equal, match one and test the other in a guard");
                }
                self.pattern_names.push(name.name.clone());
                let local = match self.or_alternative_local(&name.name) {
                    // Or-pattern alternatives must bind identical names at
                    // identical types, so the second alternative reuses the
                    // first's binding rather than shadowing it.
                    Some(existing) => {
                        let existing_ty = self.local_ty(existing);
                        self.unify_at(name.span, &ty, &existing_ty, "the other alternative");
                        existing
                    }
                    None => {
                        let l = self.new_local(&name.name, ty.clone(), name.span);
                        self.record_or_binding(&name.name, l);
                        l
                    }
                };
                self.bind(&name.name, local);
                self.note_capture_risk(local, &ty);
                let sub = sub.as_ref().map(|s| Box::new(self.check_pattern(s, &ty)));
                typed::PatKind::Bind { local, sub }
            }

            tree::Pattern::LitInt { value, negative, raw, .. } => {
                let lit = self.subst.fresh_num(NumClass::Int, span);
                self.unify_at(span, &lit, &ty, "the scrutinee");
                self.lit_checks.push(LitCheck {
                    value: *value,
                    negative: *negative,
                    raw: raw.clone(),
                    ty: lit,
                    span,
                });
                typed::PatKind::Int(*value, *negative)
            }
            tree::Pattern::LitFloat { value, negative, .. } => {
                let lit = self.subst.fresh_num(NumClass::Float, span);
                self.unify_at(span, &lit, &ty, "the scrutinee");
                typed::PatKind::Float(if *negative { -*value } else { *value })
            }
            tree::Pattern::LitStr { value, .. } => {
                let s = self.prim(Prim::Str);
                self.unify_at(span, &s, &ty, "the scrutinee");
                typed::PatKind::Str(value.clone())
            }
            tree::Pattern::LitChar { value, .. } => {
                let c = self.prim(Prim::Char);
                self.unify_at(span, &c, &ty, "the scrutinee");
                typed::PatKind::Char(*value)
            }
            tree::Pattern::LitBool { value, .. } => {
                let b = self.prim(Prim::Bool);
                self.unify_at(span, &b, &ty, "the scrutinee");
                typed::PatKind::Bool(*value)
            }

            tree::Pattern::Unit { .. } => {
                self.unify_at(span, &Ty::Unit, &ty, "the scrutinee");
                typed::PatKind::Unit
            }

            tree::Pattern::Tuple { elems, .. } => {
                let elem_types = match &ty {
                    Ty::Tuple(ts) if ts.len() == elems.len() => ts.clone(),
                    Ty::Error => vec![Ty::Error; elems.len()],
                    other => {
                        let shown = self.show_ty(other);
                        let n = elems.len();
                        self.err(span, format!("`{shown}` is not a {n}-tuple"))
                            .fix(format!("match the shape of `{shown}`, not a {n}-tuple"));
                        vec![Ty::Error; elems.len()]
                    }
                };
                typed::PatKind::Tuple(
                    elems
                        .iter()
                        .zip(&elem_types)
                        .map(|(e, t)| self.check_pattern(e, t))
                        .collect(),
                )
            }

            tree::Pattern::Array { elems, rest, .. } => {
                let elem_ty = match &ty {
                    Ty::Array(e) => (**e).clone(),
                    Ty::Error => Ty::Error,
                    other => {
                        let shown = self.show_ty(other);
                        self.err(span, format!("`{shown}` is not an array"))
                            .fix(format!("an array pattern matches `[T]`, not `{shown}`"));
                        Ty::Error
                    }
                };
                let checked: Vec<typed::Pattern> =
                    elems.iter().map(|e| self.check_pattern(e, &elem_ty)).collect();
                let rest_local = rest.as_ref().map(|name| {
                    name.as_ref().map(|n| {
                        if self.or_bindings.is_none() && self.pattern_names.contains(&n.name) {
                            let dup = n.name.clone();
                            self.err(n.span, format!("`{dup}` is bound twice in this pattern")).code("duplicate-bound")
                                .fix("rename one of them");
                        }
                        self.pattern_names.push(n.name.clone());
                        let arr = Ty::Array(Box::new(elem_ty.clone()));
                        let l = self.new_local(&n.name, arr, n.span);
                        self.bind(&n.name, l);
                        l
                    })
                });
                typed::PatKind::Array { elems: checked, rest: rest_local }
            }

            tree::Pattern::Or { alts, .. } => {
                let mut out = Vec::new();
                let saved = self.or_bindings.take();
                self.or_bindings = Some(HashMap::new());
                let mut names: Option<Vec<String>> = None;
                for alt in alts {
                    self.or_bindings.as_mut().unwrap().clear();
                    // Each alternative starts from the same set, so a name
                    // bound by the first is reused rather than redeclared.
                    if let Some(prev) = &self.or_first {
                        let prev = prev.clone();
                        *self.or_bindings.as_mut().unwrap() = prev;
                    }
                    let checked = self.check_pattern(alt, &ty);
                    let bound = self.or_bindings.as_ref().unwrap().clone();
                    let mut these: Vec<String> = bound.keys().cloned().collect();
                    these.sort();
                    match &names {
                        None => {
                            names = Some(these);
                            self.or_first = Some(bound);
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
                            self.err(
                                alt.span(),
                                "or-pattern alternatives must bind the same names",
                            ).code("or-pattern-bindings")
                            .fix(
                                "bind the same names in every alternative, or split this into \
                                 separate arms",
                            )
                            .notes
                            .push(note);
                            names = Some(these);
                        }
                        _ => {}
                    }
                    out.push(checked);
                }
                self.or_first = None;
                self.or_bindings = saved;
                typed::PatKind::Or(out)
            }

            tree::Pattern::Path { path, dotted, payload, .. } => {
                self.check_path_pattern(path, *dotted, payload.as_ref(), &ty, span)
            }
        };
        typed::Pattern { kind, ty, span }
    }

    fn or_alternative_local(&self, name: &str) -> Option<LocalId> {
        self.or_bindings.as_ref()?.get(name).copied()
    }

    fn record_or_binding(&mut self, name: &str, local: LocalId) {
        if let Some(map) = self.or_bindings.as_mut() {
            map.insert(name.to_string(), local);
        }
    }

    fn check_path_pattern(
        &mut self,
        path: &[tree::Ident],
        dotted: bool,
        payload: Option<&tree::PatPayload>,
        ty: &Ty,
        span: Span,
    ) -> typed::PatKind {
        // `.Variant` — the scrutinee's type supplies the enum.
        if dotted {
            let Ty::Con(con, args) = ty else {
                if !ty.is_error() {
                    let shown = self.show_ty(ty);
                    self.err(span, format!("`{shown}` is not an enum"))
                        .fix("a `.Variant` pattern matches an enum; match this value another way");
                }
                return typed::PatKind::Error;
            };
            let name = &path[0].name;
            let Some(index) = self.c.tables.tycon(*con).variant_index(name) else {
                self.report_no_variant(*con, name, span);
                return typed::PatKind::Error;
            };
            return self.variant_pattern(*con, index, args.clone(), payload, span);
        }

        // `Enum.Variant`, `mod.Enum.Variant`, `Struct { .. }`, `Tuple(x)`.
        let resolved = if path.len() == 1 {
            self.c.scopes[self.module.index()].names.get(&path[0].name).cloned()
        } else {
            let head = &path[0].name;
            if let Some(ns) = self.c.scopes[self.module.index()].namespaces.get(head).copied() {
                self.c.lookup_export_pub(ns, &path[1].name)
            } else {
                self.c.scopes[self.module.index()].names.get(head).cloned()
            }
        };

        match resolved {
            Some(Sym::Ty(con)) => {
                let variant_name = if path.len() > 1 { Some(&path[path.len() - 1].name) } else { None };
                let is_enum = matches!(self.c.tables.tycon(con).def, TyDef::Enum { .. });
                let args = match ty {
                    Ty::Con(c, a) if *c == con => a.clone(),
                    Ty::Error => vec![Ty::Error; self.c.tables.tycon(con).arity()],
                    other => {
                        let want = self.c.tables.tycon(con).name.clone();
                        let shown = self.show_ty(other);
                        self.err(span, format!("expected `{shown}`, found a `{want}` pattern"))
                            .mismatch(format!("`{shown}`"), format!("a `{want}` pattern"))
                            .fix(format!("match the shape of `{shown}`"));
                        vec![Ty::Error; self.c.tables.tycon(con).arity()]
                    }
                };
                if is_enum {
                    let Some(vname) = variant_name else {
                        let n = self.c.tables.tycon(con).name.clone();
                        self.err(span, format!("`{n}` is an enum; name a variant"))
                            .fix(format!("write `{n}.Variant` or `.Variant`"));
                        return typed::PatKind::Error;
                    };
                    let Some(index) = self.c.tables.tycon(con).variant_index(vname) else {
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
                let shown = path.iter().map(|i| i.name.clone()).collect::<Vec<_>>().join(".");
                self.err(span, format!("there is no type `{shown}`")).code("unresolved-type")
                    .fix("write `.Variant` for a variant, or a lowerCamelCase name to bind the value")
                    .notes
                    .push(
                        "a bare identifier in a pattern is always a binding; a variant is \
                         written `.Variant` or `Enum.Variant`"
                            .into(),
                    );
                typed::PatKind::Error
            }
        }
    }

    fn report_no_variant(&mut self, con: TyConId, name: &str, span: Span) {
        let ty = self.c.tables.tycon(con).name.clone();
        let variants: Vec<String> =
            self.c.tables.tycon(con).variants().iter().map(|v| v.name.clone()).collect();
        let refs: Vec<&str> = variants.iter().map(|s| s.as_str()).collect();
        let near = crate::build::buildfile::nearest(name, &refs).map(|s| s.to_string());
        let d = self.err(span, format!("`{ty}` has no variant `{name}`"));
        d.fix("name a variant the enum declares");
        match near {
            Some(x) => d.notes.push(format!("did you mean `.{x}`?")),
            None if !variants.is_empty() => {
                d.notes.push(format!("its variants are {}", variants.join(", ")))
            }
            None => {}
        }
    }

    fn variant_pattern(
        &mut self,
        con: TyConId,
        index: usize,
        args: Vec<Ty>,
        payload: Option<&tree::PatPayload>,
        span: Span,
    ) -> typed::PatKind {
        let variant = self.c.tables.tycon(con).variants()[index].clone();
        // A type with any unexported variant cannot be matched outside its
        // module.
        let owner = self.c.tables.tycon(con).module;
        if owner != self.module && owner.0 != u32::MAX && !variant.exported {
            let t = self.c.tables.tycon(con).name.clone();
            let v = variant.name.clone();
            self.err(span, format!("variant `{v}` of `{t}` is private to its module"))
                .fix(format!("add `export` to `{v}`, or match through a function `{t}`'s module provides"));
        }
        let fields = self.payload_patterns(&variant.fields, &args, payload, span, &variant.name);
        typed::PatKind::Variant { con, variant: index, fields }
    }

    fn struct_field_patterns(
        &mut self,
        con: TyConId,
        args: &[Ty],
        payload: Option<&tree::PatPayload>,
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
                    self.err(span, format!("field `{fname}` of `{name}` is private to its module"))
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
        payload: Option<&tree::PatPayload>,
        span: Span,
        what: &str,
    ) -> Vec<typed::FieldPat> {
        match payload {
            None => {
                if !decl.is_empty() {
                    self.err(span, format!("`{what}` has a payload, so the pattern needs one"))
                        .fix(format!("write `.{what}(..)`, or name each field"));
                }
                Vec::new()
            }
            Some(tree::PatPayload::Tuple(ps)) => {
                if ps.len() != decl.len() {
                    let want = decl.len();
                    let have = ps.len();
                    self.err(
                        span,
                        format!("`{what}` holds {want} values, but {have} were matched"),
                    )
                    .mismatch(want.to_string(), have.to_string())
                    .fix(format!("match exactly {want}, or end the pattern with `..`"));
                }
                ps.iter()
                    .enumerate()
                    .filter(|(i, _)| *i < decl.len())
                    .map(|(i, p)| {
                        let ty = substitute(&decl[i].ty, args, None);
                        typed::FieldPat { index: i, pattern: self.check_pattern(p, &ty) }
                    })
                    .collect()
            }
            Some(tree::PatPayload::Record { fields, rest }) => {
                let mut out = Vec::new();
                let mut seen = Vec::new();
                for f in fields {
                    let Some(i) = decl.iter().position(|d| d.name == f.name.name) else {
                        let n = f.name.name.clone();
                        self.err(f.name.span, format!("`{what}` has no field `{n}`"))
                            .fix("check the spelling, or name a field it declares");
                        continue;
                    };
                    seen.push(i);
                    let ty = substitute(&decl[i].ty, args, None);
                    let pattern = match &f.pattern {
                        Some(p) => self.check_pattern(p, &ty),
                        // Field shorthand: `User { id, name }` binds both.
                        None => {
                            if self.or_bindings.is_none()
                                && self.pattern_names.contains(&f.name.name)
                            {
                                let n = f.name.name.clone();
                                self.err(f.name.span, format!("`{n}` is bound twice in this pattern")).code("duplicate-bound")
                                    .fix("rename one of them");
                            }
                            self.pattern_names.push(f.name.name.clone());
                            let local = self.new_local(&f.name.name, ty.clone(), f.name.span);
                            self.bind(&f.name.name, local);
                            self.record_or_binding(&f.name.name, local);
                            typed::Pattern {
                                kind: typed::PatKind::Bind { local, sub: None },
                                ty,
                                span: f.name.span,
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
                        self.err(span, format!("this pattern does not mention {missing}")).code("missing-field-pattern")
                            .fix(format!("match {missing} too, or end the pattern with `..` to ignore the rest"));
                    }
                }
                out
            }
        }
    }
}
