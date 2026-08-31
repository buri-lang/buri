//! The layer-A tree into the layer-B CFG.
//!
//! Takes the `typed::Expr` bodies that `middle::run` and `middle::native` have
//! finished with and produces [`super::ir`]: one `ir::Func` per
//! `monomorphize::Func`, at the same index, in block-argument SSA.
//!
//! No byte is chosen here. An aggregate stays one SSA value naming the source
//! type whose layout it has, and the flattening of VALUE-MODEL.md §5.1 happens
//! in a backend against [`super::layout`] — the module header of [`super::ir`]
//! argues why that boundary is where it is.
//!
//! It also assigns `Func::unit`, the codegen unit — the set of monomorphized
//! functions whose declaration came from one source module. That partition
//! already exists in the data, because `Func::debug_name` is `module:owner.name`;
//! this is where it becomes an explicit number the action graph can key on
//! (`design/native/ARCHITECTURE.md` §5).
//!
//! Design: `design/native/ARCHITECTURE.md` §2.1.
//!
//! # How a tree becomes a graph
//!
//! One rule does most of the work: **an expression is lowered into whatever
//! block is current, and answers one value.** A construct that branches makes
//! its own blocks, gives the join block a parameter, and answers that
//! parameter — which is a phi, in the right place, without anything here
//! knowing the word.
//!
//! The rule needs one escape, because an expression can also *not* answer:
//! `abort`, `?` on a `.None`, and a tail call that became a back edge all
//! leave with control gone. Rather than making every caller handle an
//! `Option<ValueId>`, those lower into a fresh block nothing jumps to and
//! answer that block's parameter. Everything downstream is then emitted into
//! dead code, and [`ir::Code::retain_reachable`] deletes it at the end of the
//! function. The producers stay straight-line and the output has no dead
//! blocks in it.
//!
//! # Tail calls arrive as loops
//!
//! `middle::tail_calls` rewrites before this runs, so a self-recursive tail
//! call is already an `ExprKind::Loop` with one entry and an
//! `ExprKind::Continue` inside it, and a mutually recursive group is already
//! one merged function with an entry per member. Lowering has no tail-position
//! rule of its own — which was the point of moving the rewrite
//! (`ARCHITECTURE.md` §2.2).
//!
//! A `Loop` becomes a header block whose parameters are the loop's variables,
//! and a `Continue` becomes a jump back to it with the new values. That is the
//! "rebind and continue" of CODEGEN-LLVM.md §2.4, and it is why no mutable
//! slot and no `alloca` appears anywhere in this file.
//!
//! **The dispatch parameter.** A merged group's function is entered at a
//! chosen entry, and `ExprKind::Continue` spells that entry as a number rather
//! than as an argument at a type the middle end would have had to invent. So
//! lowering materialises it: a function whose body is a `Loop` with more than
//! one entry takes **one extra leading `i32` parameter**, the entry index, and
//! its header switches on it. Both backends need to know that, and this
//! sentence is where they are told.
//!
//! # What is not lowered here, and why
//!
//! * **A `Lambda`.** `middle::closures` lifts one to a top-level function over
//!   an explicit environment (`ARCHITECTURE.md` §2.2). Until it lands, a
//!   lambda that survives to here is an abort naming the pass, because the
//!   alternative — lifting it in the lowering — would be closure conversion
//!   written a second time in the file that must not own it.
//! * **Where a reference count goes.** `middle::rc` decides that, over the
//!   tree, and this pass *places* what it decided ([`run_with`]). The division
//!   is the pass's own (`rc.rs`, "where the operations go"): ownership is a
//!   fixpoint over a call graph that is exact only in layer A, and last use is
//!   a property of an evaluation order the CFG has already flattened — while
//!   the tree has no statement form to hang an `incref` on and a block does.
//! * **The drop glue a `decref` calls.** Generated per layout, so
//!   [`ir::Inst::DecRef`]'s `drop` is left `None` and each backend fills it
//!   from its own layout table (`stencil/glue.rs`'s `Helper::Walk`).
//! * **`monomorphize::Func::desc`.** The descriptor a `testing_assert.report`
//!   or a `json.decode` is handed. No descriptor reaches a native artifact at
//!   all (VALUE-MODEL.md §9), so it is dropped here and `middle::derives`
//!   supplies a generated `show`/`decode` at the type instead — the same
//!   substitution [`ir::Inst::Structural`] stands for.

use crate::compiler::middle::ir::{
    self, BinOp, BlockId, Body, Code, Const, Facts, Func, Inst, Ownership, Purity, Signature,
    StructuralOp, Target, Term, Type, TypeId, TypeInfo, UnOp, ValueId,
};
use crate::compiler::middle::monomorphize::{FuncKind, Program};
use crate::compiler::middle::rc;
use crate::compiler::semantics::typed::{
    self, Arm, ArrayRest, Expr, ExprKind, FieldPat, OptionOrResult, PatKind, Pattern, PrimOp,
    Stmt, TemplatePart,
};
use crate::compiler::semantics::types::{FuncIdx, LocalId, Prim, Tables, Ty};
use crate::diagnostics::Invariant as _;
use crate::hash::Map as HashMap;

/// Lowers every function in the program.
///
/// Takes the program by reference and builds a new one rather than consuming
/// it: the JavaScript backend reads the same tree, `--backend` can ask for two
/// backends over one program (`ARCHITECTURE.md` §4), and a lowering that ate
/// its input would make an agreement test over two backends impossible to
/// write.
pub fn run(program: &Program, tables: &Tables) -> ir::Program {
    let mut counted = rc::Syntactic::new(program);
    let plan = rc::analyze(program, &mut counted, &rc::Options::default());
    run_with(program, tables, &plan)
}

/// The same, against a caller's own reference-counting plan.
///
/// `middle::rc` computes ownership, purity and the placement of every
/// `incref`/`decref` over the *tree* — where the call graph is exact and
/// evaluation order is still stated — and this pass places them, because the
/// tree has no statement form to hang one on and the CFG does (`rc.rs`, "where
/// the operations go"). [`run`] asks for a plan from `rc::Syntactic`, which is
/// the same classifier both native backends build from the same `Program` — see
/// `rc.rs`, "which types carry a count", for why it has to be.
pub fn run_with(program: &Program, tables: &Tables, plan: &rc::Plan) -> ir::Program {
    let mut types = Types::default();
    let units = Units::assign(program);
    // How many loop entries each function has, which is how a `Continue` into
    // another function knows whether to pass a dispatch index. Computed for
    // the whole program first because a forwarder is lowered before the merged
    // function it names.
    let entries: Vec<usize> = program.funcs.iter().map(|f| loop_entries(f.body())).collect();

    let mut funcs = Vec::with_capacity(program.funcs.len());
    for (i, f) in program.funcs.iter().enumerate() {
        let dispatch = entries.get(i).copied().unwrap_or(0) > 1;
        let mut sig = Signature { params: Vec::new(), rets: Vec::new() };
        if dispatch {
            sig.params.push(Type::I32);
        }
        for p in &f.params {
            let ty = f.locals.get(p.index()).map(|l| l.ty.clone()).unwrap_or(Ty::Unit);
            let t = types.of(tables, &ty);
            sig.params.push(t);
        }
        let ret = returns(f);
        sig.rets.push(types.of(tables, &ret));

        let fplan = plan.func(FuncIdx(i as u32));
        let body = match &f.kind {
            FuncKind::Intrinsic(key) => Body::Runtime(bounded_key(tables, key, &ret)),
            FuncKind::Unbuilt | FuncKind::Body(_) => {
                let mut lower = FnLower {
                    tables,
                    program,
                    types: &mut types,
                    entries: &entries,
                    locals: &f.locals,
                    ret: ret.clone(),
                    code: Code::new(),
                    cur: BlockId(0),
                    env: vec![None; f.locals.len()],
                    loops: Vec::new(),
                    unmatched: None,
                    sites: Sites::of(fplan, f.body()),
                    node_values: HashMap::default(),
                };
                Body::Code(lower.func(&sig, &f.params, f.body(), dispatch))
            }
        };

        funcs.push(Func {
            facts: facts(fplan, &sig, dispatch),
            symbol: f.symbol.clone(),
            debug_name: f.debug_name.clone(),
            sig,
            unit: units.of(&f.debug_name),
            body,
            span: f.span,
        });
    }

    ir::Program {
        funcs,
        units: units.names,
        types: types.list,
        crosses_tasks: plan.crosses_tasks,
    }
}

/// What a backend may assume, from the plan where there is one.
///
/// The dispatch parameter a merged group's function takes is not a Buri
/// parameter and `middle::rc` knows nothing about it, so it is prepended here
/// as owned — an entry index is an integer, and ownership of an integer is a
/// question with no content. A plan whose column is a different length from
/// the signature is not trusted at all, because a *shifted* ownership column
/// is a missing increment on the wrong parameter.
fn facts(plan: Option<&rc::FuncPlan>, sig: &Signature, dispatch: bool) -> Facts {
    let conservative = Facts {
        params: vec![Ownership::Own; sig.params.len()],
        purity: Purity::Effectful,
        can_abort: true,
        can_park: true,
    };
    let Some(plan) = plan else { return conservative };
    let mut params = plan.params.clone();
    if dispatch {
        params.insert(0, Ownership::Own);
    }
    if params.len() != sig.params.len() {
        return conservative;
    }
    Facts { params, purity: plan.purity, can_abort: plan.can_abort, can_park: plan.can_park }
}

/// What a function returns.
///
/// Not `monomorphize::Func::ret`, which is only filled in for an intrinsic
/// (`monomorphize.rs:398`) and is `()` for every function with a body — and
/// for the merged function `tail_calls` builds out of a group, which copies
/// it. The body's own type is the answer that is always right, and for a body
/// that is a `Loop` it is the type of an entry: the loop expression carries
/// the same unfilled `ret` its function does.
fn returns(f: &crate::compiler::middle::monomorphize::Func) -> Ty {
    let Some(body) = f.body() else { return f.ret.clone() };
    match &body.kind {
        ExprKind::Loop { entries } => {
            entries.first().map(|e| e.ty.clone()).unwrap_or_else(|| body.ty.clone())
        }
        _ => body.ty.clone(),
    }
}

