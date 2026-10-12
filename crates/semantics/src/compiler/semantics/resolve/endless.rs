//! Every type and every function has finitely many instantiations
//! (design/static-rules.md rule 14).
//!
//! Each instantiation is compiled on its own, with a layout and glue of its
//! own, so `enum Grow<T> { Deeper(Grow<Option<T>>), Here(T) }` would need
//! `Grow<Option<T>>`, then `Grow<Option<Option<T>>>`, without end. A function
//! that calls itself at `Option<T>` needs the same endless copies of itself.
//!
//! The search is over type parameters. `Grow<Option<T>>` in `Grow`'s fields is
//! an edge from `Grow`'s `T` back to it that grows: `T` lands inside another
//! type rather than as itself. A call's type arguments are edges the same way,
//! from the caller's parameters to the callee's. A cycle through an edge that
//! grows never ends.

use super::*;

/// A type parameter: what declares it, a type or a function, and its index.
type Node<K> = (K, usize);

/// An instantiation a declaration mentions: what it instantiates and the
/// arguments, in terms of the declaration's own parameters.
type Step<'a, K> = (K, &'a [Ty]);

/// How a search reached each node: the node before it and the step between,
/// or `None` for where it began.
type Via<'a, K> = HashMap<Node<K>, Option<(Node<K>, Step<'a, K>)>>;

/// Where one of a declaration's parameters lands in an instantiation it
/// mentions.
#[derive(Clone, Copy)]
struct Edge<'a, K> {
    from: usize,
    to: Node<K>,
    step: Step<'a, K>,
    /// Whether the parameter lands inside another type rather than as itself.
    grows: bool,
}

/// The declarations among `decls` whose instantiations never end, each with
/// the arguments it contains itself at. Types that grow through each other are
/// reported once, on the first declared.
pub(super) fn endless(tables: &Tables, decls: &[TyConId]) -> Vec<(TyConId, Vec<Ty>)> {
    let owners: Vec<(TyConId, usize)> = decls.iter().map(|&c| (c, tables.tycon(c).arity())).collect();
    search(&owners, |c| {
        let con = tables.tycon(c);
        let mut out = Vec::new();
        for f in con.fields().iter().chain(con.variants().iter().flat_map(|v| v.fields.iter())) {
            walk(f.ty, &mut |mention| {
                if let TyKind::Con(to, args) = mention.kind() {
                    mentions(*to, args, &mut out);
                }
            });
        }
        out
    })
}

/// The functions among `fns` whose instantiations never end, each with the
/// type arguments it calls itself at. Functions that grow through each other
/// are reported once, on the first declared. A cycle stays among `fns`, so
/// calls out of them are not followed: what they call can't call them back.
pub(super) fn endless_functions(tables: &Tables, bodies: &BodyMap, fns: &[FnId]) -> Vec<(FnId, Vec<Ty>)> {
    let owners: Vec<(FnId, usize)> = fns.iter().map(|&f| (f, tables.fn_info(f).generics.len())).collect();
    let mine: HashSet<FnId> = fns.iter().copied().collect();
    search(&owners, |f| {
        let mut out = Vec::new();
        if let Some(body) = bodies.get(&f).filter(|_| mine.contains(&f)) {
            typed::walk(&body.expr, &mut |e| {
                if let typed::ExprKind::CallFn { func: typed::Callee::Decl { id, targs }, .. }
                | typed::ExprKind::FnRef(typed::Callee::Decl { id, targs }) = &e.kind
                {
                    mentions(*id, targs, &mut out);
                }
            });
        }
        out
    })
}

/// The edges from the parameters inside `args`, an instantiation of `to`.
fn mentions<'a, K: Copy>(to: K, args: &'a [Ty], out: &mut Vec<Edge<'a, K>>) {
    for (j, arg) in args.iter().enumerate() {
        walk(*arg, &mut |inner| {
            if let TyKind::Param(i) = inner.kind() {
                out.push(Edge { from: *i as usize, to: (to, j), step: (to, args), grows: *arg != inner });
            }
        });
    }
}

