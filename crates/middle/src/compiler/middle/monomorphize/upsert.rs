//! A value read out of a map with `withDefault`, changed, and written back
//! under the same key, as one descent that changes the value in place.
//!
//! ```text
//! let waiting = net.links.get(link).withDefault(queue.empty());
//! Network { ..net, links: net.links.insert(ctx, link, waiting.push(ctx, e)) }
//! ```
//!
//! `waiting` is a second holder of the queue the map still holds, so the push
//! copies it, and the insert descends a second time to put the copy back. So
//! the insert becomes a call to the map module's `upserted`, which takes the
//! value out of its slot, hands it to the change, and stores what comes back,
//! in one descent:
//!
//! ```text
//! let fallback = queue.empty();
//! Network { ..net, links: upserted(ctx, net.links, link, fallback, e,
//!     fn(c, waiting, e2) => waiting.push(c, e2)) }
//! ```
//!
//! The change is a lambda that captures nothing: the context comes in as an
//! argument, and so do the other locals it reads, packed into a tuple when
//! there are two or more. Tuples are passed flat, so making it allocates
//! nothing.
//!
//! Two more shapes become one descent. The read can sit inside the value the
//! insert stores, `m.insert(ctx, k, m.get(k).withDefault(d).push(ctx, e))`,
//! when `d` is a literal or a name. And a value worked out from `m.get(k)`
//! some other way, read only by the insert right after it, becomes a call to
//! the map's `remade`, whose change gets what the descent found:
//!
//! ```text
//! let version = store.get(key).map(fn(r) => r.version + 1).withDefault(1);
//! store.insert(ctx, key, Record { value, version })
//! // becomes
//! remade(ctx, store, key, value, fn(c, found, value2) => {
//!     let version = found.map(fn(r) => r.version + 1).withDefault(1);
//!     Record { value: value2, version }
//! })
//! ```
//!
//! **Why it is correct.** The map and the key are each a name or a field of
//! one, so both reads name the same values, and `get` and `withDefault` have
//! no effect but their answer. The default is still evaluated where the `let`
//! was, or is a literal or a name. Nothing runs between the read and the
//! insert but the insert's map, context and key. The value is read nowhere but in the insert's value, which now runs
//! inside the descent: the only early exit, `?`, can't be in it, and nothing
//! the descent does is visible to it. So the stored value, the map and every
//! other value are what they were, and so is the key the map keeps.
//!
//! **When it runs.** On both stdlib maps, before instantiation, so the call is
//! instantiated like any other, on both backends and under `--coverage`. The
//! standard library is left alone: each instance of a stdlib function must
//! show every branch, and an `upserted` instance for a key type that never
//! collides, such as `Int` in `groupBy`, has a collision arm no input reaches.

use super::Monomorphizer;
use crate::compiler::semantics::resolve::{Checked, Sym};
use crate::compiler::semantics::typed::{self, Expr, ExprKind, PatKind, Stmt};
use crate::compiler::semantics::types::*;
use crate::diagnostics::{Invariant as _, Span};
use crate::hash::Set;

/// Each stdlib map module and the type it declares.
const MAPS: [(&str, &str); 2] = [("core/orderedmap", "OrderedMap"), ("core/map", "Map")];

/// The functions the rewrite reads and writes, of the maps this compilation
/// loaded.
#[derive(Default)]
pub(super) struct Upserts {
    with_default: Option<FnId>,
    maps: Vec<MapFns>,
}

/// One map's `get` and `insert`, and the `upserted` and `remade` that stand
/// for both.
struct MapFns {
    get: FnId,
    insert: FnId,
    upserted: FnId,
    remade: FnId,
}