/// The number of entries the body's loop has, or zero where the body is not a
/// loop.
fn loop_entries(body: Option<&Expr>) -> usize {
    match body.map(|b| &b.kind) {
        Some(ExprKind::Loop { entries }) => entries.len(),
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Codegen units
// ---------------------------------------------------------------------------

/// The codegen-unit partition: one unit per source module (ARCHITECTURE.md §5).
#[derive(Default)]
struct Units {
    names: Vec<String>,
    index: HashMap<String, u32>,
}

impl Units {
    /// Assigns a number to every unit the program names, in function order —
    /// which is source order (`monomorphize.rs:247-248`), so the partition is
    /// the same on two builds of one commit.
    fn assign(program: &Program) -> Units {
        let mut units = Units::default();
        for f in &program.funcs {
            let _ = units.intern(&f.debug_name);
        }
        units
    }

    fn intern(&mut self, debug_name: &str) -> u32 {
        let name = unit_name(debug_name);
        if let Some(i) = self.index.get(&name) {
            return *i;
        }
        let i = self.names.len() as u32;
        self.index.insert(name.clone(), i);
        self.names.push(name);
        i
    }

    fn of(&self, debug_name: &str) -> u32 {
        self.index.get(&unit_name(debug_name)).copied().unwrap_or(0)
    }
}

/// The unit a `module:owner.name` debug name belongs to, as a filename stem.
///
/// A merged tail-call group is named `tail group core/list:a, core/list:b`, so
/// the module is the last word before the first colon rather than everything
/// before it. A name with no colon at all — a test body, a context
/// constructor — has no declaring module and lands in `root`.
fn unit_name(debug_name: &str) -> String {
    let Some((head, _)) = debug_name.split_once(':') else {
        return "root".into();
    };
    let module = head.split_whitespace().last().unwrap_or("root");
    module.replace(['/', '.'], "_")
}

// ---------------------------------------------------------------------------
// The type interner
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Types {
    list: Vec<TypeInfo>,
    index: HashMap<Ty, TypeId>,
}

impl Types {
    fn intern(&mut self, tables: &Tables, ty: &Ty) -> TypeId {
        let ty = &Types::runtime_ty(tables, ty);
        if let Some(id) = self.index.get(ty) {
            return *id;
        }
        let id = TypeId(self.list.len() as u32);
        self.list.push(TypeInfo {
            name: crate::compiler::semantics::types::show(tables, None, &[], ty),
            ty: ty.clone(),
        });
        self.index.insert(ty.clone(), id);
        id
    }

    /// `Template` is `Str` — VALUE-MODEL.md §3.3, "there is no `Template`
    /// value at run time on either backend" — so the two intern to one
    /// `TypeId` and a value of either can meet a value of the other.
    ///
    /// This is not a tidiness: the language's one implicit conversion widens a
    /// `Str` to a `Template` in argument position (`expressions.rs`'s
    /// `coerce`), and it does that by *wrapping* the expression in a
    /// one-hole `Template` whose hole is already a string — which [`template`]
    /// then lowers by handing the hole's own value straight back, typed `Str`.
    /// So a `match` whose type is `Template` because one arm interpolates has
    /// arms producing `Str`, its join block's parameter is `Template`, and
    /// without this the IR does not verify:
    ///
    /// ```text
    /// let r = match (o) { .Some(v) => "s${v}", .None => "n" };
    /// stdout.println(r)
    /// ```
    ///
    /// `Prim::Str | Prim::Template` is already one arm everywhere below this —
    /// `layout.rs`, `rc.rs`, and both backends' `emit` — so the distinction had
    /// no consumer past the front end; keeping it in the interner only gave the
    /// verifier two names for one machine type.
    ///
    /// [`template`]: Lowering::template
    fn runtime_ty(tables: &Tables, ty: &Ty) -> Ty {
        if matches!(tables.as_prim(ty), Some(Prim::Template)) {
            return tables.prim(Prim::Str);
        }
        ty.clone()
    }

    /// The machine shape of a source type. Total: a type with no register
    /// shape is an aggregate, and that includes `Str`, a list, a closure, a
    /// context and — where a tree reached lowering with one — `Ty::Error`.
    fn of(&mut self, tables: &Tables, ty: &Ty) -> Type {
        if matches!(ty, Ty::Unit) {
            return Type::Unit;
        }
        match tables.as_prim(ty).and_then(Type::of_prim) {
            Some(t) => t,
            None => Type::Agg(self.intern(tables, ty)),
        }
    }
}

// ---------------------------------------------------------------------------
// One function
// ---------------------------------------------------------------------------

/// Where every reference-count operation goes, by node.
///
/// `middle::rc` keys its sites by the pre-order index of an expression in one
/// body, and [`rc::preorder`] is that numbering — used here rather than
/// recomputed, because two implementations of a numbering is one edit away
/// from an increment landing on the wrong value. The map is from the address
/// of an expression, so lowering does not have to visit nodes in the order the
/// numbering does: it asks the node it is holding what its number is. An
/// address is an identity here because the body is borrowed, immutably, for
/// the whole of the function's lowering — nothing moves and nothing is
/// dropped, so no two nodes can share one.
#[derive(Default)]
struct Sites {
    at: HashMap<(u32, bool), Vec<rc::Site>>,
    ids: HashMap<*const Expr, rc::NodeId>,
}

impl Sites {
    fn of(plan: Option<&rc::FuncPlan>, body: Option<&Expr>) -> Sites {
        let (Some(plan), Some(body)) = (plan, body) else { return Sites::default() };
        if plan.sites.is_empty() {
            return Sites::default();
        }
        let mut sites = Sites::default();
        rc::preorder(body, &mut |id, e| {
            sites.ids.insert(std::ptr::from_ref(e), id);
        });
        for site in &plan.sites {
            sites
                .at
                .entry((site.node.0, matches!(site.at, rc::Position::After)))
                .or_default()
                .push(*site);
        }
        sites
    }

    fn id_of(&self, e: &Expr) -> Option<rc::NodeId> {
        self.ids.get(&std::ptr::from_ref(e)).copied()
    }

    fn get(&self, node: rc::NodeId, after: bool) -> &[rc::Site] {
        self.at.get(&(node.0, after)).map(Vec::as_slice).unwrap_or_default()
    }
}

/// One loop being lowered: where to jump back to, and with what.
struct LoopFrame {
    header: BlockId,
    /// The header parameter carrying the entry index, where the loop has more
    /// than one entry.
    dispatch: Option<ValueId>,
    /// The header parameters the loop's variables are rebound through, in
    /// parameter order.
    vars: Vec<ValueId>,
}

struct FnLower<'a> {
    tables: &'a Tables,
    program: &'a Program,
    types: &'a mut Types,
    entries: &'a [usize],
    locals: &'a [typed::Local],
    ret: Ty,
    code: Code,
    cur: BlockId,
    /// The value each local currently holds. A local is bound once by a
    /// parameter, a `let` or a pattern, at a point that dominates every read
    /// of it — which is what lexical scope means and is why this is a table
    /// rather than a stack of scopes.
    env: Vec<Option<ValueId>>,
    loops: Vec<LoopFrame>,
    /// The shared block for a refutable pattern that matched nothing, built
    /// once per function and only where something needs it.
    unmatched: Option<BlockId>,
    /// Where `middle::rc` wants an `incref` or a `decref`.
    sites: Sites,
    /// What each node evaluated to, for the sites that name a temporary
    /// rather than a local.
    node_values: HashMap<rc::NodeId, ValueId>,
}

impl FnLower<'_> {
    // -- the shape of a function --------------------------------------------

    fn func(
        &mut self,
        sig: &Signature,
        params: &[LocalId],
        body: Option<&Expr>,
        dispatch: bool,
    ) -> Code {
        let entry = self.code.block(&sig.params);
        self.cur = entry;
        let mut args: Vec<ValueId> = self.code.get(entry).params.clone();
        let dispatch = if dispatch && !args.is_empty() { Some(args.remove(0)) } else { None };
        for (local, value) in params.iter().zip(args.iter()) {
            self.bind(*local, *value);
        }

        match body {
            Some(e) => {
                let v = self.body(e, params, dispatch);
                self.set_term(Term::Return(vec![v]));
            }
            // A function nothing built: a trait method with no impl, or a
            // build that stopped after a diagnostic. Reaching one is a
            // compiler bug, so it aborts where it is rather than returning a
            // value at whatever type the caller expected.
            None => {
                self.abort("this function was never built");
            }
        }

        let mut code = std::mem::take(&mut self.code);
        code.retain_reachable();
        code
    }

    /// The body, with a `Loop` at the root recognised: its header parameters
    /// are the function's parameters, rebound per iteration.
    fn body(&mut self, e: &Expr, params: &[LocalId], dispatch: Option<ValueId>) -> ValueId {
        let ExprKind::Loop { entries } = &e.kind else {
            return self.expr(e);
        };
        let ret = self.ret.clone();
        let result = self.type_of(&ret);
        let var_types: Vec<Type> = params.iter().map(|p| self.local_type(*p)).collect();

        let mut header_types: Vec<Type> = Vec::new();
        let multi = entries.len() > 1;
        if multi {
            header_types.push(Type::I32);
        }
        header_types.extend(var_types.iter().copied());
        let header = self.code.block(&header_types);
        let join = self.code.block(&[result]);

        let mut header_params = self.code.get(header).params.clone();
        let header_dispatch =
            if multi && !header_params.is_empty() { Some(header_params.remove(0)) } else { None };

        // Into the loop, with the parameters the function was called with —
        // and, for a merged group, the entry its caller chose.
        let mut first: Vec<ValueId> = Vec::new();
        if multi {
            let d = match dispatch {
                Some(d) => d,
                None => self.constant(Type::I32, Const::Int { bits: 0, negative: false }),
            };
            first.push(d);
        }
        for p in params {
            first.push(self.read(*p));
        }
        self.set_term(Term::Jump(Target::new(header, first)));

        for (local, value) in params.iter().zip(header_params.iter()) {
            self.bind(*local, *value);
        }
        self.loops.push(LoopFrame {
            header,
            dispatch: header_dispatch,
            vars: header_params.clone(),
        });

        // The header selects an entry. One entry is a jump; a merged group is
        // a switch over the dispatch parameter, total by construction because
        // the only producer of an index is this pass.
        let entry_blocks: Vec<BlockId> =
            entries.iter().map(|_| self.code.block(&[])).collect::<Vec<_>>();
        self.cur = header;
        match (header_dispatch, entry_blocks.as_slice()) {
            (_, [only]) => self.set_term(Term::Jump(Target::to(*only))),
            (Some(d), blocks) => {
                let cases: Vec<(u64, Target)> = blocks
                    .iter()
                    .enumerate()
                    .map(|(i, b)| (i as u64, Target::to(*b)))
                    .collect();
                self.set_term(Term::Switch { on: d, cases, default: None });
            }
            (None, _) => self.set_term(Term::Unreachable),
        }

        for (entry, block) in entries.iter().zip(entry_blocks.iter()) {
            self.cur = *block;
            let v = self.expr(entry);
            self.set_term(Term::Jump(Target::new(join, vec![v])));
        }
        self.loops.pop();

        self.cur = join;
        *self.code.get(join).params.first().or_ice("the join block was built with one parameter")
    }

    // -- the builder --------------------------------------------------------

    fn type_of(&mut self, ty: &Ty) -> Type {
        self.types.of(self.tables, ty)
    }

    fn type_id(&mut self, ty: &Ty) -> TypeId {
        self.types.intern(self.tables, ty)
    }

    fn local_type(&mut self, l: LocalId) -> Type {
        let ty = self.locals.get(l.index()).map(|x| x.ty.clone()).unwrap_or(Ty::Unit);
        self.type_of(&ty)
    }

    fn bind(&mut self, l: LocalId, v: ValueId) {
        if let Some(slot) = self.env.get_mut(l.index()) {
            *slot = Some(v);
        }
    }

    fn read(&mut self, l: LocalId) -> ValueId {
        match self.env.get(l.index()).copied().flatten() {
            Some(v) => v,
            // Not reachable from any input: every local is bound by a
            // parameter, a `let` or a pattern before the scope that reads it,
            // which the checker enforced long before this pass.
            None => {
                let ty = self.local_type(l);
                self.constant(ty, Const::Undef)
            }
        }
    }

    fn block(&mut self, params: &[Type]) -> BlockId {
        self.code.block(params)
    }

    fn push(&mut self, inst: Inst) {
        let cur = self.cur;
        self.code.get_mut(cur).insts.push(inst);
    }

    fn set_term(&mut self, term: Term) {
        let cur = self.cur;
        self.code.get_mut(cur).term = term;
    }

    /// Emits an instruction with one result of a known type.
    fn emit(&mut self, ty: Type, make: impl FnOnce(ValueId) -> Inst) -> ValueId {
        let dest = self.code.value(ty);
        self.push(make(dest));
        dest
    }

    fn constant(&mut self, ty: Type, value: Const) -> ValueId {
        self.emit(ty, |dest| Inst::Const { dest, value })
    }

    fn int(&mut self, ty: Type, n: usize) -> ValueId {
        self.constant(ty, Const::Int { bits: n as u128, negative: false })
    }

    /// Ends the current block with an abort and continues in a block nothing
    /// reaches, so that the caller still has a value and the dead code is
    /// deleted with the block.
    fn abort(&mut self, message: &str) -> ValueId {
        self.push(Inst::Abort { message: message.into() });
        self.set_term(Term::Unreachable);
        let ret = self.ret.clone();
        let ty = self.type_of(&ret);
        self.dead(ty)
    }

    /// A value in a block nothing jumps to: what an expression that does not
    /// answer answers.
    fn dead(&mut self, ty: Type) -> ValueId {
        let b = self.block(&[ty]);
        self.cur = b;
        *self.code.get(b).params.first().or_ice("the block was built with one parameter")
    }

    /// The block a refutable pattern falls off the end into.
    ///
    /// Exhaustiveness is proved before this pass (`exhaustiveness.rs`), so
    /// nothing reaches it; it exists because the last arm of a `match` needs
    /// somewhere to send a failed test, and an abort says what happened if the
    /// proof and the lowering ever disagree.
    fn unmatched(&mut self) -> BlockId {
        if let Some(b) = self.unmatched {
            return b;
        }
        let here = self.cur;
        let b = self.block(&[]);
        self.cur = b;
        self.push(Inst::Abort { message: "no arm of this match applied".into() });
        self.set_term(Term::Unreachable);
        self.cur = here;
        self.unmatched = Some(b);
        b
    }

    // -- statements ---------------------------------------------------------

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let { pattern, value, .. } => {
                // The value node's `After` operations are emitted once the
                // pattern has bound, rather than where [`FnLower::expr`] would
                // put them. `middle::rc` keys the drop of a binding nothing
                // reads on the value's node (`Scan::expr`'s `Stmt::Let` arm)
                // and [`FnLower::rc`] skips a site naming a local nothing has
                // bound yet, so in the other order that drop was silently
                // dropped itself — one leaked block per `let` whose binding is
                // never read. `rc`'s own balance checker replays the statement
                // in this order and says so.
                let node = self.sites.id_of(value);
                if let Some(n) = node {
                    self.rc(n, false);
                }
                let v = self.expr_inner(value);
                if let Some(n) = node {
                    self.node_values.insert(n, v);
                }
                let fail = self.unmatched();
                self.pattern(v, pattern, fail);
                if let Some(n) = node {
                    self.rc(n, true);
                }
            }
            Stmt::Expr(e) => {
                let _ = self.expr(e);
            }
        }
    }

    // -- expressions --------------------------------------------------------

    fn exprs(&mut self, es: &[Expr]) -> Vec<ValueId> {
        es.iter().map(|e| self.expr(e)).collect()
    }

    /// Lowers one expression, with the reference-count operations
    /// `middle::rc` placed at it.
    ///
    /// Before the node's own code, and after its value exists: the two
    /// positions `rc::Position` names, and the reason this wrapper exists
    /// rather than a call at every arm of the match below.
    fn expr(&mut self, e: &Expr) -> ValueId {
        let node = self.sites.id_of(e);
        if let Some(n) = node {
            self.rc(n, false);
        }
        let v = self.expr_inner(e);
        if let Some(n) = node {
            self.node_values.insert(n, v);
            self.rc(n, true);
        }
        v
    }

    /// Emits the reference operations at one node and position, in plan order.
    ///
    /// A site naming a local nothing has bound yet, or a node whose value this
    /// pass never produced — a lambda's body, which `middle::closures` lifts
    /// and this pass does not descend into — is skipped rather than guessed
    /// at. `drop` is left `None`: the per-type drop glue is generated from the
    /// layout table, which this pass does not have (`rc.rs`, the contract).
    fn rc(&mut self, node: rc::NodeId, after: bool) {
        let sites: Vec<rc::Site> = self.sites.get(node, after).to_vec();
        for site in sites {
            let value = match site.target {
                rc::Target::Local(l) => self.env.get(l.index()).copied().flatten(),
                rc::Target::Node(n) => self.node_values.get(&n).copied(),
            };
            let Some(value) = value else { continue };
            self.push(match site.op {
                rc::RcOp::IncRef => Inst::IncRef { value },
                rc::RcOp::DecRef => Inst::DecRef { value, drop: None },
            });
        }
    }

    fn expr_inner(&mut self, e: &Expr) -> ValueId {
        let ty = self.type_of(&e.ty);
        match &e.kind {
            ExprKind::Int(v, negative) => {
                self.constant(ty, Const::Int { bits: *v, negative: *negative })
            }
            ExprKind::Float(v) => {
                // A literal takes its type from context, so an `F32` one is
                // the binary32 value it denotes — the same narrowing the
                // JavaScript backend does, so the two agree on the digits.
                let v = match self.tables.as_prim(&e.ty) {
                    Some(Prim::F32) => f64::from(*v as f32),
                    _ => *v,
                };
                self.constant(ty, Const::Float(v))
            }
            ExprKind::Str(s) => self.constant(ty, Const::Str(s.clone())),
            ExprKind::Char(c) => self.constant(ty, Const::Char(*c)),
            ExprKind::Bool(b) => self.constant(ty, Const::Bool(*b)),
            ExprKind::Unit => self.constant(ty, Const::Unit),
            ExprKind::Local(l) => self.read(*l),

            // Three shapes monomorphization removes: a module-level `let` is inlined at
            // its use, a trait call is resolved to a direct one, and a context
            // constructor becomes a call to the function that builds it. One
            // that survives means this tree did not go through
            // `monomorphize::run`, which is a compiler bug and not an input.
            ExprKind::Const(_) | ExprKind::CallTrait { .. } | ExprKind::CtxCall { .. } => {
                self.abort("this expression was not monomorphized")
            }
            ExprKind::Error => self.abort("this expression did not check"),

            ExprKind::FnRef(callee) => match callee.func() {
                Some(f) => self.emit(ty, |dest| Inst::MakeClosure { dest, func: f, env: None }),
                None => self.abort("this function reference was not monomorphized"),
            },
            ExprKind::CallFn { func, args } => match func.func() {
                Some(f) => {
                    let args = self.exprs(args);
                    self.emit(ty, |dest| Inst::Call { dests: vec![dest], func: f, args })
                }
                None => self.abort("this call was not monomorphized"),
            },
            ExprKind::CallValue { callee, args } => {
                let c = self.expr(callee);
                let args = self.exprs(args);
                self.emit(ty, |dest| Inst::CallIndirect { dests: vec![dest], callee: c, args })
            }
            ExprKind::Intrinsic { name, args, .. } => self.intrinsic(ty, name, args),

            ExprKind::StructLit { fields, .. } => {
                let fields = self.exprs(fields);
                self.emit(ty, |dest| Inst::MakeStruct { dest, fields })
            }
            ExprKind::StructUpdate { con, base, updates } => {
                // `..base` is evaluated first and once, and the replacements
                // run in field order — which is what the struct literal this
                // is shorthand for would have done.
                let b = self.expr(base);
                let arity = self.tables.tycon(*con).fields().len();
                let mut fields = Vec::with_capacity(arity);
                for i in 0..arity {
                    match updates.iter().find(|(j, _)| *j == i) {
                        Some((_, v)) => {
                            let v = self.expr(v);
                            fields.push(v);
                        }
                        None => {
                            let f = self.field_type(&base.ty, i);
                            fields.push(
                                self.emit(f, |dest| Inst::GetField {
                                    dest,
                                    agg: b,
                                    index: i as u32,
                                }),
                            );
                        }
                    }
                }
                self.emit(ty, |dest| Inst::MakeStruct { dest, fields })
            }
            ExprKind::EnumLit { variant, args, .. } => {
                let fields = self.exprs(args);
                let variant = *variant as u32;
                self.emit(ty, |dest| Inst::MakeEnum { dest, variant, fields })
            }
            ExprKind::Tuple(xs) => {
                let fields = self.exprs(xs);
                self.emit(ty, |dest| Inst::MakeStruct { dest, fields })
            }
            ExprKind::Array(xs) => {
                let elems = self.exprs(xs);
                self.emit(ty, |dest| Inst::MakeArray { dest, elems })
            }

            ExprKind::Field { base, index } | ExprKind::TupleIndex { base, index } => {
                let b = self.expr(base);
                let index = *index as u32;
                self.emit(ty, |dest| Inst::GetField { dest, agg: b, index })
            }
            ExprKind::Index { base, index, elem } => self.index(ty, base, index, elem),

            ExprKind::Block { stmts, tail } => {
                for s in stmts {
                    self.stmt(s);
                }
                match tail {
                    Some(t) => self.expr(t),
                    None => self.constant(ty, Const::Unit),
                }
            }
            ExprKind::If { cond, then, else_ } => {
                let c = self.expr(cond);
                let then_b = self.block(&[]);
                let else_b = self.block(&[]);
                let join = self.block(&[ty]);
                self.set_term(Term::Branch {
                    cond: c,
                    then: Target::to(then_b),
                    else_: Target::to(else_b),
                });
                for (block, arm) in [(then_b, then), (else_b, else_)] {
                    self.cur = block;
                    let v = self.expr(arm);
                    self.set_term(Term::Jump(Target::new(join, vec![v])));
                }
                self.cur = join;
                *self.code.get(join).params.first().or_ice("the join takes one parameter")
            }
            ExprKind::Match { scrutinee, arms } => self.match_(ty, scrutinee, arms),

            ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
                let is_and = matches!(e.kind, ExprKind::And { .. });
                let l = self.expr(lhs);
                let rhs_b = self.block(&[]);
                let short_b = self.block(&[]);
                let join = self.block(&[Type::I1]);
                let (then, else_) = if is_and {
                    (Target::to(rhs_b), Target::to(short_b))
                } else {
                    (Target::to(short_b), Target::to(rhs_b))
                };
                self.set_term(Term::Branch { cond: l, then, else_ });

                self.cur = short_b;
                let short = self.constant(Type::I1, Const::Bool(!is_and));
                self.set_term(Term::Jump(Target::new(join, vec![short])));

                self.cur = rhs_b;
                let r = self.expr(rhs);
                self.set_term(Term::Jump(Target::new(join, vec![r])));

                self.cur = join;
                *self.code.get(join).params.first().or_ice("the join takes one parameter")
            }
            ExprKind::Coalesce { lhs, rhs, kind } => {
                let l = self.expr(lhs);
                let held = self.held_variant(&lhs.ty, *kind);
                let ok = self.tag_is(l, held);
                let held_b = self.block(&[]);
                let rhs_b = self.block(&[]);
                let join = self.block(&[ty]);
                self.set_term(Term::Branch {
                    cond: ok,
                    then: Target::to(held_b),
                    else_: Target::to(rhs_b),
                });

                self.cur = held_b;
                let payload = self.emit(ty, |dest| Inst::GetPayload {
                    dest,
                    agg: l,
                    variant: held,
                    index: 0,
                });
                self.set_term(Term::Jump(Target::new(join, vec![payload])));

                self.cur = rhs_b;
                let r = self.expr(rhs);
                self.set_term(Term::Jump(Target::new(join, vec![r])));

                self.cur = join;
                *self.code.get(join).params.first().or_ice("the join takes one parameter")
            }
            ExprKind::Try { base, kind } => self.try_(ty, base, *kind),

            ExprKind::Prim { op, prim, args } => self.prim(ty, *op, *prim, args),
            ExprKind::StructuralEq { negate, args } => {
                let op = if *negate { StructuralOp::Ne } else { StructuralOp::Eq };
                self.structural(ty, op, args)
            }
            ExprKind::StructuralCmp { args, .. } => self.structural(ty, StructuralOp::Cmp, args),
            ExprKind::Template { parts } => self.template(ty, parts),

            ExprKind::CtxLit { bindings } => {
                let fields = bindings.iter().map(|(_, v)| self.expr(v)).collect();
                self.emit(ty, |dest| Inst::MakeStruct { dest, fields })
            }
            ExprKind::CtxGet { base, trait_id } => {
                let b = self.expr(base);
                let slot = match &base.ty {
                    Ty::Ctx(id) => self
                        .program
                        .ctx_layouts
                        .get(id)
                        .and_then(|l| l.iter().position(|t| t == trait_id))
                        .unwrap_or(0),
                    _ => 0,
                };
                let index = slot as u32;
                self.emit(ty, |dest| Inst::GetField { dest, agg: b, index })
            }

            // What `middle::closures` leaves in a lambda's place: a code
            // pointer and the captured values. The environment is a record of
            // exactly those values, and its layout is a tuple's — fields in
            // capture order, at natural alignment (VALUE-MODEL.md §5) — so the
            // type it is built at is the tuple of their types rather than a
            // synthesized declaration the type tables would have to carry.
            ExprKind::Closure { func, env } => {
                let env_ty = Ty::Tuple(env.iter().map(|e| e.ty.clone()).collect());
                let fields = self.exprs(env);
                let func = *func;
                if fields.is_empty() {
                    return self.emit(ty, |dest| Inst::MakeClosure { dest, func, env: None });
                }
                let record = self.type_of(&env_ty);
                let env = self.emit(record, |dest| Inst::MakeStruct { dest, fields });
                self.emit(ty, |dest| Inst::MakeClosure { dest, func, env: Some(env) })
            }
            // A lambda `middle::closures` did not lift. Lifting it here would
            // be closure conversion written a second time, in the file that
            // must not own it.
            ExprKind::Lambda { .. } => {
                self.abort("a lambda reached lowering; `middle::closures` lifts it")
            }

            // A nested loop is not a shape `tail_calls` produces — a `Loop` is
            // always a whole body — but lowering it costs one line and means
            // the pass is not the reason it could not be.
            ExprKind::Loop { .. } => {
                let params: Vec<LocalId> = Vec::new();
                self.body(e, &params, None)
            }
            ExprKind::Continue { func, entry, args } => self.continue_(ty, *func, *entry, args),
        }
    }

    // -- the pieces ---------------------------------------------------------

    /// `xs[i]`, which answers `Option<T>` and cannot go out of bounds
    /// (`list.buri:24-27`). The check is two comparisons and a branch here
    /// rather than a runtime call, so that the optimizer can see it: a bound
    /// it can prove is a bound it can delete.
    fn index(&mut self, ty: Type, base: &Expr, index: &Expr, elem: &Ty) -> ValueId {
        let xs = self.expr(base);
        let i = self.expr(index);
        let len = self.emit(Type::I64, |dest| Inst::ArrayLen { dest, array: xs });
        let zero = self.int(Type::I64, 0);
        let low = self.emit(Type::I1, |dest| Inst::Binary {
            dest,
            op: BinOp::Ge,
            prim: Prim::I64,
            lhs: i,
            rhs: zero,
        });
        let high_b = self.block(&[]);
        let some_b = self.block(&[]);
        let none_b = self.block(&[]);
        let join = self.block(&[ty]);
        self.set_term(Term::Branch {
            cond: low,
            then: Target::to(high_b),
            else_: Target::to(none_b),
        });

        self.cur = high_b;
        let high = self.emit(Type::I1, |dest| Inst::Binary {
            dest,
            op: BinOp::Lt,
            prim: Prim::I64,
            lhs: i,
            rhs: len,
        });
        self.set_term(Term::Branch {
            cond: high,
            then: Target::to(some_b),
            else_: Target::to(none_b),
        });

        self.cur = some_b;
        let elem_ty = self.type_of(elem);
        let v = self.emit(elem_ty, |dest| Inst::ArrayGet { dest, array: xs, index: i });
        let some = self.emit(ty, |dest| Inst::MakeEnum { dest, variant: 0, fields: vec![v] });
        self.set_term(Term::Jump(Target::new(join, vec![some])));

        self.cur = none_b;
        let none = self.emit(ty, |dest| Inst::MakeEnum { dest, variant: 1, fields: Vec::new() });
        self.set_term(Term::Jump(Target::new(join, vec![none])));

        self.cur = join;
        *self.code.get(join).params.first().or_ice("the join takes one parameter")
    }

    /// `?`, the only early exit in the language (SPEC 6.8).
    ///
    /// The failure arm returns a *rebuilt* value rather than the one it
    /// matched, because natively `Result<T, E>` and `Result<U, E>` are
    /// different types with different layouts — the JavaScript backend can
    /// return what it matched (`generate.rs:2074`) only because there both are
    /// the same array.
    fn try_(&mut self, ty: Type, base: &Expr, kind: OptionOrResult) -> ValueId {
        let v = self.expr(base);
        let held = self.held_variant(&base.ty, kind);
        let ok = self.tag_is(v, held);
        let held_b = self.block(&[]);
        let fail_b = self.block(&[]);
        self.set_term(Term::Branch {
            cond: ok,
            then: Target::to(held_b),
            else_: Target::to(fail_b),
        });

        // The cold arm: `.None` and `.Err(e)` both leave the function
        // (CODEGEN-LLVM.md §6 marks this block cold).
        self.cur = fail_b;
        let ret = self.ret.clone();
        let ret_ty = self.type_of(&ret);
        let out = match kind {
            OptionOrResult::Option => {
                let variant = self.variant_of(&ret, "None", 1);
                self.emit(ret_ty, |dest| Inst::MakeEnum { dest, variant, fields: Vec::new() })
            }
            OptionOrResult::Result => {
                let from = self.variant_of(&base.ty, "Err", 1);
                let err_ty = self.payload_type(&base.ty, from, 0);
                let e = self.emit(err_ty, |dest| Inst::GetPayload {
                    dest,
                    agg: v,
                    variant: from,
                    index: 0,
                });
                let variant = self.variant_of(&ret, "Err", 1);
                self.emit(ret_ty, |dest| Inst::MakeEnum { dest, variant, fields: vec![e] })
            }
        };
        self.set_term(Term::Return(vec![out]));

        self.cur = held_b;
        self.emit(ty, |dest| Inst::GetPayload { dest, agg: v, variant: held, index: 0 })
    }

    fn prim(&mut self, ty: Type, op: PrimOp, prim: Prim, args: &[Expr]) -> ValueId {
        let vs = self.exprs(args);
        let unary = match op {
            PrimOp::Neg => Some(UnOp::Neg),
            PrimOp::Not => Some(UnOp::Not),
            PrimOp::BitNot => Some(UnOp::BitNot),
            _ => None,
        };
        if let Some(op) = unary {
            let Some(arg) = vs.first().copied() else {
                return self.abort("a unary operation with no operand");
            };
            return self.emit(ty, |dest| Inst::Unary { dest, op, prim, arg });
        }
        let op = match op {
            PrimOp::Add => BinOp::Add,
            PrimOp::Sub => BinOp::Sub,
            PrimOp::Mul => BinOp::Mul,
            PrimOp::Div => BinOp::Div,
            PrimOp::Rem => BinOp::Rem,
            PrimOp::BitAnd => BinOp::BitAnd,
            PrimOp::BitOr => BinOp::BitOr,
            PrimOp::BitXor => BinOp::BitXor,
            PrimOp::Eq => BinOp::Eq,
            PrimOp::Ne => BinOp::Ne,
            PrimOp::Lt => BinOp::Lt,
            PrimOp::Le => BinOp::Le,
            PrimOp::Gt => BinOp::Gt,
            PrimOp::Ge => BinOp::Ge,
            PrimOp::Neg | PrimOp::Not | PrimOp::BitNot => BinOp::Add,
        };
        let (Some(lhs), Some(rhs)) = (vs.first().copied(), vs.get(1).copied()) else {
            return self.abort("a binary operation with one operand");
        };
        self.emit(ty, |dest| Inst::Binary { dest, op, prim, lhs, rhs })
    }

    fn structural(&mut self, ty: Type, op: StructuralOp, args: &[Expr]) -> ValueId {
        let at = args.first().map(|a| a.ty.clone()).unwrap_or(Ty::Unit);
        let at = self.type_id(&at);
        let args = self.exprs(args);
        self.emit(ty, |dest| Inst::Structural { dest, op, ty: at, args })
    }

    /// An operation the runtime supplies.
    ///
    /// The `structural*` family is the exception: it is emitted with a
    /// descriptor index as its last argument (`monomorphize.rs:815-840`), and
    /// natively no descriptor reaches the artifact at all (VALUE-MODEL.md §9).
    /// So the descriptor is dropped and the operation becomes an
    /// [`Inst::Structural`] at the type, which `middle::derives` turns into a
    /// call to the function it generated for that type.
    fn intrinsic(&mut self, ty: Type, name: &str, args: &[Expr]) -> ValueId {
        let structural = match name {
            "structuralEq" => Some(StructuralOp::Eq),
            "structuralCompare" => Some(StructuralOp::Cmp),
            "structuralShow" => Some(StructuralOp::Show),
            "structuralHash" => Some(StructuralOp::Hash),
            "structuralToJson" => Some(StructuralOp::ToJson),
            _ => None,
        };
        if let Some(op) = structural {
            let without_desc = args.split_last().map(|(_, rest)| rest).unwrap_or(&[]);
            return self.structural(ty, op, without_desc);
        }
        let key = qualified_key(self.tables, name, args);
        let args = self.exprs(args);
        self.emit(ty, |dest| Inst::CallIntrinsic { dests: vec![dest], key, args })
    }

    /// An interpolation: render every hole from its static type and join the
    /// parts (VALUE-MODEL.md §3.3).
    ///
    /// The join is `str.concat` with no context argument, because a context of
    /// zero-sized implementations is dropped from every signature
    /// (VALUE-MODEL.md §8) and `core/host`'s are all zero-sized. A program
    /// whose allocator carries state would want the context threaded here, and
    /// the place to do that is the middle-end rewrite VALUE-MODEL.md §3.3
    /// describes — one that turns a `Template` into a `str.concat` chain in
    /// the *tree*, where the context is still in scope.
    /// # Who drops the chain
    ///
    /// `middle::rc` plans over the **tree**, and every value the join below
    /// builds — each `Show` result and each intermediate `str.concat` — is a
    /// value this pass invented, with no `typed::Expr` for a [`rc::Site`] to
    /// name. So nothing in the plan can drop them, and until this dropped them
    /// itself *every interpolation of a non-`Str` hole leaked a block, and so
    /// did every join after the first*: `"[${s}]"` in a loop grew the heap by
    /// one block an iteration, forever, in a language whose whole memory story
    /// is that it does not.
    ///
    /// The `mine` flag is what says which of them this owns. A text run is an
    /// immortal literal and a `Str` hole is a value the plan is already
    /// accounting for — dropping either here would be a double free. A `Show`
    /// result and every intermediate are this pass's, and the last `acc` is
    /// the node's own value, which the plan *does* cover: `rc::fresh` counts a
    /// `Template` as producing a new reference, so a borrowing parent drops it.
    fn template(&mut self, ty: Type, parts: &[TemplatePart]) -> ValueId {
        let mut rendered: Vec<(ValueId, bool)> = Vec::new();
        for p in parts {
            match p {
                TemplatePart::Text(t) => {
                    let v = self.constant(ty, Const::Str(t.clone()));
                    rendered.push((v, false));
                }
                TemplatePart::Hole(h) => {
                    let v = self.expr(h);
                    let is_str =
                        matches!(self.tables.as_prim(&h.ty), Some(Prim::Str | Prim::Template));
                    if is_str {
                        rendered.push((v, false));
                    } else {
                        let at = self.type_id(&h.ty);
                        let shown = self.emit(ty, |dest| Inst::Structural {
                            dest,
                            op: StructuralOp::Show,
                            ty: at,
                            args: vec![v],
                        });
                        rendered.push((shown, true));
                    }
                }
            }
        }
        let mut it = rendered.into_iter();
        let Some((mut acc, mut acc_is_mine)) = it.next() else {
            return self.constant(ty, Const::Str(String::new()));
        };
        for (next, next_is_mine) in it {
            let joined = self.emit(ty, |dest| Inst::CallIntrinsic {
                dests: vec![dest],
                key: "str.concat".into(),
                args: vec![acc, next],
            });
            // *After* the join, not before. On MEMORY.md §5.3's in-place path
            // the result is `acc`'s own block with one more count on it, so
            // this drop takes that count back and frees nothing; on the two
            // allocating paths the bytes have already been copied out. Either
            // way the operand is dead the instant the join has run, and a
            // `str.concat` chain of *k* parts ends holding exactly one block.
            if next_is_mine {
                self.push(Inst::DecRef { value: next, drop: None });
            }
            if acc_is_mine {
                self.push(Inst::DecRef { value: acc, drop: None });
            }
            acc = joined;
            acc_is_mine = true;
        }
        // `rc::fresh` counts a `Template` as producing a *new* reference, and
        // the plan drops it on that word. A chain of one part does not produce
        // one on its own — `"${s}"` renders to `s`'s own three words — so the
        // count it is about to be dropped by is taken here. On a literal, whose
        // `base` is null, this is nothing.
        if !acc_is_mine {
            self.push(Inst::IncRef { value: acc });
        }
        acc
    }

    /// `Continue`: a back edge, or a tail call into the function a merged
    /// group became.
    fn continue_(
        &mut self,
        ty: Type,
        func: Option<FuncIdx>,
        entry: usize,
        args: &[Expr],
    ) -> ValueId {
        let vs = self.exprs(args);
        match func {
            // Into the enclosing loop: rebind and jump. This is the whole of
            // tail-call elimination at this layer.
            None => {
                let Some(frame) = self.loops.last() else {
                    return self.abort("a `continue` outside a loop");
                };
                let (header, dispatch, vars) =
                    (frame.header, frame.dispatch, frame.vars.clone());
                let mut jump_args: Vec<ValueId> = Vec::new();
                if dispatch.is_some() {
                    jump_args.push(self.int(Type::I32, entry));
                }
                jump_args.extend(self.pad(&vs, &vars));
                self.set_term(Term::Jump(Target::new(header, jump_args)));
                self.dead(ty)
            }
            // Into another function's loop. It is a tail call and stays one:
            // `return_call` is deliberately not used on either backend
            // (CODEGEN-LLVM.md §5), because after
            // SCC merging the tail-call graph is a DAG and the stack is
            // bounded by its longest path.
            Some(f) => {
                let mut call_args: Vec<ValueId> = Vec::new();
                if self.entries.get(f.index()).copied().unwrap_or(0) > 1 {
                    call_args.push(self.int(Type::I32, entry));
                }
                // The same padding as a back edge, for the same reason: a
                // member forwarding into the merged function has only its own
                // parameters and the function has the widest member's.
                call_args.extend(self.pad_to(&vs, f));
                let v =
                    self.emit(ty, |dest| Inst::Call { dests: vec![dest], func: f, args: call_args });
                self.set_term(Term::Return(vec![v]));
                self.dead(ty)
            }
        }
    }

    /// The same, for a call into another function's loop, where the slots are
    /// that function's parameters rather than a block's.
    fn pad_to(&mut self, args: &[ValueId], func: FuncIdx) -> Vec<ValueId> {
        let Some(callee) = self.program.funcs.get(func.index()) else { return args.to_vec() };
        let slots: Vec<Ty> = callee
            .params
            .iter()
            .map(|p| callee.locals.get(p.index()).map(|l| l.ty.clone()).unwrap_or(Ty::Unit))
            .collect();
        slots
            .iter()
            .enumerate()
            .map(|(i, ty)| match args.get(i) {
                Some(v) => *v,
                None => {
                    let ty = self.type_of(ty);
                    self.constant(ty, Const::Undef)
                }
            })
            .collect()
    }

    /// Pads an argument list out to the parameters it is being passed to.
    ///
    /// A merged group's function has the arity of its widest member
    /// (`tail_calls::max_arity`), so a narrower member's `Continue` has fewer
    /// arguments than there are slots. The entry it selects never reads the
    /// extra ones — they are that member's parameters and it has none there —
    /// so the padding is [`Const::Undef`], which is exactly the claim "nothing
    /// reads this".
    fn pad(&mut self, args: &[ValueId], params: &[ValueId]) -> Vec<ValueId> {
        params
            .iter()
            .enumerate()
            .map(|(i, p)| match args.get(i) {
                Some(v) => *v,
                None => {
                    let ty = self.code.ty_of(*p);
                    self.constant(ty, Const::Undef)
                }
            })
            .collect()
    }

    // -- matching -----------------------------------------------------------

    fn match_(&mut self, ty: Type, scrutinee: &Expr, arms: &[Arm]) -> ValueId {
        let s = self.expr(scrutinee);
        let join = self.block(&[ty]);
        if !self.switch(s, &scrutinee.ty, arms, join) {
            self.chain(s, arms, join);
        }
        self.cur = join;
        *self.code.get(join).params.first().or_ice("the join takes one parameter")
    }

    /// The one-`switch` form: an enum match whose arms discriminate on the
    /// variant and bind, which is what `middle::decision` produces and what a
    /// jump table (the machine backend) and a `switch` (LLVM) want. Answers
    /// whether it applied.
    ///
    /// The condition is that every arm is a distinct variant with irrefutable
    /// field patterns and no guard, with at most one trailing catch-all.
    /// Anything else — a nested refutable pattern, a literal, a guard — falls
    /// back to [`FnLower::chain`], because a partial switch that then chains
    /// is two mechanisms where one will do.
    fn switch(&mut self, s: ValueId, ty: &Ty, arms: &[Arm], join: BlockId) -> bool {
        let Some(con) = ty.head() else { return false };
        let variants = self.tables.tycon(con).variants().len();
        if variants < 2 {
            return false;
        }
        let mut cases: Vec<(u32, &[FieldPat], &Expr)> = Vec::new();
        let mut default: Option<&Arm> = None;
        for (i, arm) in arms.iter().enumerate() {
            if arm.guard.is_some() || default.is_some() {
                return false;
            }
            match &arm.pattern.kind {
                PatKind::Variant { variant, fields, .. }
                    if fields.iter().all(|f| f.pattern.is_irrefutable(self.tables)) =>
                {
                    let v = *variant as u32;
                    if cases.iter().any(|(seen, _, _)| *seen == v) {
                        return false;
                    }
                    cases.push((v, fields, &arm.body));
                }
                PatKind::Wild | PatKind::Bind { sub: None, .. } => {
                    // A catch-all is only a default if it is last, and an arm
                    // after it would be unreachable anyway.
                    if i.saturating_add(1) != arms.len() {
                        return false;
                    }
                    default = Some(arm);
                }
                _ => return false,
            }
        }
        if cases.len() < 2 {
            return false;
        }

        let tag = self.emit(Type::I32, |dest| Inst::GetTag { dest, agg: s });
        let mut targets: Vec<(u64, Target)> = Vec::new();
        let mut bodies: Vec<(BlockId, u32, &[FieldPat], &Expr)> = Vec::new();
        for (variant, fields, body) in &cases {
            let b = self.block(&[]);
            targets.push((u64::from(*variant), Target::to(b)));
            bodies.push((b, *variant, fields, body));
        }
        let default_block = match default {
            Some(arm) => {
                let b = self.block(&[]);
                Some((b, arm))
            }
            // Total, because exhaustiveness was proved and every variant has
            // an arm. `Profile::defensive_aborts` is what decides whether a
            // backend emits an unreachable default anyway.
            None if cases.len() == variants => None,
            // Not total and no catch-all: exhaustiveness was proved, so this
            // is the belt rather than the braces, and it aborts.
            None => Some((self.unmatched(), arms.last().or_ice("a match has at least one arm"))),
        };
        self.set_term(Term::Switch {
            on: tag,
            cases: targets,
            default: default_block.map(|(b, _)| Target::to(b)),
        });

        for (block, variant, fields, body) in bodies {
            self.cur = block;
            for f in fields {
                // A pattern's type is already instantiated, so the projection
                // needs no substitution: the pattern is where the concrete
                // type of a payload field is written down.
                let ty = self.type_of(&f.pattern.ty);
                let v = self.emit(ty, |dest| Inst::GetPayload {
                    dest,
                    agg: s,
                    variant,
                    index: f.index as u32,
                });
                let fail = self.unmatched();
                self.pattern(v, &f.pattern, fail);
            }
            let v = self.expr(body);
            self.set_term(Term::Jump(Target::new(join, vec![v])));
        }
        if let Some((block, arm)) = default_block {
            // The catch-all block is only ours to fill when we made it; the
            // shared `unmatched` block already ends in an abort.
            if self.unmatched != Some(block) {
                self.cur = block;
                let fail = self.unmatched();
                self.pattern(s, &arm.pattern, fail);
                let v = self.expr(&arm.body);
                self.set_term(Term::Jump(Target::new(join, vec![v])));
            }
        }
        true
    }

    /// Arms in order: test, bind, guard, body. The general form, for
    /// everything the switch form declines.
    fn chain(&mut self, s: ValueId, arms: &[Arm], join: BlockId) {
        for (i, arm) in arms.iter().enumerate() {
            let last = i.saturating_add(1) == arms.len();
            let next = if last { self.unmatched() } else { self.block(&[]) };
            self.pattern(s, &arm.pattern, next);
            if let Some(g) = &arm.guard {
                let c = self.expr(g);
                let body_b = self.block(&[]);
                self.set_term(Term::Branch {
                    cond: c,
                    then: Target::to(body_b),
                    else_: Target::to(next),
                });
                self.cur = body_b;
            }
            let v = self.expr(&arm.body);
            self.set_term(Term::Jump(Target::new(join, vec![v])));
            self.cur = next;
        }
    }

    /// Tests `pat` against `val`, binding what it binds, and jumps to `fail`
    /// where it does not match. Leaves the current block as the one where the
    /// match has succeeded and every binding is in scope.
    fn pattern(&mut self, val: ValueId, pat: &Pattern, fail: BlockId) {
        match &pat.kind {
            PatKind::Wild | PatKind::Unit => {}
            PatKind::Bind { local, sub } => {
                self.bind(*local, val);
                if let Some(s) = sub {
                    self.pattern(val, s, fail);
                }
            }
            PatKind::Int(v, negative) => {
                let ty = self.type_of(&pat.ty);
                let c = self.constant(ty, Const::Int { bits: *v, negative: *negative });
                self.test_eq(val, c, &pat.ty, fail);
            }
            PatKind::Float(v) => {
                let ty = self.type_of(&pat.ty);
                let c = self.constant(ty, Const::Float(*v));
                self.test_eq(val, c, &pat.ty, fail);
            }
            PatKind::Str(s) => {
                let ty = self.type_of(&pat.ty);
                let c = self.constant(ty, Const::Str(s.clone()));
                self.test_eq(val, c, &pat.ty, fail);
            }
            PatKind::Char(c) => {
                let ty = self.type_of(&pat.ty);
                let c = self.constant(ty, Const::Char(*c));
                self.test_eq(val, c, &pat.ty, fail);
            }
            PatKind::Bool(b) => {
                let c = self.constant(Type::I1, Const::Bool(*b));
                self.test_eq(val, c, &pat.ty, fail);
            }
            PatKind::Tuple(ps) => {
                for (i, p) in ps.iter().enumerate() {
                    let ty = self.type_of(&p.ty);
                    let f = self.emit(ty, |dest| Inst::GetField {
                        dest,
                        agg: val,
                        index: i as u32,
                    });
                    self.pattern(f, p, fail);
                }
            }
            PatKind::Struct { fields, .. } => {
                for f in fields {
                    let ty = self.type_of(&f.pattern.ty);
                    let v = self.emit(ty, |dest| Inst::GetField {
                        dest,
                        agg: val,
                        index: f.index as u32,
                    });
                    self.pattern(v, &f.pattern, fail);
                }
            }
            PatKind::Variant { con, variant, fields } => {
                let variant = *variant as u32;
                // A single-variant enum has nothing to fall through to, so
                // there is no tag to test — only projections.
                if self.tables.tycon(*con).variants().len() > 1 {
                    let ok = self.tag_is(val, variant);
                    let next = self.block(&[]);
                    self.set_term(Term::Branch {
                        cond: ok,
                        then: Target::to(next),
                        else_: Target::to(fail),
                    });
                    self.cur = next;
                }
                for f in fields {
                    let ty = self.type_of(&f.pattern.ty);
                    let v = self.emit(ty, |dest| Inst::GetPayload {
                        dest,
                        agg: val,
                        variant,
                        index: f.index as u32,
                    });
                    self.pattern(v, &f.pattern, fail);
                }
            }
            PatKind::Array { elems, rest } => {
                let len = self.emit(Type::I64, |dest| Inst::ArrayLen { dest, array: val });
                let want = self.int(Type::I64, elems.len());
                let op = if rest.is_open() { BinOp::Ge } else { BinOp::Eq };
                let ok = self.emit(Type::I1, |dest| Inst::Binary {
                    dest,
                    op,
                    prim: Prim::I64,
                    lhs: len,
                    rhs: want,
                });
                let next = self.block(&[]);
                self.set_term(Term::Branch {
                    cond: ok,
                    then: Target::to(next),
                    else_: Target::to(fail),
                });
                self.cur = next;
                for (i, p) in elems.iter().enumerate() {
                    let ty = self.type_of(&p.ty);
                    let idx = self.int(Type::I64, i);
                    let v = self.emit(ty, |dest| Inst::ArrayGet {
                        dest,
                        array: val,
                        index: idx,
                    });
                    self.pattern(v, p, fail);
                }
                if let ArrayRest::Bound(l) = rest {
                    let ty = self.local_type(*l);
                    let from = self.int(Type::I64, elems.len());
                    let tail = self.emit(ty, |dest| Inst::ArraySlice {
                        dest,
                        array: val,
                        from,
                    });
                    self.bind(*l, tail);
                }
            }
            // Alternatives bind the same names at the same types, and each
            // binds them to values of its own — so the continuation takes them
            // as block parameters. This is the case that would force a
            // mutable slot in an IR without block arguments, and it is one
            // line of `Target` here.
            PatKind::Or(alts) => {
                let mut binds: Vec<LocalId> = Vec::new();
                if let Some(first) = alts.first() {
                    first.binds(&mut binds);
                }
                // The merge takes the first alternative's locals, so one that
                // bound locals of its own would jump carrying a value defined
                // in a block that does not dominate the read. The checker
                // gives every alternative the same locals, in whatever order
                // each writes them; this says so, where a regression is an
                // internal error rather than a miscompile the backends find.
                let mut want = binds.clone();
                want.sort_unstable();
                for alt in alts {
                    let mut theirs: Vec<LocalId> = Vec::new();
                    alt.binds(&mut theirs);
                    theirs.sort_unstable();
                    if theirs != want {
                        crate::ice!("an or-pattern's alternatives bind different locals");
                    }
                }
                let types: Vec<Type> = binds.iter().map(|b| self.local_type(*b)).collect();
                let merge = self.block(&types);
                for (i, alt) in alts.iter().enumerate() {
                    let last = i.saturating_add(1) == alts.len();
                    let next = if last { fail } else { self.block(&[]) };
                    self.pattern(val, alt, next);
                    let args: Vec<ValueId> = binds.iter().map(|b| self.read(*b)).collect();
                    self.set_term(Term::Jump(Target::new(merge, args)));
                    self.cur = next;
                }
                self.cur = merge;
                let params = self.code.get(merge).params.clone();
                for (local, value) in binds.iter().zip(params.iter()) {
                    self.bind(*local, *value);
                }
            }
            // Poison: a diagnostic was already reported, so this arm is
            // whatever keeps the CFG well formed.
            PatKind::Error => {
                self.set_term(Term::Jump(Target::to(fail)));
                let _ = self.dead(Type::Unit);
            }
        }
    }

    /// `val == c`, continuing where it holds and jumping to `fail` where it
    /// does not.
    fn test_eq(&mut self, val: ValueId, c: ValueId, ty: &Ty, fail: BlockId) {
        let ok = match self.tables.as_prim(ty) {
            Some(prim) => self.emit(Type::I1, |dest| Inst::Binary {
                dest,
                op: BinOp::Eq,
                prim,
                lhs: val,
                rhs: c,
            }),
            None => {
                let at = self.type_id(ty);
                self.emit(Type::I1, |dest| Inst::Structural {
                    dest,
                    op: StructuralOp::Eq,
                    ty: at,
                    args: vec![val, c],
                })
            }
        };
        let next = self.block(&[]);
        self.set_term(Term::Branch {
            cond: ok,
            then: Target::to(next),
            else_: Target::to(fail),
        });
        self.cur = next;
    }

    /// `tag(v) == variant`.
    fn tag_is(&mut self, v: ValueId, variant: u32) -> ValueId {
        let tag = self.emit(Type::I32, |dest| Inst::GetTag { dest, agg: v });
        let want = self.int(Type::I32, variant as usize);
        self.emit(Type::I1, |dest| Inst::Binary {
            dest,
            op: BinOp::Eq,
            prim: Prim::I32,
            lhs: tag,
            rhs: want,
        })
    }

    // -- reading the type tables --------------------------------------------

    /// The variant of `ty` with this name, or `fallback` where the type is not
    /// the enum it was expected to be — which is what a poisoned tree looks
    /// like.
    fn variant_of(&self, ty: &Ty, name: &str, fallback: u32) -> u32 {
        ty.head()
            .and_then(|c| self.tables.tycon(c).variant_index(name))
            .map(|i| i as u32)
            .unwrap_or(fallback)
    }

    /// Which variant of an `Option` or `Result` holds a value: `Some`, `Ok`.
    fn held_variant(&self, ty: &Ty, kind: OptionOrResult) -> u32 {
        match kind {
            OptionOrResult::Option => self.variant_of(ty, "Some", 0),
            OptionOrResult::Result => self.variant_of(ty, "Ok", 0),
        }
    }

    fn field_type(&mut self, ty: &Ty, index: usize) -> Type {
        let field = ty
            .head()
            .and_then(|c| self.tables.tycon(c).fields().get(index).map(|f| f.ty.clone()))
            .unwrap_or(Ty::Unit);
        let field = self.substituted(ty, field);
        self.type_of(&field)
    }

    fn payload_type(&mut self, ty: &Ty, variant: u32, index: usize) -> Type {
        let field = ty
            .head()
            .and_then(|c| self.tables.tycon(c).variants().get(variant as usize))
            .and_then(|v| v.fields.get(index).map(|f| f.ty.clone()))
            .unwrap_or(Ty::Unit);
        let field = self.substituted(ty, field);
        self.type_of(&field)
    }

    /// A declared field type, with the owning type's arguments substituted in.
    fn substituted(&self, owner: &Ty, field: Ty) -> Ty {
        match owner {
            Ty::Con(_, args) if !args.is_empty() => {
                crate::compiler::semantics::types::substitute(&field, args, None)
            }
            _ => field,
        }
    }
}

