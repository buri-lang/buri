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
//!
//! A trait method called on a type parameter runs whichever `impl` the type
//! argument picks, so it is an edge to each `impl` it could pick, and one that
//! can shrink: `T` matched against an `impl` for `Boxed<U>` makes `U` the type
//! inside `T`. Where a cycle has both, each edge says where in the caller's
//! parameter the callee's comes from, and the cycle never ends where going
//! round it leaves the parameter inside more than it took off
//! ([`Graph::endless_walk`]).

use super::*;

/// A type parameter: what declares it, a type or a function, and its index.
type Node<K> = (K, usize);

/// A step into a type: the shape it goes through and which of its parts.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Shape {
    Con(TyConId),
    Tuple(usize),
    Array,
    Fn(usize),
}

/// Where a type sits inside another, from the outside in.
type Path = Vec<(Shape, usize)>;

/// An instantiation a declaration mentions, in terms of the declaration's own
/// parameters.
#[derive(Clone, Copy)]
pub(super) enum Step<'a, K> {
    /// `K` at these arguments.
    Call(K, &'a [Ty]),
    /// A trait method called on `recv`, landing in `K`: the method of the
    /// `impl` for `head`, which has `arity` parameters of its own before the
    /// method's `targs`.
    Dispatch {
        to: K,
        head: Ty,
        arity: usize,
        recv: Ty,
        targs: &'a [Ty],
    },
}

impl<K: Copy> Step<'_, K> {
    fn target(&self) -> K {
        match *self {
            Step::Call(to, _) | Step::Dispatch { to, .. } => to,
        }
    }

    /// The arguments the target gets, where the declaration taking this step
    /// is at `args`.
    fn apply(&self, args: &[Ty]) -> Vec<Ty> {
        match *self {
            Step::Call(_, step_args) => step_args
                .iter()
                .map(|a| substitute(a, args, None))
                .collect(),
            Step::Dispatch {
                head,
                arity,
                recv,
                targs,
                ..
            } => {
                let mut found = Vec::new();
                bind(head, substitute(&recv, args, None), &mut found);
                (0..arity)
                    .map(|i| {
                        found
                            .iter()
                            .rfind(|&&(j, _)| j == i)
                            .map_or(Ty::ERROR, |&(_, t)| t)
                    })
                    .chain(targs.iter().map(|a| substitute(a, args, None)))
                    .collect()
            }
        }
    }
}

/// How a search reached each node: the node before it and the step between,
/// or `None` for where it began.
type Via<'a, K> = HashMap<Node<K>, Option<(Node<K>, Step<'a, K>)>>;

/// A walk [`Graph::walk_from`] keeps: where it is, what it has built, and the
/// walk and step it came by.
type Walk<'a, K> = (Node<K>, Path, Option<(usize, Step<'a, K>)>);

/// Where one of a declaration's parameters lands in an instantiation it
/// mentions.
#[derive(Clone)]
struct Edge<'a, K> {
    from: usize,
    to: Node<K>,
    step: Step<'a, K>,
    /// The callee's parameter is the part of the caller's at `shrink`, inside
    /// new types along `grow`. At most one of the two is non-empty.
    shrink: Path,
    grow: Path,
}

/// A declaration that needs endless instantiations.
pub(super) struct Endless<'a, K> {
    /// Where the error goes: where the cycle is read from, when that is one of
    /// the owners searched, or else the first declared of those it goes
    /// through.
    pub report: K,
    /// Where the cycle is read from: there, at its own parameters, each step
    /// round it only grows them.
    pub from: K,
    pub steps: Vec<Step<'a, K>>,
}