impl Upserts {
    pub(super) fn find(checked: &Checked, module_paths: &[String]) -> Upserts {
        let maps = MAPS
            .iter()
            .filter_map(|(path, ty)| {
                let (_, scope) = checked
                    .scopes
                    .entries()
                    .find(|(id, _)| module_paths.get(id.index()).is_some_and(|p| p == path))?;
                // A loaded map module declares its type, `upserted` and
                // `remade`.
                let (mut con, mut upserted, mut remade) = (None, None, None);
                for (name, sym) in &scope.own {
                    match sym {
                        Sym::Ty(c) if name == ty => con = Some(*c),
                        Sym::Fn(f) if name == "upserted" => upserted = Some(*f),
                        Sym::Fn(f) if name == "remade" => remade = Some(*f),
                        _ => {}
                    }
                }
                let con = con.or_ice("a map module declares its type");
                let method = |name| checked.tables.method(con, name).or_ice("a map method");
                Some(MapFns {
                    get: method("get"),
                    insert: method("insert"),
                    upserted: upserted.or_ice("a map module declares `upserted`"),
                    remade: remade.or_ice("a map module declares `remade`"),
                })
            })
            .collect();
        Upserts {
            with_default: checked.tables.method(TyConId::OPTION, "withDefault"),
            maps,
        }
    }
}

/// A `let` of `m.get(k).withDefault(d)`, taken apart.
struct Read<'a> {
    value: LocalId,
    map: &'a Expr,
    key: &'a Expr,
    fns: &'a MapFns,
}