/// The intrinsic key, with the *return* type's primitive appended where the
/// operation is one of `Bounded`'s two.
///
/// `num.minValue` and `num.maxValue` take no argument at all: `Bounded`'s
/// methods reach their type through the return type (`js/intrinsics.rs`'s
/// `numeric_free` says the same thing on the other backend). By the time a
/// backend sees the call, that type is an IR scalar — `I8`, which is `I8` and
/// `U8` alike — and the two answers differ by 128.
///
/// So the key gains it, and becomes the three-segment form every other numeric
/// operation already has: `num.U8.minValue`. That is the same trick
/// [`qualified_key`] plays for `derivePrimShow`, for the same reason, and it is
/// applied here rather than in `monomorphize` because only `middle::lower`'s
/// output — which is the native backends' input and nothing else's — needs it.
fn bounded_key(tables: &Tables, key: &str, ret: &Ty) -> String {
    if !matches!(key, "num.minValue" | "num.maxValue") {
        return key.to_string();
    }
    match tables.as_prim(ret) {
        Some(p) => format!("num.{}.{}", p.name(), key.trim_start_matches("num.")),
        None => key.to_string(),
    }
}

/// The intrinsic key, with the operand's primitive appended where the operation
/// is one of `middle::derives`'s type-directed three.
///
/// `derivePrimShow`, `derivePrimHash` and `derivePrimJson` are declared over a
/// type variable and carry the operand type in `targs` (`derives.rs`'s header).
/// By the time an [`Inst::CallIntrinsic`] reaches a backend that type is gone —
/// the IR records `I64`, which is `I64` and `U64` alike — and a backend needs
/// it, because `show` of `255` at `U8` is `255` and of the same byte at `I8` is
/// `-1`.
///
/// Appending it to the key rather than adding a field to `Inst` is the smaller
/// change and the more useful one: the key is what `Backend::missing_intrinsics`
/// reports, so a program using a rendering a backend has not implemented is told
/// *which type* it could not render.
///
/// The operand is the first argument, except for `derivePrimHash`, whose
/// signature is `(U64, T) -> U64` and whose accumulator comes first.
///
/// These three keys reach no other backend: `middle::derives` runs only inside
/// `middle::native`, and the JavaScript backend still sees `structuralShow` and
/// a descriptor.
fn qualified_key(tables: &Tables, name: &str, args: &[Expr]) -> String {
    let operand = match name {
        "derivePrimShow" | "derivePrimJson" => args.first(),
        "derivePrimHash" => args.get(1),
        _ => return name.to_string(),
    };
    match operand.and_then(|a| tables.as_prim(&a.ty)) {
        Some(p) => format!("{name}.{}", p.name()),
        // A `derivePrim*` at something that is not a primitive is a bug in
        // `derives.rs` rather than in the program, and the unqualified key is
        // what reports it: no backend implements it, so it is named.
        None => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::driver;
    use crate::compiler::middle;
    use crate::compiler::middle::monomorphize;
    use crate::compiler::modules::Role;
    use crate::diagnostics::{Diagnostics, SourceMap};

    /// Compiles a snippet through the real front end and the real middle end,
    /// and lowers it.
    ///
    /// Through the ordinary driver on purpose: a lowering tested against a
    /// hand-built tree would be tested against the tree its author imagined,
    /// and the shapes that break a lowering are the ones a checker produces
    /// and nobody thought to write down.
    fn lower(text: &str) -> ir::Program {
        lower_with(text, true)
    }

    /// The same, with the inliner off.
    ///
    /// A golden print of a two-line function has to be a print of *that*
    /// function, and with the inliner on a two-line function is inlined into
    /// its caller and then dropped by `dce` — so the test would assert on an
    /// abort. The pipeline is still the real one; only the budget differs, and
    /// the tests that care about what the whole middle end produces run with
    /// it on.
    fn lower_plain(text: &str) -> ir::Program {
        lower_with(text, false)
    }

    fn lower_with(text: &str, inline: bool) -> ir::Program {
        let (program, analysis) = compiled(text, inline);
        checked(run(&program, &analysis.checked.tables))
    }

    /// The snippet, through the front end and the whole middle end, stopping
    /// before the lowering — for the tests that need to hand `run_with` a plan
    /// of their own.
    fn compiled(text: &str, inline: bool) -> (Program, driver::Analysis) {
        let mut map = SourceMap::new();
        let analysis = driver::analyze_snippet(&mut map, "test", text, Role::Entry);
        let complaints: Vec<&str> =
            analysis.diagnostics.items.iter().map(|d| d.message.as_str()).collect();
        assert!(!analysis.diagnostics.has_errors(), "snippet did not compile: {complaints:?}");
        let entry = analysis.checked.entry.expect("the snippet exports `main`");
        let module_paths: Vec<String> =
            analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
        let mut diags = Diagnostics::new();
        let mut program = monomorphize::run(
            &analysis.checked,
            module_paths,
            &mut diags,
            monomorphize::Roots::Main(entry),
        );
        assert!(!diags.has_errors(), "monomorphization failed");
        let opts = middle::Options {
            inline: crate::compiler::middle::inline::Options { inline },
        };
        middle::run(&mut program, &opts);
        middle::native(&mut program);
        (program, analysis)
    }

    /// Every test verifies what it lowered: a lowering that produced a
    /// malformed CFG and an assertion about one instruction in it would pass.
    fn checked(lowered: ir::Program) -> ir::Program {
        let problems = ir::verify(&lowered);
        if !problems.is_empty() && std::env::var("IR_DEBUG").is_ok() {
            println!("{lowered}");
        }
        assert!(problems.is_empty(), "the lowered IR is malformed:\n{}", problems.join("\n"));
        lowered
    }

    /// The function whose debug name ends with this, rendered.
    fn render(p: &ir::Program, suffix: &str) -> String {
        let f = p
            .funcs
            .iter()
            .find(|f| f.debug_name.ends_with(suffix))
            .unwrap_or_else(|| panic!("no function named `{suffix}`"));
        p.render_func(f)
    }

    /// Wraps a body in a `main` the driver will accept.
    fn program(extra: &str, body: &str) -> String {
        format!("{extra}\n\nexport fn main(): Result<(), Str> {{\n{body}\n  .Ok(())\n}}\n")
    }

    #[test]
    fn arithmetic_lowers_to_one_block() {
        let p = lower_plain(&program(
            "export fn add(a: Int, b: Int): Int { a + b }",
            "  let _ = add(1, 2);",
        ));
        assert_eq!(
            render(&p, ":add"),
            "; test:add [unit test]\n\
             fn test$add(i64, i64) -> i64 {\n\
             \x20 b0(v0: i64, v1: i64):\n\
             \x20   v2 = add.I64 v0, v1\n\
             \x20   return v2\n\
             }\n"
        );
    }

    #[test]
    fn an_if_becomes_a_branch_and_a_join_with_a_parameter() {
        let p = lower_plain(&program(
            "export fn pick(c: Bool): Int { if (c) { 1 } else { 2 } }",
            "  let _ = pick(true);",
        ));
        assert_eq!(
            render(&p, ":pick"),
            "; test:pick [unit test]\n\
             fn test$pick(i1) -> i64 {\n\
             \x20 b0(v0: i1):\n\
             \x20   branch v0, b1(), b2()\n\
             \x20 b1():\n\
             \x20   v2 = const 1\n\
             \x20   jump b3(v2)\n\
             \x20 b2():\n\
             \x20   v3 = const 2\n\
             \x20   jump b3(v3)\n\
             \x20 b3(v1: i64):\n\
             \x20   return v1\n\
             }\n"
        );
    }

    /// The property the whole design rests on: a value that differs per
    /// predecessor arrives as a block parameter, which is a phi in the other
    /// notation (CODEGEN-LLVM.md §2.1).
    #[test]
    fn every_join_takes_its_value_as_a_parameter() {
        let p = lower_plain(&program(
            "export fn pick(c: Bool): Int { if (c) { 1 } else { 2 } }",
            "  let _ = pick(false);",
        ));
        for f in &p.funcs {
            let Some(code) = f.code() else { continue };
            let preds = code.preds();
            for (i, b) in code.blocks.iter().enumerate() {
                let n = preds.get(i).map(Vec::len).unwrap_or(0);
                if n > 1 {
                    // Every predecessor supplies an argument list of exactly
                    // this block's arity — checked by `verify`, and this is
                    // the statement of *why* it matters.
                    for p in preds.get(i).map(Vec::as_slice).unwrap_or_default() {
                        let t = code
                            .get(*p)
                            .term
                            .targets()
                            .into_iter()
                            .find(|t| t.block.index() == i)
                            .expect("a predecessor names the block");
                        assert_eq!(t.args.len(), b.params.len());
                    }
                }
            }
        }
    }

    #[test]
    fn an_enum_match_becomes_one_switch() {
        let src = "
export enum Colour { Red, Green, Blue }

export fn code(c: Colour): Int {
  match (c) {
    .Red => 1,
    .Green => 2,
    .Blue => 3,
  }
}
";
        let p = lower_plain(&program(src, "  let _ = code(.Red);"));
        let text = render(&p, ":code");
        assert!(text.contains("v2 = tag v0"), "{text}");
        assert!(text.contains("switch v2, [0 -> b2(), 1 -> b3(), 2 -> b4()]"), "{text}");
        // Total, so no default: an enum's table always is (VALUE-MODEL.md §6).
        assert!(!text.contains("default"), "{text}");
    }

    #[test]
    fn a_match_with_a_guard_falls_back_to_tests_in_order() {
        let src = "
export fn classify(n: Int): Int {
  match (n) {
    0 => 10,
    x if (x > 100) => 20,
    _ => 30,
  }
}
";
        let p = lower_plain(&program(src, "  let _ = classify(1);"));
        let text = render(&p, ":classify");
        assert!(text.contains("eq.I64"), "{text}");
        assert!(text.contains("gt.I64"), "{text}");
        assert!(!text.contains("switch"), "{text}");
    }

    /// A self-recursive tail call is a loop before this pass runs
    /// (`middle::tail_calls`), so lowering sees a `Loop` and produces a header
    /// block with the parameters as block parameters and a back edge to it.
    #[test]
    fn a_tail_recursive_function_becomes_a_back_edge_to_a_header() {
        let src = "
export fn count(n: Int, acc: Int): Int {
  if (n <= 0) { acc } else { count(n - 1, acc + n) }
}
";
        let p = lower_plain(&program(src, "  let _ = count(10, 0);"));
        let text = render(&p, ":count");
        // The entry falls into a header, and the header is the back edge's
        // destination: the entry block is never a branch target, which both
        // backends require.
        assert!(text.contains("b0(v0: i64, v1: i64):\n    jump b1(v0, v1)"), "{text}");
        let f = p
            .funcs
            .iter()
            .find(|f| f.debug_name.ends_with(":count"))
            .expect("the function is in the program");
        let code = f.code().expect("it has a body");
        let preds = code.preds();
        let header = preds.get(1).cloned().unwrap_or_default();
        assert!(header.len() >= 2, "the header has the entry and a back edge: {header:?}");
        assert!(
            header.iter().any(|b| b.index() > 1),
            "one predecessor comes from later in the function: {header:?}"
        );
        // Nothing jumps to the entry.
        assert!(preds.first().map(Vec::is_empty).unwrap_or(false), "{text}");
    }

    #[test]
    fn a_question_mark_branches_and_returns_early() {
        let src = "
export fn half(n: Int): Option<Int> { if (n % 2 == 0) { .Some(n / 2) } else { .None } }

export fn quarter(n: Int): Option<Int> {
  let h = half(n)?;
  half(h)
}
";
        let p = lower_plain(&program(src, "  let _ = quarter(8);"));
        let text = render(&p, ":quarter");
        // The failure arm builds `.None` at *this* function's return type and
        // returns it, rather than passing the matched value through.
        assert!(text.contains("make #1 ()"), "{text}");
        let returns = text.matches("return").count();
        assert!(returns >= 2, "one return for the early exit and one for the value: {text}");
    }

    #[test]
    fn a_lowered_program_names_one_unit_per_module() {
        let p = lower_plain(&program(
            "export fn id(n: Int): Int { n }",
            "  let _ = id([1, 2].len());",
        ));
        assert!(p.units.iter().any(|u| u == "test"), "{:?}", p.units);
        assert!(p.units.iter().any(|u| u.starts_with("core_")), "{:?}", p.units);
        // Every function's unit is one this program declares.
        for f in &p.funcs {
            assert!((f.unit as usize) < p.units.len());
        }
    }

    #[test]
    fn an_intrinsic_is_a_runtime_symbol_and_not_a_body() {
        let p = lower(&program("", "  let _ = [1, 2].len();"));
        let runtime: Vec<&str> = p.funcs.iter().filter_map(|f| f.intrinsic_key()).collect();
        assert!(
            runtime.contains(&"list.len"),
            "`len` is supplied by the runtime, by key: {runtime:?}"
        );
        for f in &p.funcs {
            match &f.body {
                Body::Runtime(_) => assert!(f.code().is_none()),
                Body::Code(c) => assert!(!c.blocks.is_empty(), "{}", f.debug_name),
            }
        }
    }

    /// Every pattern form the checker can produce, lowered and verified.
    ///
    /// The value of this one is not the assertions at the bottom; it is that
    /// `lower` runs over an array pattern with a rest binding, an
    /// alternative pattern that binds nothing, a payload-carrying enum, a
    /// struct update and a tuple, and that the CFG it produces passes the
    /// verifier — which is the property every backend written after this
    /// depends on.
    #[test]
    fn every_pattern_form_lowers_and_verifies() {
        let src = "
export struct Point { export x: Int, export y: Int }

export enum Shape { Circle(Int), Rect(Int, Int), Empty }

export fn area(s: Shape): Int {
  match (s) {
    .Circle(r) => r * r * 3,
    .Rect(w, h) => w * h,
    .Empty => 0,
  }
}

export fn head(xs: [Int]): Int {
  match (xs) {
    [] => 0,
    [a] => a,
    [a, b, ..rest] => a + b + rest.len(),
  }
}

export fn label(n: Int): Str {
  match (n) {
    0 | 1 => \"small\",
    _ => \"big\",
  }
}

export fn moved(p: Point): Point { Point { ..p, x: p.x + 1 } }

export fn both(t: (Int, Bool)): Int { match (t) { (n, true) => n, (n, false) => 0 - n } }
";
        let p = lower_plain(&program(
            src,
            "
  let _a = area(.Rect(2, 3));
  let _h = head([1, 2, 3]);
  let _l = label(2);
  let _m = moved(Point { x: 1, y: 2 });
  let _b = both((1, true));
",
        ));
        assert!(ir::verify(&p).is_empty());
        let all: String = p.to_string();
        assert!(all.contains("payload."), "an enum arm projects its payload: {all}");
        assert!(all.contains("= len "), "an array pattern tests the length");
        assert!(all.contains("= slice "), "a rest binding slices");
        assert!(all.contains("= field."), "a struct update reads the fields it keeps");
    }

    /// A mutually tail-recursive group is one function with a dispatch
    /// parameter, and the dispatch parameter is this pass's to materialise.
    ///
    /// `middle::tail_calls` merges the group and leaves the entry index as a
    /// number on `ExprKind::Continue`, deliberately not smuggled in as an
    /// argument at a type it would have had to invent. So the merged
    /// function's signature grows a leading `i32` here, its header switches on
    /// it, and a member with fewer parameters than the widest one pads the
    /// slots it has nothing for.
    #[test]
    fn a_merged_tail_call_group_switches_on_a_dispatch_parameter() {
        let src = "
export fn walk(n: Int, acc: Int): Int {
  if (n <= 0) { acc } else { step(n - 1) }
}

export fn step(n: Int): Int {
  if (n <= 0) { 0 } else { walk(n - 1, n) }
}
";
        let p = lower_plain(&program(src, "  let _ = walk(4, 0);"));
        let merged = p
            .funcs
            .iter()
            .find(|f| f.debug_name.starts_with("tail group"))
            .expect("the group was merged into one function");
        let text = p.render_func(merged);
        assert_eq!(merged.sig.params.first(), Some(&ir::Type::I32), "{text}");
        // The entry falls into a header carrying every parameter, including
        // the index, and the header is what switches — the entry block is
        // never a branch target on either backend.
        assert!(text.contains("b0(v0: i32, v1: i64, v2: i64):\n    jump b1(v0, v1, v2)"), "{text}");
        assert!(text.contains("switch v3, [0 -> b3(), 1 -> b4()]"), "{text}");
        assert!(text.contains("const undef"), "the narrower member pads: {text}");
        // And the members forward into it rather than being deleted: an
        // `FnRef` or a non-tail call to one still has to work.
        for name in [":walk", ":step"] {
            let f = p
                .funcs
                .iter()
                .find(|f| f.debug_name.ends_with(name))
                .expect("the member kept its name");
            assert!(p.render_func(f).contains("call f"), "{name} forwards");
        }
    }

    /// `middle::rc`'s plan is placed, not ignored.
    ///
    /// The plan is built here rather than taken from `rc::analyze`, and that
    /// is the point: a test that waited for the analysis to emit a site at
    /// this program would be testing that pass rather than this contract. What
    /// is asserted is the half that lives here: a site at a node names a value,
    /// and the instruction lands at that node, before or after, in plan order.
    #[test]
    fn a_plan_site_becomes_an_instruction_at_its_node() {
        let (program, analysis) = compiled(
            &program("export fn keep(s: Str): Str { s }", "  let _ = keep(\"hi\");"),
            false,
        );
        let target = program
            .funcs
            .iter()
            .position(|f| f.debug_name.ends_with(":keep"))
            .expect("the function is in the program");

        let mut plan = rc::Plan { funcs: Vec::new(), crosses_tasks: false };
        for (i, f) in program.funcs.iter().enumerate() {
            let params = vec![Ownership::Own; f.params.len()];
            let sites = if i == target {
                let local = *f.params.first().expect("`keep` takes one parameter");
                vec![
                    rc::Site {
                        node: rc::NodeId(0),
                        at: rc::Position::Before,
                        op: rc::RcOp::IncRef,
                        target: rc::Target::Local(local),
                    },
                    rc::Site {
                        node: rc::NodeId(0),
                        at: rc::Position::After,
                        op: rc::RcOp::DecRef,
                        target: rc::Target::Node(rc::NodeId(0)),
                    },
                ]
            } else {
                Vec::new()
            };
            plan.funcs.push(rc::FuncPlan {
                params,
                purity: ir::Purity::Pure,
                can_abort: false,
                can_park: false,
                sites,
                reuse: Vec::new(),
                unclassified: Vec::new(),
                inherits: Vec::new(),
            });
        }

        let lowered = checked(run_with(&program, &analysis.checked.tables, &plan));
        let f = lowered
            .funcs
            .iter()
            .find(|f| f.debug_name.ends_with(":keep"))
            .expect("the function survived lowering");
        assert_eq!(
            lowered.render_func(f),
            "; test:keep [unit test]\n\
             fn test$keep(Str) -> Str {\n\
             \x20 b0(v0: Str):\n\
             \x20   incref v0\n\
             \x20   decref v0\n\
             \x20   return v0\n\
             }\n"
        );
        // And the facts come from the plan rather than from the conservative
        // default `ir::Facts` documents.
        assert_eq!(f.facts.purity, ir::Purity::Pure);
        assert!(!f.facts.can_abort);
    }

    /// A `let` whose binding nothing reads is dropped where it was bound.
    ///
    /// `middle::rc` keys that drop on the *value's* node and [`FnLower::rc`]
    /// skips a site naming a local nothing has bound yet, so emitting the
    /// node's `After` operations before `pattern` ran threw it away — one
    /// leaked block per unread binding, and `rc`'s own balance checker had
    /// said all along that the binding exists first.
    #[test]
    fn a_binding_nothing_reads_is_still_dropped() {
        let p = lower_plain(&program(
            "
from \"core/effect/lib.buri\" import { Alloc };
from \"core/host/lib.buri\" import * as host;

export fn junk<C: Alloc>(ctx: C, n: Int): Int {
  let s = \"z\".repeat(ctx, n);
  n
}
",
            "  let _ = junk(context { Alloc: host.alloc }, 4);",
        ));
        assert_eq!(
            render(&p, ":junk"),
            "; test:junk [unit test]\n\
             fn test$junk$72mdf3(a context, i64) -> i64 {\n\
             \x20 b0(v0: a context, v1: i64):\n\
             \x20   v2 = const \"z\"\n\
             \x20   v3 = call fn core_str_lib_buri$Str_repeat$72mdf3(v2, v0, v1)\n\
             \x20   decref v3\n\
             \x20   decref v2\n\
             \x20   return v1\n\
             }\n"
        );
    }

    /// The whole standard library a program touches, lowered and verified.
    /// This is the test that finds the expression shape nobody thought of.
    #[test]
    fn the_standard_library_a_program_reaches_lowers_and_verifies() {
        let p = lower(&program(
            "
export struct Point { export x: Int, export y: Int }

export fn sum(xs: [Int]): Int { xs.fold(fn(a, b) => a + b, 0) }
",
            "
  let pt = Point { x: 1, y: 2 };
  let _s = \"${pt.x}-${pt.y}\";
  let _n = sum([1, 2, 3]);
  let _o = [1, 2, 3].get(1);
",
        ));
        assert!(ir::verify(&p).is_empty());
        // Both kinds of body reach the backend: generated code, and a
        // runtime symbol.
        assert!(p.funcs.iter().any(|f| f.code().is_some()));
        assert!(p.funcs.iter().any(|f| f.intrinsic_key().is_some()));
    }

}