/// The declarations among `decls` whose instantiations never end, each with
/// the arguments it contains itself at. Types that grow through each other are
/// reported once, on the first declared.
pub(super) fn endless(tables: &Tables, decls: &[TyConId]) -> Vec<(TyConId, Vec<Ty>)> {
    let owners: Vec<(TyConId, usize)> = decls
        .iter()
        .map(|&c| (c, tables.tycon(c).arity()))
        .collect();
    let found = search(&owners, |c| {
        let con = tables.tycon(c);
        let mut out = Vec::new();
        for f in con
            .fields()
            .iter()
            .chain(con.variants().iter().flat_map(|v| v.fields.iter()))
        {
            visit_all(f.ty, &mut |mention| {
                if let TyKind::Con(to, args) = mention.kind() {
                    mentions(Step::Call(*to, args), args, 0, &mut out);
                }
            });
        }
        out
    });
    found
        .into_iter()
        .map(|e| {
            let mut args: Vec<Ty> = (0..tables.tycon(e.from).arity())
                .map(|i| Ty::param(i as u32))
                .collect();
            for step in &e.steps {
                args = step.apply(&args);
            }
            (e.report, args)
        })
        .collect()
}

/// The functions among `fns` whose instantiations never end. Functions that
/// grow through each other are reported once (see [`Endless::report`]).
///
/// Calls out of `fns` are followed only when `beyond` says so: a cycle that
/// leaves `fns` comes back only through a trait method, so where none of
/// `fns` is an `impl`'s method, nothing outside needs walking.
pub(super) fn endless_functions<'a>(
    tables: &'a Tables,
    bodies: &'a BodyMap,
    fns: &[FnId],
    beyond: bool,
) -> Vec<Endless<'a, FnId>> {
    let owners: Vec<(FnId, usize)> = fns
        .iter()
        .map(|&f| (f, tables.fn_info(f).generics.len()))
        .collect();
    let mine: HashSet<FnId> = fns.iter().copied().collect();
    // Every `impl` of each trait, in a fixed order, for a method called on a
    // bare parameter, which could reach any of them.
    let mut by_trait: Option<HashMap<TraitId, Vec<(TyConId, &'a ImplInfo)>>> = None;
    search(&owners, |f| {
        let mut out = Vec::new();
        let Some(body) = bodies.get(&f).filter(|_| beyond || mine.contains(&f)) else {
            return out;
        };
        typed::walk(&body.expr, &mut |e| match &e.kind {
            typed::ExprKind::CallFn {
                func: typed::Callee { id, targs },
                ..
            }
            | typed::ExprKind::FnRef(typed::Callee { id, targs }) => {
                mentions(Step::Call(*id, targs), targs, 0, &mut out)
            }
            typed::ExprKind::CallTrait {
                trait_id,
                method,
                recv,
                targs,
                ..
            } => {
                let mut receivers = Vec::new();
                receivers_of(
                    tables,
                    *trait_id,
                    *recv,
                    &mut HashSet::default(),
                    &mut receivers,
                );
                for recv in receivers {
                    let impls: Vec<&ImplInfo> = if matches!(recv.kind(), TyKind::Param(_)) {
                        let index = by_trait.get_or_insert_with(|| {
                            let mut index: HashMap<TraitId, Vec<(TyConId, &ImplInfo)>> =
                                HashMap::default();
                            for ((t, c), imp) in tables.impls.iter() {
                                index.entry(*t).or_default().push((*c, imp));
                            }
                            index.values_mut().for_each(|v| v.sort_by_key(|(c, _)| *c));
                            index
                        });
                        index
                            .get(trait_id)
                            .into_iter()
                            .flatten()
                            .map(|(_, imp)| *imp)
                            .collect()
                    } else {
                        recv.head()
                            .and_then(|con| tables.impls.get(&(*trait_id, con)))
                            .into_iter()
                            .collect()
                    };
                    // A derived `impl` has no method of its own: where one is
                    // reached, its components were taken as receivers above.
                    for (imp, to) in impls
                        .into_iter()
                        .filter_map(|imp| Some((imp, imp.method(*method)?)))
                    {
                        let arity = imp.generics.len();
                        let step = Step::Dispatch {
                            to,
                            head: imp.head,
                            arity,
                            recv,
                            targs,
                        };
                        dispatch(imp.head, recv, &mut |from, param, shrink, grow| {
                            out.push(Edge {
                                from,
                                to: (to, param),
                                step,
                                shrink,
                                grow,
                            });
                        });
                        mentions(step, targs, arity, &mut out);
                    }
                }
            }
            _ => {}
        });
        out
    })
}