impl Monomorphizer<'_> {
    /// Rewrites each `let` in the block that reads a map with `withDefault`
    /// and whose value only the block's write back to the same key reads.
    pub(super) fn upsert_block(
        &mut self,
        stmts: &mut Vec<Stmt>,
        tail: &mut Option<Box<Expr>>,
        targs: &[Ty],
    ) {
        if !self.upsert_here {
            return;
        }
        let upserts = std::mem::take(&mut self.upserts);
        let mut dropped = Vec::new();
        for i in 0..stmts.len() {
            let (made, after) = stmts
                .split_at_mut(i)
                .1
                .split_first_mut()
                .or_ice("`i` is a statement's index");
            let Stmt::Let { pattern, value, .. } = &mut *made else {
                continue;
            };
            let Some(read) = read(&upserts, &pattern.kind, value) else {
                let w = match pattern.kind {
                    PatKind::Bind { local, sub: None } => local,
                    _ => continue,
                };
                let mut rest = after
                    .iter_mut()
                    .map(stmt_expr_mut)
                    .chain(tail.as_deref_mut());
                let root = rest.next();
                // Counted only for an insert: most `let`s aren't one.
                let later = || rest.map(|e| uses(e, w)).sum::<usize>();
                let made = (&*pattern, w, &*value);
                if root.is_some_and(|root| self.remake(&upserts, made, root, later, targs)) {
                    dropped.push(i);
                }
                continue;
            };
            let total = after
                .iter()
                .map(stmt_expr)
                .chain(tail.as_deref())
                .map(|e| uses(e, read.value))
                .sum::<usize>();
            let root = after
                .iter_mut()
                .map(stmt_expr_mut)
                .chain(tail.as_deref_mut())
                .find(|e| uses(e, read.value) == total);
            let Some(fallback) = root.and_then(|e| self.write_in(&read, e, total, targs)) else {
                continue;
            };
            // The default stays where the `let` evaluated it, bound to the
            // local the call reads it from.
            let default = std::mem::replace(value, Expr::new(ExprKind::Unit, Ty::UNIT, value.span));
            *value = into_default(default);
            pattern.kind = PatKind::Bind {
                local: fallback,
                sub: None,
            };
        }
        self.upserts = upserts;
        let mut at = (0..stmts.len()).map(|i| !dropped.contains(&i));
        stmts.retain(|_| at.next().unwrap_or(true));
    }

    /// Finds the insert that holds every one of the `total` reads of the
    /// value, under `e` and outside any lambda, and rewrites it. Answers the
    /// local the new call reads the default from.
    fn write_in(
        &mut self,
        read: &Read,
        e: &mut Expr,
        total: usize,
        targs: &[Ty],
    ) -> Option<LocalId> {
        match std::mem::replace(&mut e.kind, ExprKind::Error) {
            ExprKind::CallFn { func, args } if is_write(read, &func, &args) => {
                match self.upsert(read, func, args, e.span, targs, None) {
                    Ok((kind, fallback)) => {
                        e.kind = kind;
                        fallback
                    }
                    Err((func, args)) => {
                        e.kind = ExprKind::CallFn { func, args };
                        None
                    }
                }
            }
            kind @ ExprKind::Lambda { .. } => {
                e.kind = kind;
                None
            }
            kind => {
                e.kind = kind;
                let mut holder = None;
                typed::children_mut(e, &mut |c| {
                    if holder.is_none() && uses(c, read.value) == total {
                        holder = Some(c);
                    }
                });
                self.write_in(read, holder?, total, targs)
            }
        }
    }

    /// `m.insert(ctx, k, <a value that reads m.get(k).withDefault(d)>)` as a
    /// call to `upserted`, when `d` reads the same wherever it is evaluated:
    /// it moves in front of the rest of the value, which runs in the descent.
    pub(super) fn upsert_inline(
        &mut self,
        func: &typed::Callee,
        args: &[Expr],
        span: Span,
        targs: &[Ty],
    ) -> Option<ExprKind> {
        if !self.upsert_here {
            return None;
        }
        let upserts = std::mem::take(&mut self.upserts);
        let call = self.upsert_inline_with(&upserts, func, args, span, targs);
        self.upserts = upserts;
        call
    }

    fn upsert_inline_with(
        &mut self,
        upserts: &Upserts,
        func: &typed::Callee,
        args: &[Expr],
        span: Span,
        targs: &[Ty],
    ) -> Option<ExprKind> {
        let fns = upserts.maps.iter().find(|m| m.insert == func.id)?;
        let [map, ctx, key, value] = <&[Expr; 4]>::try_from(args)
            .ok()
            .or_ice("an insert takes four arguments");
        let is_read = |e: &Expr| inline_default(upserts, fns, map, key, e).is_some();
        if !(stable(map) && stable(key) && holds(value, &is_read)) {
            return None;
        }
        let mut value = value.clone();
        let at = find_mut(&mut value, &is_read).or_ice("`holds` found the read");
        let found = self.new_local("__found", self.sub(&at.ty, targs), at.span);
        let read = std::mem::replace(at, Expr::new(ExprKind::Local(found), at.ty, at.span));
        let default = inline_default(upserts, fns, map, key, &read)
            .or_ice("`find_mut` stopped at the read")
            .clone();
        let read = Read {
            value: found,
            map,
            key,
            fns,
        };
        let args = vec![map.clone(), ctx.clone(), key.clone(), value];
        let (call, _) = self
            .upsert(&read, func.clone(), args, span, targs, Some(default))
            .ok()?;
        Some(call)
    }

    /// The insert as a call to `upserted`, and the local that call reads the
    /// default from, when `default` doesn't hand the call one itself; or the
    /// insert as it was, when its value can't become a lambda.
    fn upsert(
        &mut self,
        read: &Read,
        func: typed::Callee,
        args: Vec<Expr>,
        span: Span,
        targs: &[Ty],
        default: Option<Expr>,
    ) -> Result<(ExprKind, Option<LocalId>), (typed::Callee, Vec<Expr>)> {
        let mut args = args.into_iter();
        let mut next = || args.next().or_ice("`is_write` saw four arguments");
        let (map, ctx, key, body) = (next(), next(), next(), next());
        let Some(captured) = captured(&body, read.value, local(&ctx)) else {
            return Err((func, vec![map, ctx, key, body]));
        };
        let value_ty = body.ty;
        let (fallback, fallback_local) = match default {
            Some(d) => (d, None),
            None => {
                let l = self.new_local("__fallback", self.sub(&value_ty, targs), span);
                (Expr::new(ExprKind::Local(l), value_ty, span), Some(l))
            }
        };
        let (arg, change) = self.change(&ctx, read.value, value_ty, body, &captured, span, targs);
        let mut call_targs = func.targs;
        call_targs.push(arg.ty);
        let call = ExprKind::CallFn {
            func: typed::Callee {
                id: read.fns.upserted,
                targs: call_targs,
            },
            args: vec![ctx, map, key, fallback, arg, change],
        };
        Ok((call, fallback_local))
    }

    /// Rewrites `let w = <reads m.get(k)>;` and the insert right after it,
    /// `m.insert(ctx, k, <reads w>)`, into one call to the map's `remade`,
    /// whose change works `w` out of what the descent found. Answers whether
    /// it did, which leaves the `let` for the caller to drop.
    ///
    /// Nothing is evaluated between the two but the insert's map, context and
    /// key, which are names or fields of them, so working `w` out inside the
    /// descent answers the same.
    fn remake(
        &mut self,
        upserts: &Upserts,
        (pattern, w, value): (&typed::Pattern, LocalId, &Expr),
        root: &mut Expr,
        later: impl FnOnce() -> usize,
        targs: &[Ty],
    ) -> bool {
        let ExprKind::CallFn { func, args } = &root.kind else {
            return false;
        };
        let Some(fns) = upserts.maps.iter().find(|m| m.insert == func.id) else {
            return false;
        };
        let [map, ctx, key, body] = <&[Expr; 4]>::try_from(args.as_slice())
            .ok()
            .or_ice("an insert takes four arguments");
        let is_get = |e: &Expr| matches!(call(e), Some((g, [m, k])) if g == fns.get && same(m, map) && same(k, key));
        let reads = uses(body, w);
        if !(stable(map)
            && stable(key)
            && reads > 0
            && uses(root, w).saturating_add(later()) == reads)
            || !holds(value, &is_get)
        {
            return false;
        }
        let mut worked = value.clone();
        let at = find_mut(&mut worked, &is_get).or_ice("`holds` found the read");
        let found_ty = at.ty;
        let found = self.new_local("__found", self.sub(&found_ty, targs), at.span);
        *at = Expr::new(ExprKind::Local(found), found_ty, at.span);
        let span = root.span;
        let value_ty = body.ty;
        let body = Expr::new(
            ExprKind::Block {
                stmts: vec![Stmt::Let {
                    pattern: pattern.clone(),
                    value: worked,
                    span,
                }],
                tail: Some(Box::new(body.clone())),
            },
            value_ty,
            span,
        );
        let Some(captured) = captured(&body, found, local(ctx)) else {
            return false;
        };
        let (map, ctx, key, remade, mut call_targs) = (
            map.clone(),
            ctx.clone(),
            key.clone(),
            fns.remade,
            func.targs.clone(),
        );
        let (arg, change) = self.change(&ctx, found, found_ty, body, &captured, span, targs);
        call_targs.push(arg.ty);
        root.kind = ExprKind::CallFn {
            func: typed::Callee {
                id: remade,
                targs: call_targs,
            },
            args: vec![ctx, map, key, arg, change],
        };
        true
    }

    /// `body` as a lambda of the context, `param` and one argument that
    /// carries the `captured` locals, and that argument: the local itself
    /// when there is one, a tuple of them when there are more. The lambda
    /// captures nothing, so making it allocates nothing.
    #[allow(clippy::too_many_arguments)]
    fn change(
        &mut self,
        ctx: &Expr,
        param: LocalId,
        param_ty: Ty,
        mut body: Expr,
        captured: &[LocalId],
        span: Span,
        targs: &[Ty],
    ) -> (Expr, Expr) {
        let ctx_local = local(ctx);
        let changed_ctx = self.new_local("__ctx", self.sub(&ctx.ty, targs), span);
        let tys: Vec<Ty> = captured.iter().map(|&x| self.locals.get(x).ty).collect();
        let (arg, arg_ty) = match (captured, tys.as_slice()) {
            ([x], [ty]) => (ExprKind::Local(*x), *ty),
            _ if captured.is_empty() => (ExprKind::Unit, Ty::UNIT),
            _ => {
                let parts = captured.iter().zip(&tys);
                let parts = parts.map(|(x, ty)| Expr::new(ExprKind::Local(*x), *ty, span));
                (
                    ExprKind::Tuple(parts.collect()),
                    Ty::tuple(tys.iter().copied()),
                )
            }
        };
        let changed_arg = self.new_local("__arg", arg_ty, span);
        // Inside, each captured local is the argument, or one of the names
        // the tuple is taken apart into.
        let inner: Vec<LocalId> = match captured {
            [_] => vec![changed_arg],
            _ => tys
                .iter()
                .map(|ty| self.new_local("__captured", *ty, span))
                .collect(),
        };
        rename(&mut body, &|l| {
            if Some(l) == ctx_local {
                changed_ctx
            } else {
                let mut pairs = captured.iter().zip(&inner);
                pairs.find(|(c, _)| **c == l).map_or(l, |(_, x)| *x)
            }
        });
        if captured.len() > 1 {
            let parts = inner.iter().zip(&tys).map(|(l, ty)| typed::Pattern {
                kind: PatKind::Bind {
                    local: *l,
                    sub: None,
                },
                ty: *ty,
                span,
            });
            let apart = Stmt::Let {
                pattern: typed::Pattern {
                    kind: PatKind::Tuple(parts.collect()),
                    ty: arg_ty,
                    span,
                },
                value: Expr::new(ExprKind::Local(changed_arg), arg_ty, span),
                span,
            };
            let ty = body.ty;
            let block = ExprKind::Block {
                stmts: vec![apart],
                tail: Some(Box::new(body)),
            };
            body = Expr::new(block, ty, span);
        }
        let ty = Ty::func([ctx.ty, param_ty, arg_ty], body.ty);
        let lambda = ExprKind::Lambda {
            params: vec![changed_ctx, param, changed_arg],
            body: Box::new(body),
            captures: Vec::new(),
        };
        (Expr::new(arg, arg_ty, span), Expr::new(lambda, ty, span))
    }
}