/// The owners, each with its parameter count, that need endless
/// instantiations, each with the arguments it reaches itself at.
fn search<'a, K, F>(owners: &[(K, usize)], edges_of: F) -> Vec<(K, Vec<Ty>)>
where
    K: Copy + Eq + std::hash::Hash,
    F: FnMut(K) -> Vec<Edge<'a, K>>,
{
    let mut graph = Graph { edges_of, edges: HashMap::default() };
    let growing = graph.in_growing_cycles(owners.iter().flat_map(|&(k, n)| (0..n).map(move |i| (k, i))));
    let mut covered: HashSet<K> = HashSet::default();
    let mut out = Vec::new();
    for &(k, arity) in owners {
        if covered.contains(&k) {
            continue;
        }
        let Some(start) = (0..arity).map(|i| (k, i)).find(|n| growing.contains(n)) else { continue };
        let cycle = graph.growing_cycle(start);
        let mut args: Vec<Ty> = (0..arity).map(|i| Ty::param(i as u32)).collect();
        for (step, step_args) in cycle {
            covered.insert(step);
            args = step_args.iter().map(|a| substitute(a, &args, None)).collect();
        }
        out.push((k, args));
    }
    out
}

struct Graph<'a, K, F> {
    edges_of: F,
    edges: HashMap<K, Vec<Edge<'a, K>>>,
}

impl<'a, K, F> Graph<'a, K, F>
where
    K: Copy + Eq + std::hash::Hash,
    F: FnMut(K) -> Vec<Edge<'a, K>>,
{
    /// Every edge out of `k`'s parameters.
    fn edges(&mut self, k: K) -> Vec<Edge<'a, K>> {
        let edges_of = &mut self.edges_of;
        self.edges.entry(k).or_insert_with(|| edges_of(k)).clone()
    }

    /// Where `u` leads, edge by edge.
    fn successors(&mut self, u: Node<K>) -> Vec<Node<K>> {
        self.edges(u.0).into_iter().filter(|e| e.from == u.1).map(|e| e.to).collect()
    }

    /// The nodes reached from `starts` that lie on a cycle through an edge
    /// that grows: the strongly connected components holding such an edge
    /// (Tarjan's algorithm, without recursion, so a long chain of calls can't
    /// overflow the stack).
    fn in_growing_cycles(&mut self, starts: impl Iterator<Item = Node<K>>) -> HashSet<Node<K>> {
        let mut index: HashMap<Node<K>, usize> = HashMap::default();
        let mut low: HashMap<Node<K>, usize> = HashMap::default();
        let mut component: HashMap<Node<K>, usize> = HashMap::default();
        let mut members: Vec<Vec<Node<K>>> = Vec::new();
        let mut stack: Vec<Node<K>> = Vec::new();
        for s in starts {
            if index.contains_key(&s) {
                continue;
            }
            let mut frames: Vec<(Node<K>, Vec<Node<K>>, usize)> = Vec::new();
            macro_rules! enter {
                ($n:expr) => {{
                    let n = $n;
                    let at = index.len();
                    index.insert(n, at);
                    low.insert(n, at);
                    stack.push(n);
                    let next = self.successors(n);
                    frames.push((n, next, 0));
                }};
            }
            enter!(s);
            while let Some((u, next, k)) = frames.last_mut() {
                let u = *u;
                if let Some(&w) = next.get(*k) {
                    *k = k.saturating_add(1);
                    match index.get(&w) {
                        None => enter!(w),
                        Some(&iw) if !component.contains_key(&w) => {
                            let lu = low.get_mut(&u).or_ice("entered");
                            *lu = (*lu).min(iw);
                        }
                        Some(_) => {}
                    }
                    continue;
                }
                frames.pop();
                let lu = *low.get(&u).or_ice("entered");
                if let Some((parent, ..)) = frames.last() {
                    let lp = low.get_mut(parent).or_ice("entered");
                    *lp = (*lp).min(lu);
                }
                if Some(&lu) == index.get(&u) {
                    let id = members.len();
                    let mut these = Vec::new();
                    while let Some(n) = stack.pop() {
                        component.insert(n, id);
                        these.push(n);
                        if n == u {
                            break;
                        }
                    }
                    members.push(these);
                }
            }
        }
        let mut out = HashSet::default();
        for these in members {
            let grows = these.iter().any(|&u| {
                let here = component.get(&u);
                self.edges(u.0).iter().any(|e| e.from == u.1 && e.grows && component.get(&e.to) == here)
            });
            if grows {
                out.extend(these);
            }
        }
        out
    }

    /// The steps of a cycle from `start`, a node in a growing cycle, back to
    /// it through an edge that grows.
    fn growing_cycle(&mut self, start: Node<K>) -> Vec<Step<'a, K>> {
        // Forward from `start`, remembering how each node was reached.
        let mut reached: Via<'a, K> = HashMap::default();
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
        let mut back: Via<'a, K> = HashMap::default();
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
        let found = order.iter().find_map(|&u| {
            self.edges(u.0).into_iter().find(|e| e.from == u.1 && e.grows && back.contains_key(&e.to)).map(|e| (u, e))
        });
        let (u, e) = found.or_ice("a node in a growing cycle has one");
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
        steps
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