/// The types whose own `impl` of `trait_id` a method called on `recv` runs:
/// `recv` itself, or for an array, a tuple, or a type that derives the trait,
/// each of its components', the way monomorphization takes them apart.
fn receivers_of(
    tables: &Tables,
    trait_id: TraitId,
    recv: Ty,
    seen: &mut HashSet<Ty>,
    out: &mut Vec<Ty>,
) {
    if !seen.insert(recv) {
        return;
    }
    let derived = recv
        .head()
        .and_then(|con| tables.impls.get(&(trait_id, con)))
        .is_some_and(ImplInfo::is_derived);
    match recv.kind() {
        TyKind::Array(_) | TyKind::Tuple(_) => parts(recv)
            .into_iter()
            .for_each(|(_, part)| receivers_of(tables, trait_id, part, seen, out)),
        TyKind::Con(con, args) if derived => {
            let decl = tables.tycon(*con);
            for f in decl
                .fields()
                .iter()
                .chain(decl.variants().iter().flat_map(|v| v.fields.iter()))
            {
                receivers_of(tables, trait_id, substitute(&f.ty, args, None), seen, out);
            }
        }
        _ => out.push(recv),
    }
}

/// The instantiations a cycle goes through, from `from` at its own
/// parameters, `rounds` times round.
pub(super) fn instantiations(
    tables: &Tables,
    found: &Endless<'_, FnId>,
    rounds: usize,
) -> Vec<(FnId, Vec<Ty>)> {
    let arity = tables.fn_info(found.from).generics.len();
    let mut args: Vec<Ty> = (0..arity).map(|i| Ty::param(i as u32)).collect();
    let mut out = vec![(found.from, args.clone())];
    for _ in 0..rounds {
        for step in &found.steps {
            args = step.apply(&args);
            out.push((step.target(), args.clone()));
        }
    }
    out
}

/// Binds the parameters of an `impl`'s `head` to the parts of `ty`, which it
/// covers: each parameter's index, with its part.
fn bind(head: Ty, ty: Ty, bound: &mut Vec<(usize, Ty)>) {
    if let TyKind::Param(i) = head.kind() {
        bound.push((*i as usize, ty));
        return;
    }
    for (h, t) in side_by_side(head, ty) {
        bind(h, t, bound);
    }
}

/// The parts of `a` and `b`, two types of the same shape, side by side.
fn side_by_side(a: Ty, b: Ty) -> Vec<(Ty, Ty)> {
    parts(a)
        .into_iter()
        .zip(parts(b))
        .map(|((_, x), (_, y))| (x, y))
        .collect()
}

/// The edges from the parameters inside `args`, the arguments `step` gives
/// its target from the `offset`th on.
fn mentions<'a, K: Copy>(
    step: Step<'a, K>,
    args: &'a [Ty],
    offset: usize,
    out: &mut Vec<Edge<'a, K>>,
) {
    let to = step.target();
    for (j, arg) in args.iter().enumerate() {
        params_in(*arg, &mut Vec::new(), &mut |from, grow| {
            out.push(Edge {
                from,
                to: (to, offset.saturating_add(j)),
                step,
                shrink: Vec::new(),
                grow: grow.clone(),
            });
        });
    }
}

/// Where the parameters of `recv`, a receiver, and of `head`, the head of an
/// `impl` the checker found covers it, meet: each pair, with where each sits
/// inside the other, as `(recv's, head's, shrink, grow)`.
fn dispatch(head: Ty, recv: Ty, visit: &mut dyn FnMut(usize, usize, Path, Path)) {
    match (head.kind(), recv.kind()) {
        (TyKind::Param(u), _) => params_in(recv, &mut Vec::new(), &mut |t, grow| {
            visit(t, *u as usize, Vec::new(), grow.clone())
        }),
        (_, TyKind::Param(t)) => params_in(head, &mut Vec::new(), &mut |u, shrink| {
            visit(*t as usize, u, shrink.clone(), Vec::new())
        }),
        _ => side_by_side(head, recv)
            .into_iter()
            .for_each(|(h, r)| dispatch(h, r, visit)),
    }
}