/// `pattern = value` as a name bound to `m.get(k).withDefault(d)` on a stdlib
/// map.
fn read<'a>(upserts: &'a Upserts, pattern: &PatKind, value: &'a Expr) -> Option<Read<'a>> {
    let PatKind::Bind { local, sub: None } = *pattern else {
        return None;
    };
    let (outer, [got, _]) = call(value)? else {
        return None;
    };
    let (get, [map, key]) = call(got)? else {
        return None;
    };
    let fns = upserts.maps.iter().find(|m| m.get == get)?;
    (Some(outer) == upserts.with_default && stable(map) && stable(key)).then_some(Read {
        value: local,
        map,
        key,
        fns,
    })
}

/// The default of `e` when it is `m.get(k).withDefault(d)` on the map `fns`
/// belongs to, and `d` reads the same wherever it is evaluated.
fn inline_default<'a>(
    upserts: &Upserts,
    fns: &MapFns,
    map: &Expr,
    key: &Expr,
    e: &'a Expr,
) -> Option<&'a Expr> {
    let (outer, [got, default]) = call(e)? else {
        return None;
    };
    let (get, [m, k]) = call(got)? else {
        return None;
    };
    (Some(outer) == upserts.with_default
        && get == fns.get
        && same(m, map)
        && same(k, key)
        && inert(default))
    .then_some(default)
}

