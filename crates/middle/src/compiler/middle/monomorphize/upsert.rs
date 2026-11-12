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
//! The change is a lambda that captures nothing: the context and the one other
//! local it reads come in as arguments, so making it allocates nothing. A
//! change that reads two or more other locals is left alone.
//!
//! **Why it is correct.** The map and the key are each a name or a field of
//! one, so both reads name the same values, and `get` and `withDefault` have
//! no effect but their answer. The default is still evaluated where the `let`
//! was. The value is read nowhere but in the insert's value, which now runs
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

/// One map's `get` and `insert`, and the `upserted` that stands for both.
struct MapFns {
    get: FnId,
    insert: FnId,
    upserted: FnId,
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
                // A loaded map module declares its type and `upserted`.
                let (mut con, mut upserted) = (None, None);
                for (name, sym) in &scope.own {
                    match sym {
                        Sym::Ty(c) if name == ty => con = Some(*c),
                        Sym::Fn(f) if name == "upserted" => upserted = Some(*f),
                        _ => {}
                    }
                }
                let con = con.or_ice("a map module declares its type");
                let method = |name| checked.tables.method(con, name).or_ice("a map method");
                Some(MapFns {
                    get: method("get"),
                    insert: method("insert"),
                    upserted: upserted.or_ice("a map module declares `upserted`"),
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
        stmts: &mut [Stmt],
        tail: &mut Option<Box<Expr>>,
        targs: &[Ty],
    ) {
        if !self.upsert_here {
            return;
        }
        let upserts = std::mem::take(&mut self.upserts);
        for i in 0..stmts.len() {
            let Some((Stmt::Let { pattern, value, .. }, after)) =
                stmts.split_at_mut(i).1.split_first_mut()
            else {
                continue;
            };
            let Some(read) = read(&upserts, &pattern.kind, value) else {
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
                let (kind, fallback) = self.upsert(read, func, args, e.span, targs);
                e.kind = kind;
                fallback
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

    /// The insert as a call to `upserted`, and the local that call reads the
    /// default from; or the insert as it was, when its value can't become a
    /// lambda.
    fn upsert(
        &mut self,
        read: &Read,
        func: typed::Callee,
        args: Vec<Expr>,
        span: Span,
        targs: &[Ty],
    ) -> (ExprKind, Option<LocalId>) {
        let mut args = args.into_iter();
        let mut next = || args.next().or_ice("`is_write` saw four arguments");
        let (map, ctx, key, mut body) = (next(), next(), next(), next());
        let ctx_local = local(&ctx);
        let mut bound = Set::default();
        let mut reads = Set::default();
        let free = changes_freely(&body, &mut bound, &mut reads);
        let captured: Vec<LocalId> = reads
            .into_iter()
            .filter(|l| !bound.contains(l) && *l != read.value && Some(*l) != ctx_local)
            .collect();
        if !free || captured.len() > 1 {
            let args = vec![map, ctx, key, body];
            return (ExprKind::CallFn { func, args }, None);
        }
        let (ctx_ty, value_ty) = (ctx.ty, body.ty);
        let fallback = self.new_local("__fallback", self.sub(&value_ty, targs), span);
        let changed_ctx = self.new_local("__ctx", self.sub(&ctx_ty, targs), span);
        let arg_local = captured.first().copied();
        let (arg, arg_ty) = match arg_local {
            Some(x) => {
                let ty = self.locals.get(x).ty;
                (Expr::new(ExprKind::Local(x), ty, span), ty)
            }
            None => (Expr::new(ExprKind::Unit, Ty::UNIT, span), Ty::UNIT),
        };
        let changed_arg = self.new_local("__arg", arg_ty, span);
        rename(&mut body, &|l| {
            if Some(l) == ctx_local {
                changed_ctx
            } else if Some(l) == arg_local {
                changed_arg
            } else {
                l
            }
        });
        let change = Expr::new(
            ExprKind::Lambda {
                params: vec![changed_ctx, read.value, changed_arg],
                body: Box::new(body),
                captures: Vec::new(),
            },
            Ty::func([ctx_ty, value_ty, arg_ty], value_ty),
            span,
        );
        let mut call_targs = func.targs;
        call_targs.push(arg_ty);
        let call = ExprKind::CallFn {
            func: typed::Callee {
                id: read.fns.upserted,
                targs: call_targs,
            },
            args: vec![
                ctx,
                map,
                key,
                Expr::new(ExprKind::Local(fallback), value_ty, span),
                arg,
                change,
            ],
        };
        (call, Some(fallback))
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
    let mut bind = |p: &typed::Pattern| {
        p.each_bind(&mut |l| {
            bound.insert(l);
        })
    };
    typed::walk(e, &mut |x| match &x.kind {
        ExprKind::Try { .. } | ExprKind::Lambda { .. } => free = false,
        ExprKind::Local(l) => {
            reads.insert(*l);
        }
        ExprKind::Block { stmts, .. } => stmts.iter().for_each(|s| {
            if let Stmt::Let { pattern, .. } = s {
                bind(pattern);
            }
        }),
        ExprKind::Match { arms, .. } => arms.iter().for_each(|a| bind(&a.pattern)),
        _ => {}
    });
    free
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
