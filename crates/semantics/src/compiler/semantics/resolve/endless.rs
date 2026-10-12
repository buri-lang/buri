//! Every type has finitely many instantiations (design/static-rules.md rule 14).
//!
//! Each instantiation is compiled on its own, with a layout and glue of its
//! own, so `enum Grow<T> { Deeper(Grow<Option<T>>), Here(T) }` would need
//! `Grow<Option<T>>`, then `Grow<Option<Option<T>>>`, without end.
//!
//! The search is over type parameters. `Grow<Option<T>>` in `Grow`'s fields is
//! an edge from `Grow`'s `T` back to it that grows: `T` lands inside another
//! type rather than as itself. A cycle through an edge that grows never ends.

use super::*;

/// A type parameter: its declaration and its index.
type Node = (TyConId, usize);

/// A type a declaration's fields mention: its constructor and arguments, in
/// terms of the declaration's own parameters.
type Step = (TyConId, &'static [Ty]);

/// Where one of a declaration's parameters lands in a type its fields mention.
#[derive(Clone, Copy)]
struct Edge {
    from: usize,
    to: Node,
    step: Step,
    /// Whether the parameter lands inside another type rather than as itself.
    grows: bool,
}

/// The declarations among `decls` whose instantiations never end, each with
/// the arguments it contains itself at. Types that grow through each other are
/// reported once, on the first declared.
pub(super) fn endless(tables: &Tables, decls: &[TyConId]) -> Vec<(TyConId, Vec<Ty>)> {
    let mut graph = Graph { tables, edges: HashMap::default() };
    let mut covered: HashSet<TyConId> = HashSet::default();
    let mut out = Vec::new();
    for &c in decls {
        if covered.contains(&c) {
            continue;
        }
        let arity = tables.tycon(c).arity();
        let Some(cycle) = (0..arity).find_map(|i| graph.growing_cycle((c, i))) else { continue };
        let mut args: Vec<Ty> = (0..arity).map(|i| Ty::param(i as u32)).collect();
        for (con, step_args) in cycle {
            covered.insert(con);
            args = step_args.iter().map(|a| substitute(a, &args, None)).collect();
        }
        out.push((c, args));
    }
    out
}

struct Graph<'a> {
    tables: &'a Tables,
    edges: HashMap<TyConId, Vec<Edge>>,
}

impl Graph<'_> {
    /// Every edge out of `c`'s parameters, through every field of every
    /// variant.
    fn edges(&mut self, c: TyConId) -> Vec<Edge> {
        let tables = self.tables;
        self.edges
            .entry(c)
            .or_insert_with(|| {
                let con = tables.tycon(c);
                let mut out = Vec::new();
                for f in con.fields().iter().chain(con.variants().iter().flat_map(|v| v.fields.iter())) {
                    walk(f.ty, &mut |mention| {
                        let TyKind::Con(to, args) = mention.kind() else { return };
                        for (j, arg) in args.iter().enumerate() {
                            walk(*arg, &mut |inner| {
                                if let TyKind::Param(i) = inner.kind() {
                                    let grows = *arg != inner;
                                    out.push(Edge { from: *i as usize, to: (*to, j), step: (*to, args), grows });
                                }
                            });
                        }
                    });
                }
                out
            })
            .clone()
    }

    /// The steps of a cycle from `start` back to it through an edge that
    /// grows, if there is one.
    fn growing_cycle(&mut self, start: Node) -> Option<Vec<Step>> {
        // Forward from `start`, remembering how each node was reached.
        let mut reached: HashMap<Node, Option<(Node, Step)>> = HashMap::default();
        reached.insert(start, None);
        let mut order = vec![start];
        let mut k = 0;
        while let Some(&u) = order.get(k) {
            k = k.saturating_add(1);
            for e in self.edges(u.0).into_iter().filter(|e| e.from == u.1) {
                reached.entry(e.to).or_insert_with(|| {
                    order.push(e.to);
                    Some((u, e.step))
                });
            }
        }
        // Backward: the reached nodes that lead to `start`, each with its next
        // step towards it.
        let mut back: HashMap<Node, Option<(Node, Step)>> = HashMap::default();
        back.insert(start, None);
        let mut changed = true;
        while changed {
            changed = false;
            for &u in &order {
                if back.contains_key(&u) {
                    continue;
                }
                let toward = self.edges(u.0).into_iter().find(|e| e.from == u.1 && back.contains_key(&e.to));
                if let Some(e) = toward {
                    back.insert(u, Some((e.to, e.step)));
                    changed = true;
                }
            }
        }
        let (u, e) = order.iter().find_map(|&u| {
            self.edges(u.0).into_iter().find(|e| e.from == u.1 && e.grows && back.contains_key(&e.to)).map(|e| (u, e))
        })?;
        let mut steps = Vec::new();
        let mut at = u;
        while let Some(&Some((prev, step))) = reached.get(&at) {
            steps.push(step);
            at = prev;
        }
        steps.reverse();
        steps.push(e.step);
        let mut at = e.to;
        while let Some(&Some((next, step))) = back.get(&at) {
            steps.push(step);
            at = next;
        }
        Some(steps)
    }
}

/// Visits `ty` and every type inside it.
fn walk(ty: Ty, visit: &mut dyn FnMut(Ty)) {
    visit(ty);
    match ty.kind() {
        TyKind::Con(_, inner) | TyKind::Tuple(inner) => inner.iter().for_each(|t| walk(*t, visit)),
        TyKind::Array(element) => walk(*element, visit),
        TyKind::Fn(params, ret) => {
            params.iter().for_each(|t| walk(*t, visit));
            walk(*ret, visit);
        }
        _ => {}
    }
}