/// A literal, a name, a field of one, or a list of those: evaluating it
/// earlier answers the same and does nothing else.
fn inert(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Int(..)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Char(_)
        | ExprKind::Bool(_)
        | ExprKind::Unit => true,
        ExprKind::Array { elems, .. } => elems.iter().all(inert),
        _ => stable(e),
    }
}

/// Whether `e` holds a node `is_read` picks, outside any lambda.
fn holds(e: &Expr, is_read: &impl Fn(&Expr) -> bool) -> bool {
    if is_read(e) {
        return true;
    }
    if matches!(e.kind, ExprKind::Lambda { .. }) {
        return false;
    }
    let mut found = false;
    typed::children(e, &mut |c| found = found || holds(c, is_read));
    found
}

/// The first node `is_read` picks in `e`, outside any lambda.
fn find_mut<'a>(e: &'a mut Expr, is_read: &impl Fn(&Expr) -> bool) -> Option<&'a mut Expr> {
    if is_read(e) {
        return Some(e);
    }
    if matches!(e.kind, ExprKind::Lambda { .. }) {
        return None;
    }
    let mut found = None;
    typed::children_mut(e, &mut |c| {
        if found.is_none() {
            found = find_mut(c, is_read);
        }
    });
    found
}

/// The default `withDefault` was handed.
fn into_default(read: Expr) -> Expr {
    match read.kind {
        ExprKind::CallFn { args, .. } => args.into_iter().nth(1),
        _ => None,
    }
    .or_ice("`read` saw the call")
}