/// Visits each parameter inside `ty` with where it sits, `path` on.
fn params_in(ty: Ty, path: &mut Path, visit: &mut dyn FnMut(usize, &Path)) {
    if let TyKind::Param(i) = ty.kind() {
        return visit(*i as usize, path);
    }
    for (step, part) in parts(ty) {
        path.push(step);
        params_in(part, path, visit);
        path.pop();
    }
}

/// The types directly inside `ty`, each with the step to it.
fn parts(ty: Ty) -> Vec<((Shape, usize), Ty)> {
    let all = |shape: Shape, inner: &[Ty]| {
        inner
            .iter()
            .enumerate()
            .map(|(k, t)| ((shape, k), *t))
            .collect()
    };
    match ty.kind() {
        TyKind::Con(c, inner) => all(Shape::Con(*c), inner),
        TyKind::Tuple(inner) => all(Shape::Tuple(inner.len()), inner),
        TyKind::Array(element) => all(Shape::Array, std::slice::from_ref(element)),
        TyKind::Fn(params, ret) => {
            let shape = Shape::Fn(params.len());
            let mut out: Vec<_> = all(shape, params);
            out.push(((shape, params.len()), *ret));
            out
        }
        _ => Vec::new(),
    }
}

/// The owners, each with its parameter count, that need endless
/// instantiations, each with the cycle it grows round.
fn search<'a, K, F>(owners: &[(K, usize)], edges_of: F) -> Vec<Endless<'a, K>>
where
    K: Copy + Eq + Ord + std::hash::Hash,
    F: FnMut(K) -> Vec<Edge<'a, K>>,
{
    let mut graph = Graph {
        edges_of,
        edges: HashMap::default(),
    };
    let components = graph.growing_components(
        owners
            .iter()
            .flat_map(|&(k, n)| (0..n).map(move |i| (k, i))),
    );
    let mut component: HashMap<Node<K>, (usize, &Component<'a, K>)> = HashMap::default();
    for (id, c) in components.iter().enumerate() {
        component.extend(c.members.iter().map(|&n| (n, (id, c))));
    }
    let order: HashMap<K, usize> = owners
        .iter()
        .enumerate()
        .map(|(i, &(k, _))| (k, i))
        .collect();
    let mut covered: HashSet<K> = HashSet::default();
    let mut walked: HashSet<usize> = HashSet::default();
    let mut out = Vec::new();
    for &(k, arity) in owners {
        if covered.contains(&k) {
            continue;
        }
        let found = (0..arity).map(|i| (k, i)).find_map(|start| {
            let &(id, c) = component.get(&start)?;
            if !c.shrinks {
                return Some((k, graph.growing_cycle(start, &c.growing)));
            }
            // A component that shrinks somewhere is searched whole, once.
            if !walked.insert(id) {
                return None;
            }
            graph.endless_walk(&c.members, &order, k)
        });
        let Some((from, steps)) = found else { continue };
        covered.extend(steps.iter().map(Step::target));
        let report = order.get(&from).map_or(k, |_| from);
        out.push(Endless {
            report,
            from,
            steps,
        });
    }
    out
}

/// An edge that grows, inside its component: where it leaves, where it lands
/// and the step it is.
type Growing<'a, K> = (Node<K>, Node<K>, Step<'a, K>);

/// A strongly connected component holding an edge that grows.
struct Component<'a, K> {
    members: HashSet<Node<K>>,
    /// Whether it holds an edge that shrinks too, when growing somewhere
    /// doesn't settle it.
    shrinks: bool,
    /// Its edges that grow, in the order its members were taken off the
    /// stack and each one's edges were declared: the first, and the rest.
    growing: (Growing<'a, K>, Vec<Growing<'a, K>>),
}

/// Where Tarjan's algorithm is in one node: the nodes its edges reach and
/// how many of them it has been to, its index and lowest index reached, and
/// how deep the stack was when it went on.
struct Frame<K> {
    next: Vec<Node<K>>,
    k: usize,
    index: usize,
    low: usize,
    depth: usize,
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
    fn edges(&mut self, k: K) -> &[Edge<'a, K>] {
        let edges_of = &mut self.edges_of;
        self.edges.entry(k).or_insert_with(|| edges_of(k))
    }

    /// The edges out of `u`.
    fn out_of(&mut self, u: Node<K>) -> impl Iterator<Item = &Edge<'a, K>> {
        self.edges(u.0).iter().filter(move |e| e.from == u.1)
    }

    /// The nodes reached from `starts` that lie on a cycle through an edge
    /// that grows: the strongly connected components holding such an edge
    /// (Tarjan's algorithm, without recursion, so a long chain of calls can't
    /// overflow the stack).
    fn growing_components(
        &mut self,
        starts: impl Iterator<Item = Node<K>>,
    ) -> Vec<Component<'a, K>> {
        let mut index: HashMap<Node<K>, usize> = HashMap::default();
        let mut component: HashMap<Node<K>, usize> = HashMap::default();
        let mut members: Vec<Vec<Node<K>>> = Vec::new();
        let mut stack: Vec<Node<K>> = Vec::new();
        for s in starts {
            if index.contains_key(&s) {
                continue;
            }
            let mut frames: Vec<Frame<K>> = Vec::new();
            macro_rules! enter {
                ($n:expr) => {{
                    let n = $n;
                    let at = index.len();
                    index.insert(n, at);
                    frames.push(Frame {
                        next: self.out_of(n).map(|e| e.to).collect(),
                        k: 0,
                        index: at,
                        low: at,
                        depth: stack.len(),
                    });
                    stack.push(n);
                }};
            }
            enter!(s);
            while let Some(frame) = frames.last_mut() {
                if let Some(&w) = frame.next.get(frame.k) {
                    frame.k = frame.k.saturating_add(1);
                    match index.get(&w) {
                        None => enter!(w),
                        Some(&iw) if !component.contains_key(&w) => {
                            frame.low = frame.low.min(iw);
                        }
                        Some(_) => {}
                    }
                    continue;
                }
                let Frame {
                    index: at,
                    low,
                    depth,
                    ..
                } = *frame;
                frames.pop();
                if let Some(parent) = frames.last_mut() {
                    parent.low = parent.low.min(low);
                }
                if low == at {
                    let id = members.len();
                    let these = stack.split_off(depth);
                    for &n in &these {
                        component.insert(n, id);
                    }
                    members.push(these);
                }
            }
        }
        let mut out = Vec::new();
        for these in members {
            let mut shrinks = false;
            let mut growing = Vec::new();
            for &u in &these {
                let here = component.get(&u);
                for e in self.out_of(u).filter(|e| component.get(&e.to) == here) {
                    if !e.grow.is_empty() {
                        growing.push((u, e.to, e.step));
                    }
                    shrinks |= !e.shrink.is_empty();
                }
            }
            if let Some((&first, rest)) = growing.split_first() {
                out.push(Component {
                    members: these.into_iter().collect(),
                    shrinks,
                    growing: (first, rest.to_vec()),
                });
            }
        }
        out
    }

    /// The steps of a cycle from `start`, a node in a growing cycle whose
    /// edges never shrink, back to it through an edge that grows: of the
    /// component's `growing` edges, the one leaving nearest `start`.
    fn growing_cycle(
        &mut self,
        start: Node<K>,
        growing: &(Growing<'a, K>, Vec<Growing<'a, K>>),
    ) -> Vec<Step<'a, K>> {
        // Forward from `start`, remembering how each node was reached.
        let mut reached: Via<'a, K> = HashMap::default();
        reached.insert(start, None);
        let mut order = vec![start];
        let mut k = 0;
        while let Some(&u) = order.get(k) {
            k = k.saturating_add(1);
            let next: Vec<(Node<K>, Step<'a, K>)> =
                self.out_of(u).map(|e| (e.to, e.step)).collect();
            for (to, step) in next {
                reached.entry(to).or_insert_with(|| {
                    order.push(to);
                    Some((u, step))
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
                let toward = self
                    .out_of(u)
                    .find(|e| back.contains_key(&e.to))
                    .map(|e| (e.to, e.step));
                if let Some(next) = toward {
                    back.insert(u, Some(next));
                    changed = true;
                }
            }
        }
        // Every node of the component is reached from `start` and leads back
        // to it, so each growing edge closes a cycle; the first reached wins.
        let reached_at: HashMap<Node<K>, usize> =
            order.iter().enumerate().map(|(i, &n)| (n, i)).collect();
        let at = |u: &Node<K>| reached_at.get(u).copied().unwrap_or(usize::MAX);
        let (first, rest) = growing;
        // `min_by_key` keeps the first of two that tie.
        let (u, to, step) = rest.iter().fold(*first, |best, &e| {
            std::cmp::min_by_key(best, e, |g| at(&g.0))
        });
        let mut steps = Vec::new();
        let mut at = u;
        while let Some(&Some((prev, step))) = reached.get(&at) {
            steps.push(step);
            at = prev;
        }
        steps.reverse();
        steps.push(step);
        let mut at = to;
        while let Some(&Some((next, step))) = back.get(&at) {
            steps.push(step);
            at = next;
        }
        steps
    }

    /// A cycle within `these` that grows without end, read from where each
    /// step round it only grows: where it is read from, from `k` if it can
    /// be, or else from the first owner in `order` it can be, and its steps.
    ///
    /// Going round a cycle again and again grows its parameter without end
    /// where it builds more than it takes apart. Then it can be read from
    /// somewhere where it never takes apart more than it has built since, so
    /// what each walk has done is a [`Path`] it has built, and a walk that
    /// would take apart more is dropped. That place is where an edge that grows
    /// leaves, so the walks start there, and none builds more than every edge
    /// of `these` together.
    fn endless_walk(
        &mut self,
        these: &HashSet<Node<K>>,
        order: &HashMap<K, usize>,
        k: K,
    ) -> Option<(K, Vec<Step<'a, K>>)>
    where
        K: Ord,
    {
        let mut starts: Vec<Node<K>> = Vec::new();
        let mut most = 0usize;
        for &u in these {
            for e in self
                .out_of(u)
                .filter(|e| !e.grow.is_empty() && these.contains(&e.to))
            {
                most = most.saturating_add(e.grow.len());
                starts.push(u);
            }
        }
        starts.sort_by_key(|&(f, i)| (f != k, order.get(&f).copied().unwrap_or(usize::MAX), f, i));
        starts.dedup();
        starts
            .into_iter()
            .find_map(|start| Some((start.0, self.walk_from(start, these, most)?)))
    }

    /// The steps of a walk from `start` back to it, within `these`, that
    /// builds something and never takes apart more than it has built, nor
    /// builds more than `most`.
    fn walk_from(
        &mut self,
        start: Node<K>,
        these: &HashSet<Node<K>>,
        most: usize,
    ) -> Option<Vec<Step<'a, K>>> {
        let mut walks: Vec<Walk<'a, K>> = vec![(start, Path::new(), None)];
        let mut seen: HashSet<(Node<K>, Path)> = HashSet::default();
        seen.insert((start, Path::new()));
        let mut k = 0;
        while let Some((u, built, _)) = walks.get(k).cloned() {
            let next: Vec<(Node<K>, Step<'a, K>, Option<Path>)> = self
                .out_of(u)
                .filter(|e| these.contains(&e.to))
                .map(|e| {
                    (
                        e.to,
                        e.step,
                        built
                            .strip_prefix(e.shrink.as_slice())
                            .map(|rest| e.grow.iter().chain(rest).copied().collect()),
                    )
                })
                .collect();
            for (to, step, after) in next {
                let Some(after) = after.filter(|g: &Path| g.len() <= most) else {
                    continue;
                };
                if to == start && !after.is_empty() {
                    let mut steps = vec![step];
                    let mut at = k;
                    while let Some((_, _, Some((prev, step)))) = walks.get(at) {
                        steps.push(*step);
                        at = *prev;
                    }
                    steps.reverse();
                    return Some(steps);
                }
                if seen.insert((to, after.clone())) {
                    walks.push((to, after, Some((k, step))));
                }
            }
            k = k.saturating_add(1);
        }
        None
    }
}

/// Visits `ty` and every type inside it.
fn visit_all(ty: Ty, visit: &mut dyn FnMut(Ty)) {
    visit(ty);
    parts(ty)
        .into_iter()
        .for_each(|(_, part)| visit_all(part, visit));
}