/// Whether the call is the map's insert, into the map and under the key the
/// value was read from.
fn is_write(read: &Read, func: &typed::Callee, args: &[Expr]) -> bool {
    func.id == read.fns.insert
        && matches!(args, [map, _, key, ..] if same(map, read.map) && same(key, read.key))
}

fn stmt_expr(s: &Stmt) -> &Expr {
    match s {
        Stmt::Let { value, .. } | Stmt::Expr(value) => value,
    }
}

fn stmt_expr_mut(s: &mut Stmt) -> &mut Expr {
    match s {
        Stmt::Let { value, .. } | Stmt::Expr(value) => value,
    }
}

/// The callee and arguments of a direct call.
fn call(e: &Expr) -> Option<(FnId, &[Expr])> {
    match &e.kind {
        ExprKind::CallFn { func, args } => Some((func.id, args.as_slice())),
        _ => None,
    }
}

fn local(e: &Expr) -> Option<LocalId> {
    match e.kind {
        ExprKind::Local(l) => Some(l),
        _ => None,
    }
}

/// A name, or a field of a stable value: reading it twice reads one value.
fn stable(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Local(_) => true,
        ExprKind::Field { base, .. } => stable(base),
        _ => false,
    }
}

/// Whether two stable expressions read the same value.
fn same(a: &Expr, b: &Expr) -> bool {
    match (&a.kind, &b.kind) {
        (ExprKind::Local(x), ExprKind::Local(y)) => x == y,
        (ExprKind::Field { base: x, index: f }, ExprKind::Field { base: y, index: g }) => {
            f == g && same(x, y)
        }
        _ => false,
    }
}

/// Whether `e` can run inside a lambda unchanged: no `?`, and no lambda of
/// its own, whose captures would need renaming too. Collects the locals it
/// binds and the ones it reads.
fn changes_freely(e: &Expr, bound: &mut Set<LocalId>, reads: &mut Set<LocalId>) -> bool {
    let mut free = true;
    let bind = |p: &typed::Pattern, bound: &mut Set<LocalId>| {
        p.each_bind(&mut |l| {
            bound.insert(l);
        })
    };
    typed::walk(e, &mut |x| match &x.kind {
        ExprKind::Try { .. } => free = false,
        // A lambda that captures nothing reads only its own names.
        ExprKind::Lambda {
            params, captures, ..
        } => {
            if !captures.is_empty() {
                free = false;
            }
            bound.extend(params.iter().copied());
        }
        ExprKind::Local(l) => {
            reads.insert(*l);
        }
        ExprKind::Block { stmts, .. } => stmts.iter().for_each(|s| {
            if let Stmt::Let { pattern, .. } = s {
                bind(pattern, bound);
            }
        }),
        ExprKind::Match { arms, .. } => arms.iter().for_each(|a| bind(&a.pattern, bound)),
        _ => {}
    });
    free
}

/// The locals `body` reads and doesn't bind, other than `param` and the
/// context, in order; or nothing, when `body` can't run inside a lambda as it
/// is.
fn captured(body: &Expr, param: LocalId, ctx: Option<LocalId>) -> Option<Vec<LocalId>> {
    let mut bound = Set::default();
    let mut reads = Set::default();
    if !changes_freely(body, &mut bound, &mut reads) {
        return None;
    }
    let mut captured: Vec<LocalId> = reads
        .into_iter()
        .filter(|l| !bound.contains(l) && *l != param && Some(*l) != ctx)
        .collect();
    captured.sort();
    Some(captured)
}

/// How many times `e` reads `l`.
fn uses(e: &Expr, l: LocalId) -> usize {
    let mut n = 0usize;
    typed::walk(e, &mut |x| {
        n = n.saturating_add(usize::from(matches!(x.kind, ExprKind::Local(y) if y == l)));
    });
    n
}

/// Every read of a local in `e`, through `to`.
fn rename(e: &mut Expr, to: &impl Fn(LocalId) -> LocalId) {
    if let ExprKind::Local(l) = &mut e.kind {
        *l = to(*l);
    }
    typed::children_mut(e, &mut |c| rename(c, to));
}
