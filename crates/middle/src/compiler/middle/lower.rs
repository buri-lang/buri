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
//!   supplies a generated `show`/`decode` at the type instead, as it does for
//!   every structural operation.

use crate::compiler::backend::runtime_table::{self, Ret};
use crate::compiler::middle::ir::{
    self, Adopt, Agg, Answer, BinOp, BlockId, Body, Code, Const, EnumTy, Facts, FnTy, Func, Inst,
    IntrinsicId, ListTy, Ownership, Passing, RecTy, Remap, ResTy, RuntimeId, Signature, Target,
    Term, Type, TypeList, UnOp, ValueId,
};
use crate::compiler::middle::layout::{self, EnumHead};
use crate::compiler::middle::monomorphize::{FuncKind, Program, ProgramRoots};
use crate::compiler::middle::rc;
use crate::compiler::semantics::layered::{Dense, DenseId};
use crate::compiler::semantics::typed::{
    self, Arm, ArrayRest, Expr, ExprKind, FieldPat, Magnitude, NumberLit, OptionOrResult, PatKind,
    Pattern, PrimOp, Stmt, TemplatePart, UnaryOp,
};
use crate::compiler::semantics::types::{
    FieldId, FuncIdx, LocalId, Prim, Tables, Ty, TyConId, TyDef, TyKind, VariantId,
};
use crate::diagnostics::Invariant as _;
use crate::hash::Map as HashMap;

mod lists;

/// The most parts a template joins by a chain of `str.concat`s. One with more
/// is joined by the runtime in one call ([`FnLower::template_joined`]).
pub(crate) const CONCAT_CHAIN_MAX: usize = 16;

/// The most parts one `list.join` of a long template takes. A longer one is
/// joined in pieces of this many ([`FnLower::template_joined`]).
pub(crate) const JOIN_PIECE: usize = 32;

/// Whether lowering builds `key`'s body itself, as IR, so that no backend
/// meets it as a call: `core/list`'s closure operations and the loops the
/// derives use (`lower/lists.rs`).
pub fn lowers(key: &str) -> bool {
    lists::handles(key)
}

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
    let units = Units::assign(program);
    // How many loop entries each function has, which is how a `Continue` into
    // another function knows whether to pass a dispatch index. Computed for
    // the whole program first because a forwarder is lowered before the merged
    // function it names.
    let entries: Dense<FuncIdx, usize> = program.funcs.map(|f| loop_entries(f.body()));
    let counts = lists::Counts::new(program);
    let cycles = layout::Cycles::new(tables);

    // Lower every function across the cores. Each gets a type interner of its
    // own — the one piece of cross-function state — and its `TypeId`s are made
    // whole-program below. `parallel::map` returns the results in index order,
    // so `funcs` is the same vector the serial loop built.
    let funcs: Vec<_> = program.funcs.entries().collect();
    let lowered: Vec<(Func, TypeList, Keys)> = crate::parallel::map_items(&funcs, |_, &(i, f)| {
        let mut types = Types::default();
        let mut keys = Keys::default();
        let func = lower_one(
            program, tables, plan, &counts, &cycles, &entries, &units, &mut types, &mut keys, i, f,
        );
        (func, types.list, keys)
    });

    // Fold the per-function type tables into one, in function order, and remap
    // each function's `TypeId`s onto it. The global interner meets function 0's
    // types first, then function 1's new ones, and so on — which is the exact
    // order a single shared interner met them in, so the emitted type table and
    // every id in it are byte-for-byte the serial ones. The intrinsic keys
    // fold the same way, for the same reason.
    let mut types = Types::named();
    let mut keys = Keys::default();
    let funcs: Dense<FuncIdx, Func> = lowered
        .into_iter()
        .map(|(mut func, local, local_keys)| {
            let remap = types.adopt(tables, local);
            remap_func_types(&mut func, &remap);
            keys.adopt(&mut func, &local_keys, &remap);
            func
        })
        .collect();
    let roots = match &program.roots {
        ProgramRoots::Main(func) => ir::Roots::Main(ir::Main {
            func: *func,
            ret: types.enum_of(tables, &returns(program.funcs.get(*func)), TyConId::RESULT),
        }),
        ProgramRoots::Tests(tests) => ir::Roots::Tests(tests.iter().map(|t| t.func).collect()),
    };

    ir::Program {
        funcs,
        units: units.into_units(),
        types: types.list,
        crosses_tasks: plan.crosses_tasks,
        cell_equal: program.cell_equal.clone(),
        intrinsics: keys.calls.keys,
        runtimes: keys.runtimes.keys,
        roots,
    }
}

/// Lowers function `i`, `f`, interning its types into `types`.
///
/// Pure in `i`: it reads the program, the plan and the two whole-program tables,
/// and writes only its own `types`. That is what lets [`run_with`] run it over
/// the cores; the `TypeId`s it mints are local to `types` and remapped there.
#[allow(
    clippy::too_many_arguments,
    reason = "the whole-program tables one lowering reads"
)]
fn lower_one(
    program: &Program,
    tables: &Tables,
    plan: &rc::Plan,
    counts: &lists::Counts<'_>,
    cycles: &layout::Cycles,
    entries: &Dense<FuncIdx, usize>,
    units: &Units,
    types: &mut Types,
    keys: &mut Keys,
    i: FuncIdx,
    f: &crate::compiler::middle::monomorphize::Func,
) -> Func {
    let dispatch = *entries.get(i) > 1;
    let mut params = Vec::new();
    if dispatch {
        params.push(Type::I32);
    }
    for p in &f.params {
        let ty = f.locals.get(*p).ty;
        let t = types.of(tables, &ty);
        params.push(t);
    }
    let ret = returns(f);
    let sig = Signature {
        params,
        ret: types.of(tables, &ret),
    };

    let fplan = plan.func(i);
    let body = match &f.kind {
        FuncKind::Intrinsic(key) if !lists::handles(key) => {
            let key = bounded_key(tables, key, &ret);
            Body::Runtime(ir::Runtime {
                key: keys.runtimes.intern(&key),
                answer: types.answer(tables, &key, &ret, sig.ret),
            })
        }
        kind => {
            let lower = FnLower {
                tables,
                program,
                plan,
                counts,
                cycles,
                types,
                keys,
                locals: &f.locals,
                ret,
                code: Code::new(&sig.params),
                cur: BlockId::ENTRY,
                env: f.locals.map(|_| None),
                loops: Vec::new(),
                unmatched: None,
                slices: Vec::new(),
                sites: Sites::of(fplan, f.body()),
                node_values: HashMap::default(),
                #[cfg(debug_assertions)]
                debug_name: &f.debug_name,
            };
            Body::Code(match kind {
                // A `core/list` loop is a body too, for a caller that reaches
                // the function through a function value and so cannot have the
                // loop at its call site.
                FuncKind::Intrinsic(key) => lower.list_body(key, &f.params),
                FuncKind::Unbuilt | FuncKind::Body(_) => lower.func(&f.params, f.body(), dispatch),
            })
        }
    };

    Func {
        facts: facts(fplan, dispatch, &sig),
        symbol: f.symbol.clone(),
        debug_name: f.debug_name.clone(),
        env: f.env,
        sig,
        unit: units.of(i),
        body,
        span: f.span,
    }
}

/// Rewrites a function's local `TypeId`s to the whole-program ones. Covers every
/// place a `TypeId` reaches the IR: a value's or a signature slot's [`Type`].
fn remap_func_types(func: &mut Func, remap: &Remap) {
    for t in func.sig.params.iter_mut().chain([&mut func.sig.ret]) {
        remap.ty(t);
    }
    for p in &mut func.facts.params {
        *p = remap.passing(*p);
    }
    if let Body::Code(code) = &mut func.body {
        code.remap_types(remap);
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
/// The plan has one ownership per parameter, and the signature has those and
/// the entry index a merged group's dispatch adds. A borrowed parameter is a
/// counted one, which is an aggregate.
fn facts(plan: &rc::FuncPlan, dispatch: bool, sig: &Signature) -> Facts {
    let mut owns = plan.params.clone();
    if dispatch {
        owns.insert(0, Ownership::Own);
    }
    let params = owns
        .iter()
        .zip(&sig.params)
        .map(|(o, t)| match (o, t) {
            (Ownership::Borrow, Type::Agg(agg)) => Passing::Borrow(*agg),
            _ => Passing::Own,
        })
        .collect();
    Facts {
        params,
        purity: plan.purity,
        can_abort: plan.can_abort,
    }
}

/// What a function returns.
///
/// Not `monomorphize::Func::ret`, which not every pass that writes a function
/// fills in. The body's own type is the answer that is always
/// right, and for a body that is a `Loop` it is the type of an entry.
fn returns(f: &crate::compiler::middle::monomorphize::Func) -> Ty {
    let Some(body) = f.body() else { return f.ret };
    match &body.kind {
        ExprKind::Loop { entries } => entries.first().map(|e| e.ty).unwrap_or_else(|| body.ty),
        _ => body.ty,
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
    /// Each unit by name: its number, by first appearance, and its functions.
    by_name: HashMap<String, (u32, Vec<FuncIdx>)>,
    /// Each function's unit.
    by_func: Dense<FuncIdx, u32>,
}

impl Units {
    /// Assigns a number to every unit the program names, in function order —
    /// which is source order (`monomorphize.rs:247-248`), so the partition is
    /// the same on two builds of one commit.
    fn assign(program: &Program) -> Units {
        let mut units = Units::default();
        let by_func = program
            .funcs
            .entries()
            .map(|(i, f)| units.add(i, &f.debug_name))
            .collect();
        units.by_func = by_func;
        units
    }

    /// Puts function `func` in the unit its debug name names, and answers
    /// that unit's number.
    fn add(&mut self, func: FuncIdx, debug_name: &str) -> u32 {
        let next = self.by_name.len() as u32;
        let (i, funcs) = self
            .by_name
            .entry(unit_name(debug_name))
            .or_insert((next, Vec::new()));
        funcs.push(func);
        *i
    }

    fn of(&self, func: FuncIdx) -> u32 {
        *self.by_func.get(func)
    }

    /// The units, by number.
    fn into_units(self) -> Vec<ir::Unit> {
        let mut units: Vec<_> = self.by_name.into_iter().collect();
        units.sort_unstable_by_key(|(_, (i, _))| *i);
        units
            .into_iter()
            .map(|(name, (_, funcs))| ir::Unit { name, funcs })
            .collect()
    }
}

/// The unit a `module:owner.name` debug name belongs to, as a filename stem.
///
/// A merged tail-call group is named `tail group core/list:a, core/list:b`, so
/// the module is the last word before the first colon rather than everything
/// before it. A name with no colon at all has no declaring module and lands in
/// `root` — which is where a generated function with no source of its own
/// goes, and nothing else: a test body and a context constructor are both
/// qualified with the module that declares them (`monomorphize::name_of`).
fn unit_name(debug_name: &str) -> String {
    let Some((head, _)) = debug_name.split_once(':') else {
        return "root".into();
    };
    let module = head.split_whitespace().last().unwrap_or("root");
    module.replace(['/', '.'], "_")
}

// ---------------------------------------------------------------------------
// The intrinsic-key interners
// ---------------------------------------------------------------------------

/// The intrinsic keys one function names, or the whole program's.
///
/// Each function is lowered against a `Keys` of its own, across the cores, and
/// [`Keys::adopt`] folds them in function order — the order one shared
/// interner would have met them in, so the program's tables are the serial
/// ones.
#[derive(Default)]
struct Keys {
    calls: Interner<IntrinsicId>,
    runtimes: Interner<RuntimeId>,
}

impl Keys {
    /// Folds a function's own keys into these, and points its ids at them.
    /// A runtime body's answer names a type, so `remap` points it at the
    /// program's table, as [`remap_func_types`] does for a body's code.
    fn adopt(&mut self, func: &mut Func, local: &Keys, remap: &Remap) {
        let calls = local.calls.keys.map(|k| self.calls.intern(k));
        let runtimes = local.runtimes.keys.map(|k| self.runtimes.intern(k));
        match &mut func.body {
            Body::Runtime(r) => {
                r.key = *runtimes.get(r.key);
                r.answer = remap.answer(r.answer);
            }
            Body::Code(code) => code.remap_intrinsics(&calls),
        }
    }
}

/// One table of keys, each once, with the id each was given.
struct Interner<I> {
    keys: Dense<I, Box<str>>,
    ids: HashMap<Box<str>, I>,
}

impl<I> Default for Interner<I> {
    fn default() -> Self {
        Interner {
            keys: Dense::default(),
            ids: HashMap::default(),
        }
    }
}

impl<I: DenseId> Interner<I> {
    fn intern(&mut self, key: &str) -> I {
        let keys = &mut self.keys;
        *self
            .ids
            .entry(key.into())
            .or_insert_with(|| keys.push(key.into()))
    }
}

// ---------------------------------------------------------------------------
// The type interner
// ---------------------------------------------------------------------------

/// One table of types, each once, by kind. A type's kind is read off it once,
/// in [`Types::agg`]; a caller that already knows the kind asks for it by
/// kind ([`Types::enum_of`], [`Types::list_of`], [`Types::record_of`]) and
/// gets that kind's id back.
#[derive(Default)]
struct Types {
    list: TypeList,
    records: HashMap<Ty, RecTy>,
    enums: HashMap<Ty, EnumTy>,
    lists: HashMap<Ty, ListTy>,
    fns: HashMap<Ty, FnTy>,
    /// Whether an entry is named. Each function's own table is not: nothing
    /// reads a function's own names, so [`Types::adopt`] names each type once
    /// per program rather than once per function that meets it.
    named: bool,
}

impl Types {
    /// The program's table, which names what it interns.
    fn named() -> Types {
        Types {
            named: true,
            ..Types::default()
        }
    }

    fn name(&self, tables: &Tables, ty: &Ty) -> String {
        if self.named {
            crate::compiler::semantics::types::show(tables, None, &[], ty)
        } else {
            String::new()
        }
    }

    /// The aggregate `ty` is, interned.
    fn agg(&mut self, tables: &Tables, ty: &Ty) -> Agg {
        let ty = Types::runtime_ty(tables, ty);
        match ty.kind() {
            TyKind::Array(elem) => Agg::List(self.list_of(tables, *elem)),
            TyKind::Fn(..) => Agg::Fn(self.closure(tables, ty)),
            TyKind::Con(con, args) if matches!(tables.tycon(*con).def, TyDef::Enum { .. }) => {
                Agg::Enum(self.enum_head(
                    tables,
                    EnumHead {
                        ty,
                        con: *con,
                        args,
                    },
                ))
            }
            _ => Agg::Record(self.record(tables, ty)),
        }
    }

    /// How `key`'s result comes back, where the result is `ret`, which lowers
    /// to `ty`: the shape its runtime row names, or, for a key with no row,
    /// the enum the backend builds.
    ///
    /// A row's `Option` or `Result` is the enum of `ret` itself, so it is the
    /// destination's own id: `ty` was interned from the same `ret`.
    fn answer(&mut self, tables: &Tables, key: &str, ret: &Ty, ty: Type) -> Answer {
        let Some(row) = runtime_table::entry(key) else {
            return match ty {
                Type::Agg(Agg::Enum(e)) if self.list.enum_head(e).con == TyConId::RESULT => {
                    Answer::InlineRes(ResTy::new(e))
                }
                Type::Agg(Agg::Enum(e)) => Answer::Inline(Some(e)),
                _ => Answer::Inline(None),
            };
        };
        self.row_answer(tables, row.ret, ret)
    }

    /// How a row answering `shape` returns `ret`.
    fn row_answer(&mut self, tables: &Tables, shape: Ret, ret: &Ty) -> Answer {
        let ret = Types::runtime_ty(tables, ret);
        match shape {
            Ret::Void => Answer::Void,
            Ret::Scalar => Answer::Scalar,
            Ret::NoReturn => Answer::NoReturn,
            Ret::Int(bits) => Answer::Int(bits),
            Ret::Out => Answer::Out,
            Ret::Sum => Answer::Sum(self.enum_of(tables, &ret, TyConId::OPTION)),
            Ret::Res => Answer::Res(self.result_of(tables, &ret)),
            Ret::ResMsg => Answer::ResMsg(self.result_of(tables, &ret)),
        }
    }

    /// `ty`, an instance of `core/result`'s `Result`.
    fn result_of(&mut self, tables: &Tables, ty: &Ty) -> ResTy {
        ResTy::new(self.enum_of(tables, ty, TyConId::RESULT))
    }

    /// `ty`, an instance of the enum `con`.
    fn enum_of(&mut self, tables: &Tables, ty: &Ty, con: TyConId) -> EnumTy {
        let args = match ty.kind() {
            TyKind::Con(_, args) => *args,
            _ => &[],
        };
        self.enum_head(tables, EnumHead { ty: *ty, con, args })
    }

    /// `[elem]`.
    fn list_of(&mut self, tables: &Tables, elem: Ty) -> ListTy {
        let ty = Ty::array(elem);
        if let Some(l) = self.lists.get(&ty) {
            return *l;
        }
        let l = self.list.push_list(self.name(tables, &ty), ty, elem);
        self.lists.insert(ty, l);
        l
    }

    /// `ty`, a struct, tuple, context or `Str`.
    fn record_of(&mut self, tables: &Tables, ty: &Ty) -> RecTy {
        let ty = Types::runtime_ty(tables, ty);
        self.record(tables, ty)
    }

    fn record(&mut self, tables: &Tables, ty: Ty) -> RecTy {
        if let Some(r) = self.records.get(&ty) {
            return *r;
        }
        let fields = crate::compiler::semantics::types::field_types(tables, &ty)
            .into_iter()
            .collect();
        let r = self.list.push_record(self.name(tables, &ty), ty, fields);
        self.records.insert(ty, r);
        r
    }

    fn enum_head(&mut self, tables: &Tables, head: EnumHead) -> EnumTy {
        if let Some(e) = self.enums.get(&head.ty) {
            return *e;
        }
        let e = self.list.push_enum(self.name(tables, &head.ty), head);
        self.enums.insert(head.ty, e);
        e
    }

    fn closure(&mut self, tables: &Tables, ty: Ty) -> FnTy {
        if let Some(f) = self.fns.get(&ty) {
            return *f;
        }
        let f = self.list.push_fn(self.name(tables, &ty), ty);
        self.fns.insert(ty, f);
        f
    }

    /// Folds a function's own table into this one, the program's, and maps
    /// the function's ids onto it. The types are already
    /// [`Types::runtime_ty`]-normalized, so a merged table is byte-for-byte a
    /// serial one.
    fn adopt(&mut self, tables: &Tables, local: TypeList) -> Remap {
        local.remap(&mut Adopting {
            types: self,
            tables,
        })
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
    /// io.println(ctx, r).ignore()
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
        *ty
    }

    /// The machine shape of a source type. Total: a type with no register
    /// shape is an aggregate, and that includes `Str`, a list, a closure, a
    /// context and — where a tree reached lowering with one — `Ty::Error`.
    fn of(&mut self, tables: &Tables, ty: &Ty) -> Type {
        if matches!(ty.kind(), TyKind::Unit) {
            return Type::Unit;
        }
        // A `derive Flags` struct is its word (`middle::flags`).
        match tables
            .as_prim(ty)
            .or_else(|| tables.flags_word_of(ty))
            .and_then(Type::of_prim)
        {
            Some(t) => t,
            None => Type::Agg(self.agg(tables, ty)),
        }
    }
}

/// The program's [`Types`], as the table [`TypeList::remap`] adopts a
/// function's entries into.
struct Adopting<'a> {
    types: &'a mut Types,
    tables: &'a Tables,
}

impl Adopt for Adopting<'_> {
    fn record(&mut self, ty: Ty) -> RecTy {
        self.types.record(self.tables, ty)
    }

    fn enum_(&mut self, head: EnumHead) -> EnumTy {
        self.types.enum_head(self.tables, head)
    }

    fn list(&mut self, _ty: Ty, elem: Ty) -> ListTy {
        self.types.list_of(self.tables, elem)
    }

    fn closure(&mut self, ty: Ty) -> FnTy {
        self.types.closure(self.tables, ty)
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
    at: HashMap<(u32, rc::Position), Vec<rc::Site>>,
    ids: HashMap<*const Expr, rc::NodeId>,
    /// [`rc::FuncPlan::moved`]: the replaced fields whose old value an update
    /// does not release, because the one read of it took the base's count.
    moved: Vec<(rc::NodeId, FieldId)>,
}

impl Sites {
    fn of(plan: &rc::FuncPlan, body: Option<&Expr>) -> Sites {
        let Some(body) = body else {
            return Sites::default();
        };
        if plan.sites.is_empty() && plan.moved.is_empty() {
            return Sites::default();
        }
        let mut sites = Sites {
            moved: plan.moved.clone(),
            ..Sites::default()
        };
        // Only a node the plan names needs its id: every other one has no
        // sites, isn't a site's target and moved nothing.
        let mut wanted: Vec<u32> = Vec::with_capacity(plan.sites.len().saturating_mul(2));
        for site in &plan.sites {
            wanted.push(site.node.0);
            if let rc::Target::Node(n) = site.target {
                wanted.push(n.0);
            }
        }
        wanted.extend(plan.moved.iter().map(|(n, _)| n.0));
        wanted.sort_unstable();
        wanted.dedup();
        sites.ids.reserve(wanted.len());
        // Pre-order hands out ids in increasing order, so one cursor into the
        // sorted list does.
        let mut next = wanted.iter().peekable();
        rc::preorder(body, &mut |id, e| {
            if next.peek() == Some(&&id.0) {
                next.next();
                sites.ids.insert(std::ptr::from_ref(e), id);
            }
        });
        for site in &plan.sites {
            sites
                .at
                .entry((site.node.0, site.at))
                .or_default()
                .push(*site);
        }
        sites
    }

    fn id_of(&self, e: &Expr) -> Option<rc::NodeId> {
        self.ids.get(&std::ptr::from_ref(e)).copied()
    }

    /// Whether the update at `node` moved field `index` out of its base.
    fn moved(&self, node: Option<rc::NodeId>, index: FieldId) -> bool {
        node.is_some_and(|n| self.moved.binary_search(&(n, index)).is_ok())
    }

    fn get(&self, node: rc::NodeId, at: rc::Position) -> &[rc::Site] {
        self.at
            .get(&(node.0, at))
            .map(Vec::as_slice)
            .unwrap_or_default()
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
    plan: &'a rc::Plan,
    counts: &'a lists::Counts<'a>,
    /// Which fields `middle::layout` puts behind a box, for
    /// [`ExprKind::StructUpdate`].
    cycles: &'a layout::Cycles,
    types: &'a mut Types,
    /// The intrinsic keys this function names.
    keys: &'a mut Keys,
    locals: &'a typed::Locals,
    ret: Ty,
    code: Code,
    cur: BlockId,
    /// The value each local currently holds. A local is bound once by a
    /// parameter, a `let` or a pattern, at a point that dominates every read
    /// of it — which is what lexical scope means and is why this is a table
    /// rather than a stack of scopes.
    env: Dense<LocalId, Option<ValueId>>,
    loops: Vec<LoopFrame>,
    /// The shared block for a refutable pattern that matched nothing, built
    /// once per function and only where something needs it.
    unmatched: Option<BlockId>,
    /// `..rest` slices a pattern has matched but not yet allocated: the
    /// allocation waits until every test in the pattern has passed, so a test
    /// that fails after one leaves no block behind ([`FnLower::matched`]).
    slices: Vec<(LocalId, ValueId, ListTy, usize)>,
    /// Where `middle::rc` wants an `incref` or a `decref`.
    sites: Sites,
    /// What each node evaluated to, for the sites that name a temporary
    /// rather than a local.
    node_values: HashMap<rc::NodeId, ValueId>,
    /// The function being lowered, for the plan-order tripwire's message and
    /// for nothing else — which is why it is not carried at all in a build
    /// that has no tripwire in it.
    #[cfg(debug_assertions)]
    debug_name: &'a str,
}

impl FnLower<'_> {
    // -- the shape of a function --------------------------------------------

    fn func(mut self, params: &[LocalId], body: Option<&Expr>, dispatch: bool) -> Code {
        let mut args: Vec<ValueId> = self.code.entry().params.clone();
        // A dispatching function's first parameter is the entry index.
        let dispatch = if dispatch { Some(args.remove(0)) } else { None };
        for (local, value) in params.iter().zip(args.iter()) {
            self.bind(*local, *value);
        }

        match body {
            Some(e) => {
                let v = self.body(e, params, dispatch);
                self.set_term(Term::Return(v));
            }
            // A function nothing built: a trait method with no impl, or a
            // build that stopped after a diagnostic. Reaching one is a
            // compiler bug, so it aborts where it is rather than returning a
            // value at whatever type the caller expected.
            None => {
                self.abort("this function was never built");
            }
        }

        self.code.retain_reachable();
        self.code
    }

    /// The body, with a `Loop` at the root recognised: its header parameters
    /// are the function's parameters, rebound per iteration.
    fn body(&mut self, e: &Expr, params: &[LocalId], dispatch: Option<ValueId>) -> ValueId {
        let ExprKind::Loop { entries } = &e.kind else {
            return self.expr(e);
        };
        let ret = self.ret;
        let result = self.type_of(&ret);
        let var_types: Vec<Type> = params.iter().map(|p| self.local_type(*p)).collect();

        let mut header_types: Vec<Type> = Vec::new();
        let multi = entries.len() > 1;
        if multi {
            header_types.push(Type::I32);
        }
        header_types.extend(var_types.iter().copied());
        let header = self.code.block(&header_types);
        let (join, join_v) = self.join(result);

        let mut header_params = self.code.get(header).params.clone();
        let header_dispatch = if multi {
            Some(header_params.remove(0))
        } else {
            None
        };

        // Into the loop, with the parameters the function was called with —
        // and, for a merged group, the entry its caller chose.
        let mut first: Vec<ValueId> = Vec::new();
        if multi {
            let d = match dispatch {
                Some(d) => d,
                None => self.constant(
                    Type::I32,
                    Const::Int {
                        bits: Magnitude::new(0),
                        negative: false,
                    },
                ),
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
        let entry_blocks: Vec<BlockId> = entries
            .iter()
            .map(|_| self.code.block(&[]))
            .collect::<Vec<_>>();
        self.cur = header;
        match (header_dispatch, entry_blocks.as_slice()) {
            (_, [only]) => self.set_term(Term::Jump(Target::to(*only))),
            (Some(d), blocks) => {
                let cases: Vec<(u64, Target)> = blocks
                    .iter()
                    .enumerate()
                    .map(|(i, b)| (i as u64, Target::to(*b)))
                    .collect();
                self.set_term(Term::Switch {
                    on: d,
                    cases,
                    default: None,
                });
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
        join_v
    }

    // -- the builder --------------------------------------------------------

    fn type_of(&mut self, ty: &Ty) -> Type {
        self.types.of(self.tables, ty)
    }

    /// `ty`, a struct, tuple, context or `Str`, as the record it is.
    fn record_of(&mut self, ty: &Ty) -> RecTy {
        self.types.record_of(self.tables, ty)
    }

    /// `ty`, an instance of the enum `con`, as the enum it is.
    fn enum_of(&mut self, ty: &Ty, con: TyConId) -> EnumTy {
        self.types.enum_of(self.tables, ty, con)
    }

    /// The list of `elem`.
    fn list_of(&mut self, elem: Ty) -> ListTy {
        self.types.list_of(self.tables, elem)
    }

    /// Whether field `index` of a `derive Flags` value is set: its bit, which
    /// `middle::flags` places at `n - 1 - index`.
    fn flag(&mut self, val: ValueId, prim: Prim, con: TyConId, index: usize) -> ValueId {
        let n = self.tables.tycon(con).fields().len();
        let ty = Type::of_prim(prim).unwrap_or(Type::I64);
        let shift = n.saturating_sub(1).saturating_sub(index);
        let mask = self.constant(
            ty,
            Const::Int {
                bits: Magnitude::new(1u128 << shift),
                negative: false,
            },
        );
        let bit = self.emit(ty, |dest| Inst::Binary {
            dest,
            op: BinOp::BitAnd,
            prim,
            lhs: val,
            rhs: mask,
        });
        let zero = self.int(ty, 0);
        self.emit(Type::I1, |dest| Inst::Binary {
            dest,
            op: BinOp::Ne,
            prim,
            lhs: bit,
            rhs: zero,
        })
    }

    fn local_type(&mut self, l: LocalId) -> Type {
        let ty = self.locals.get(l).ty;
        self.type_of(&ty)
    }

    fn bind(&mut self, l: LocalId, v: ValueId) {
        *self.env.get_mut(l) = Some(v);
    }

    fn read(&mut self, l: LocalId) -> ValueId {
        match *self.env.get(l) {
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

    /// A block that takes the one value the paths into it agree on.
    fn join(&mut self, ty: Type) -> (BlockId, ValueId) {
        let (block, [value]) = self.code.block_of([ty]);
        (block, value)
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
        self.constant(
            ty,
            Const::Int {
                bits: Magnitude::new(n as u128),
                negative: false,
            },
        )
    }

    /// Ends the current block with an abort and continues in a block nothing
    /// reaches, so that the caller still has a value and the dead code is
    /// deleted with the block.
    fn abort(&mut self, message: &str) -> ValueId {
        self.push(Inst::Abort {
            message: message.into(),
        });
        self.set_term(Term::Unreachable);
        let ret = self.ret;
        let ty = self.type_of(&ret);
        self.dead(ty)
    }

    /// A value in a block nothing jumps to: what an expression that does not
    /// answer answers.
    fn dead(&mut self, ty: Type) -> ValueId {
        let (b, b_v) = self.join(ty);
        self.cur = b;
        b_v
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
        self.push(Inst::Abort {
            message: "no arm of this match applied".into(),
        });
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
                    self.rc(n, rc::Position::Before);
                }
                let v = self.expr_inner(value);
                if let Some(n) = node {
                    self.node_values.insert(n, v);
                }
                let fail = self.unmatched();
                self.matched(v, pattern, fail);
                if let Some(n) = node {
                    self.rc(n, rc::Position::After);
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
            self.rc(n, rc::Position::Before);
        }
        let v = self.expr_inner(e);
        if let Some(n) = node {
            self.node_values.insert(n, v);
            self.rc(n, rc::Position::After);
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
    fn rc(&mut self, node: rc::NodeId, at: rc::Position) {
        let sites: Vec<rc::Site> = self.sites.get(node, at).to_vec();
        #[cfg(debug_assertions)]
        let mut emitted: Vec<(rc::RcOp, ValueId)> = Vec::new();
        for site in sites {
            let value = match site.target {
                rc::Target::Local(l) => *self.env.get(l),
                rc::Target::Node(n) => self.node_values.get(&n).copied(),
                rc::Target::LocalExcept(l, moved) => {
                    self.release_except(l, moved);
                    continue;
                }
            };
            let Some(value) = value else { continue };
            #[cfg(debug_assertions)]
            emitted.push((site.op, value));
            self.push(match site.op {
                rc::RcOp::IncRef => Inst::IncRef { value },
                rc::RcOp::DecRef => Inst::DecRef { value },
            });
        }
        #[cfg(debug_assertions)]
        self.check_plan_order(node, at, &emitted);
    }

    /// Releases each field of the struct in local `l` except the ones `moved`
    /// names ([`rc::Target::LocalExcept`]), the way a functional update
    /// releases the fields it replaces.
    fn release_except(&mut self, l: LocalId, moved: u64) {
        let agg = (*self.env.get(l)).or_ice("a local `rc` releases is bound");
        let ty = self.locals.get(l).ty;
        let ids = ty.head().map(|c| self.tables.tycon(c).fields().ids());
        for i in ids.into_iter().flatten() {
            if i.index() < 64 && moved & (1u64 << i.index()) != 0 {
                continue;
            }
            let f = self.field_type(&ty, i);
            let rec = self.record_of(&ty);
            let value = self.emit(f, |dest| Inst::GetField {
                dest,
                agg,
                ty: rec,
                field: i,
            });
            self.push(Inst::DecRef { value });
        }
    }

    /// The plan-order invariant: **no value is released at a key and retained
    /// at that same key**.
    ///
    /// *Here* rather than anywhere else because this is the one place the
    /// identity the invariant needs exists. The plan holds [`rc::Target`]s, and
    /// a drop keyed on `Local(l)` and the increment that gave `l` its count
    /// keyed on the `Node` that produced it are two targets naming one block —
    /// a distinction that dissolves three lines above, where both kinds resolve
    /// to a `ValueId`. It sees exactly what is emitted, too: a site whose
    /// target resolves to nothing is skipped rather than guessed at, and a
    /// skipped site cannot free anything.
    ///
    /// **A tripwire, not a gate, and that is why it is `debug_assertions`.**
    /// [`rc::order_sites`] runs every increment at a key before every decrement
    /// at it, so this can no longer fire on any plan that pass produced: what
    /// it watches for is a *future edit* to the ordering or to `lower`'s
    /// emission, which is a compiler contributor's mistake and not a property
    /// of the program being compiled. So the audience is the test suite, where
    /// it runs over every program the corpus compiles on both native pipelines,
    /// and the cost — a `Vec` and a quadratic scan of it, at every node and
    /// both positions of every function lowered — is one the release compiler
    /// does not pay to learn something it cannot act on. A release build could
    /// only turn a violation into a compiler panic on a user's program, which
    /// is a worse answer than the one `order_sites` already guarantees. The
    /// same shape and the same argument as `ir::verify`, which both native
    /// backends run behind `cfg!(debug_assertions)`.
    ///
    /// It **asserts**, and the whole function is absent from a release build —
    /// which is what keeps it inside this repository's rule that no input may
    /// panic the toolchain (`Cargo.toml`, `[workspace.lints.clippy]`). There is
    /// no input that can reach it in a shipped compiler, because there is no it.
    /// A diagnostic would be the alternative, in the shape `ir::verify`'s
    /// callers use — and `lower` has no diagnostic channel to put one on, which
    /// is the second reason this is a developer's check rather than a user's.
    ///
    /// The message names the function, the node, the position, the value and
    /// the whole plan for the key: a *named failing test*, three frames and one
    /// allocation earlier than the segfault this class produced when nobody
    /// knew to look for it (`reports/llvm-parallel-listen-fix.md` §2).
    #[cfg(debug_assertions)]
    fn check_plan_order(
        &self,
        node: rc::NodeId,
        at: rc::Position,
        emitted: &[(rc::RcOp, ValueId)],
    ) {
        let violation = rc::release_then_retain(emitted);
        assert!(
            violation.is_none(),
            "release-then-retain in `{}`: {} is released and then retained at ({node:?}, {}) — \
             a free, and then a write through the freed header. The plan for this key is \
             {emitted:?}; `rc::order_sites` is what keeps every increment at a key ahead of \
             every decrement at it.",
            self.debug_name,
            violation.map_or(String::new(), |v| format!("v{v}")),
            match at {
                rc::Position::Before => "before",
                rc::Position::After => "after",
                rc::Position::Escape => "escape",
                rc::Position::Rejected => "rejected",
            }
        );
    }

    fn expr_inner(&mut self, e: &Expr) -> ValueId {
        let ty = self.type_of(&e.ty);
        match &e.kind {
            ExprKind::Int(v, negative) => self.constant(
                ty,
                Const::Int {
                    bits: *v,
                    negative: *negative,
                },
            ),
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
            ExprKind::Str(s) => self.constant(ty, Const::Str(s.as_str().into())),
            ExprKind::Char(c) => self.constant(ty, Const::Char(*c)),
            ExprKind::Bool(b) => self.constant(ty, Const::Bool(*b)),
            ExprKind::Unit => self.constant(ty, Const::Unit),
            ExprKind::Local(l) => self.read(*l),

            // Three shapes monomorphization removes: a module-level `let` is inlined at
            // its use, a trait call is resolved to a direct one, and a context
            // constructor becomes a call to the function that builds it. One
            // that survives means this tree did not go through
            // `monomorphize::run`, which is a compiler bug and not an input.
            ExprKind::Const(_)
            | ExprKind::CallTrait { .. }
            | ExprKind::CtxCall { .. }
            | ExprKind::CallFn { .. }
            | ExprKind::FnRef(_) => self.abort("this expression was not monomorphized"),
            ExprKind::Error => self.abort("this expression did not check"),

            ExprKind::FuncRef(f) => self.emit(ty, |dest| Inst::MakeClosure {
                dest,
                func: *f,
                env: None,
            }),
            ExprKind::CallFunc { func, args } => {
                if let FuncKind::Intrinsic(key) = &self.program.funcs.get(*func).kind {
                    if lists::handles(key) {
                        return self.list_call(key, args, &e.ty);
                    }
                }
                let args = self.exprs(args);
                self.emit(ty, |dest| Inst::Call {
                    dest,
                    func: *func,
                    args,
                })
            }
            ExprKind::CallValue { callee, args } => {
                let c = self.expr(callee);
                let args = self.exprs(args);
                self.emit(ty, |dest| Inst::CallIndirect {
                    dest,
                    callee: c,
                    args,
                })
            }
            ExprKind::Intrinsic { name, args, .. } if lists::handles(name) => {
                self.list_call(name, args, &e.ty)
            }
            ExprKind::Intrinsic { name, args, .. } => self.intrinsic(ty, &e.ty, name, args),

            ExprKind::StructLit { fields, .. } => {
                let fields = self.exprs(fields);
                let rec = self.record_of(&e.ty);
                self.emit(ty, |dest| Inst::MakeStruct {
                    dest,
                    ty: rec,
                    fields,
                })
            }
            ExprKind::StructUpdate { con, base, updates } => {
                // `..base` is evaluated first and once, and the replacements
                // run in field order — which is what the struct literal this
                // is shorthand for would have done.
                let b = self.expr(base);
                let ids = self.tables.tycon(*con).fields().ids();
                if self.has_boxed_field(&base.ty) {
                    return self.boxed_update(e, b, ids, updates, ty);
                }
                let mut fields = Vec::new();
                // The old value of every field this update *replaces*.
                //
                // `middle::rc` takes `..base` as a whole (`child_modes`, a
                // construction owns what it is given), so this expression holds
                // one reference to each of the base's counted fields — and the
                // struct it builds carries only the ones it did not replace. A
                // replaced field's old block was therefore counted for by
                // nobody, which is [`ExprKind::Field`]'s own case one construct
                // up: the reference exists and the value it named does not
                // outlive the expression. So it is released here, and there is
                // no node in the tree to hang that on — the old value is not
                // written anywhere in the source — which is why this is a
                // lowering rather than a site in the plan, exactly as
                // [`FnLower::template`]'s intermediate joins are.
                //
                // Reading it before the replacements run keeps the projection
                // ahead of anything that could release the base, and the
                // release itself goes last: a replacement is free to *be* the
                // old block (`cli/runtime/list.rs`'s `append_dest` reuses a
                // unique one in place), and that path takes a count of its own,
                // so the drop below takes back this expression's and not the
                // result's.
                //
                // Except a field `middle::rc` **moved** out of a dying base
                // (`rc::FuncPlan::moved`): its one read took this
                // expression's reference instead of a new one, so whoever read
                // it releases it, and releasing it here too would be twice.
                let node = self.sites.id_of(e);
                let mut replaced = Vec::new();
                for (i, _) in updates {
                    if self.sites.moved(node, *i) {
                        continue;
                    }
                    let f = self.field_type(&base.ty, *i);
                    let rec = self.record_of(&base.ty);
                    replaced.push(self.emit(f, |dest| Inst::GetField {
                        dest,
                        agg: b,
                        ty: rec,
                        field: *i,
                    }));
                }
                for i in ids {
                    match updates.iter().find(|(j, _)| *j == i) {
                        Some((_, v)) => {
                            let v = self.expr(v);
                            fields.push(v);
                        }
                        None => {
                            let f = self.field_type(&base.ty, i);
                            let rec = self.record_of(&base.ty);
                            fields.push(self.emit(f, |dest| Inst::GetField {
                                dest,
                                agg: b,
                                ty: rec,
                                field: i,
                            }));
                        }
                    }
                }
                let rec = self.record_of(&e.ty);
                let out = self.emit(ty, |dest| Inst::MakeStruct {
                    dest,
                    ty: rec,
                    fields,
                });
                for old in replaced {
                    self.push(Inst::DecRef { value: old });
                }
                out
            }
            ExprKind::EnumLit {
                con, variant, args, ..
            } => {
                let fields = self.exprs(args);
                let enum_ty = self.enum_of(&e.ty, *con);
                self.emit(ty, |dest| Inst::MakeEnum {
                    dest,
                    ty: enum_ty,
                    variant: *variant,
                    fields,
                })
            }
            ExprKind::Tuple(xs) => {
                let fields = self.exprs(xs);
                let rec = self.record_of(&e.ty);
                self.emit(ty, |dest| Inst::MakeStruct {
                    dest,
                    ty: rec,
                    fields,
                })
            }
            ExprKind::Array { elem, elems } => {
                let elems = self.exprs(elems);
                let list = self.list_of(*elem);
                self.emit(ty, |dest| Inst::MakeArray {
                    dest,
                    ty: list,
                    elems,
                })
            }

            ExprKind::Field { base, index } | ExprKind::TupleIndex { base, index } => {
                let b = self.expr(base);
                let field = *index;
                let rec = self.record_of(&base.ty);
                self.emit(ty, |dest| Inst::GetField {
                    dest,
                    agg: b,
                    ty: rec,
                    field,
                })
            }
            ExprKind::Index { base, index, elem } => self.index(ty, &e.ty, base, index, elem),

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
                let (join, join_v) = self.join(ty);
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
                join_v
            }
            ExprKind::Match { scrutinee, arms } => self.match_(ty, scrutinee, arms),

            ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
                let is_and = matches!(e.kind, ExprKind::And { .. });
                let l = self.expr(lhs);
                let rhs_b = self.block(&[]);
                let short_b = self.block(&[]);
                let (join, join_v) = self.join(Type::I1);
                let (then, else_) = if is_and {
                    (Target::to(rhs_b), Target::to(short_b))
                } else {
                    (Target::to(short_b), Target::to(rhs_b))
                };
                self.set_term(Term::Branch {
                    cond: l,
                    then,
                    else_,
                });

                self.cur = short_b;
                let short = self.constant(Type::I1, Const::Bool(!is_and));
                self.set_term(Term::Jump(Target::new(join, vec![short])));

                self.cur = rhs_b;
                let r = self.expr(rhs);
                self.set_term(Term::Jump(Target::new(join, vec![r])));

                self.cur = join;
                join_v
            }
            ExprKind::Try { base, kind } => {
                let node = self.sites.id_of(e);
                self.try_(ty, base, *kind, node)
            }

            ExprKind::Prim { op, prim, args } => self.prim(ty, *op, *prim, args),
            ExprKind::Unary { op, prim, arg } => {
                let arg = self.expr(arg);
                let op = match op {
                    UnaryOp::Neg => UnOp::Neg,
                    UnaryOp::Not => UnOp::Not,
                    UnaryOp::BitNot => UnOp::BitNot,
                };
                self.emit(ty, |dest| Inst::Unary {
                    dest,
                    op,
                    prim: *prim,
                    arg,
                })
            }
            ExprKind::Template { parts } => self.template(ty, parts),

            ExprKind::CtxLit { bindings, .. } => {
                let fields = bindings.iter().map(|(_, v)| self.expr(v)).collect();
                let rec = self.record_of(&e.ty);
                self.emit(ty, |dest| Inst::MakeStruct {
                    dest,
                    ty: rec,
                    fields,
                })
            }
            ExprKind::CtxGet { base, field, .. } => {
                let b = self.expr(base);
                let field = *field;
                let rec = self.record_of(&base.ty);
                self.emit(ty, |dest| Inst::GetField {
                    dest,
                    agg: b,
                    ty: rec,
                    field,
                })
            }

            // What `middle::closures` leaves in a lambda's place: a code
            // pointer and the captured values. The environment is a record of
            // exactly those values, and its layout is a tuple's — fields in
            // capture order, at natural alignment (VALUE-MODEL.md §5) — so the
            // type it is built at is the tuple of their types rather than a
            // synthesized declaration the type tables would have to carry.
            ExprKind::Closure { func, env } => {
                let env_ty = Ty::tuple(env.iter().map(|e| e.ty));
                let fields = self.exprs(env);
                let func = *func;
                // `middle::closures` makes a lambda that captured nothing a
                // reference instead, so a closure always carries something.
                let record = self.type_of(&env_ty);
                let rec = self.record_of(&env_ty);
                let env = self.emit(record, |dest| Inst::MakeStruct {
                    dest,
                    ty: rec,
                    fields,
                });
                self.emit(ty, |dest| Inst::MakeClosure {
                    dest,
                    func,
                    env: Some(ir::Env {
                        value: env,
                        ty: rec,
                    }),
                })
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
    fn index(&mut self, ty: Type, opt: &Ty, base: &Expr, index: &Expr, elem: &Ty) -> ValueId {
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
        let (join, join_v) = self.join(ty);
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
        let list = self.list_of(*elem);
        let v = self.emit(elem_ty, |dest| Inst::ArrayGet {
            dest,
            array: xs,
            ty: list,
            index: i,
        });
        let option = self.enum_of(opt, TyConId::OPTION);
        let some = self.emit(ty, |dest| Inst::MakeEnum {
            dest,
            ty: option,
            variant: VariantId::SOME,
            fields: vec![v],
        });
        self.set_term(Term::Jump(Target::new(join, vec![some])));

        self.cur = none_b;
        let none = self.emit(ty, |dest| Inst::MakeEnum {
            dest,
            ty: option,
            variant: VariantId::NONE,
            fields: Vec::new(),
        });
        self.set_term(Term::Jump(Target::new(join, vec![none])));

        self.cur = join;
        join_v
    }

    /// `?`, the only early exit in the language (SPEC 6.8).
    ///
    /// The failure arm returns a *rebuilt* value rather than the one it
    /// matched, because natively `Result<T, E>` and `Result<U, E>` are
    /// different types with different layouts — the JavaScript backend can
    /// return what it matched (`generate.rs:2074`) only because there both are
    /// the same array.
    ///
    /// `node` is the `?`'s own key in the plan, and the failure arm is where
    /// [`rc::Position::Escape`]'s drops go: the continuation that would have
    /// released what this function still owns is the half being abandoned, so
    /// without them every owned local live across a `?` leaks whenever the `?`
    /// takes this arm. They stand **after** the payload has been read out of
    /// `v` and before the return, because a drop is the one thing that can make
    /// a projection's base unreadable.
    fn try_(
        &mut self,
        ty: Type,
        base: &Expr,
        kind: OptionOrResult,
        node: Option<rc::NodeId>,
    ) -> ValueId {
        let v = self.expr(base);
        let held = Self::held_variant(kind);
        let from_ty = self.enum_of(&base.ty, con_of(kind));
        let ok = self.tag_is(v, from_ty, held);
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
        let ret = self.ret;
        let ret_ty = self.type_of(&ret);
        let out = match kind {
            OptionOrResult::Option => {
                let variant = VariantId::NONE;
                let ret_enum = self.enum_of(&ret, TyConId::OPTION);
                self.emit(ret_ty, |dest| Inst::MakeEnum {
                    dest,
                    ty: ret_enum,
                    variant,
                    fields: Vec::new(),
                })
            }
            OptionOrResult::Result => {
                let err_ty = self.payload_type(&base.ty, VariantId::ERR, FieldId::FIRST);
                let e = self.emit(err_ty, |dest| Inst::GetPayload {
                    dest,
                    agg: v,
                    ty: from_ty,
                    variant: VariantId::ERR,
                    field: FieldId::FIRST,
                });
                let variant = VariantId::ERR;
                let ret_enum = self.enum_of(&ret, TyConId::RESULT);
                self.emit(ret_ty, |dest| Inst::MakeEnum {
                    dest,
                    ty: ret_enum,
                    variant,
                    fields: vec![e],
                })
            }
        };
        if let Some(n) = node {
            self.rc(n, rc::Position::Escape);
        }
        self.set_term(Term::Return(out));

        self.cur = held_b;
        self.emit(ty, |dest| Inst::GetPayload {
            dest,
            agg: v,
            ty: from_ty,
            variant: held,
            field: FieldId::FIRST,
        })
    }

    fn prim(&mut self, ty: Type, op: PrimOp, prim: Prim, [lhs, rhs]: &[Expr; 2]) -> ValueId {
        let (lhs, rhs) = (self.expr(lhs), self.expr(rhs));
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
        };
        self.emit(ty, |dest| Inst::Binary {
            dest,
            op,
            prim,
            lhs,
            rhs,
        })
    }

    /// An operation the runtime supplies.
    ///
    /// The `structural*` family never reaches here: `middle::derives` replaced
    /// each with a call to the function it generated for the type.
    fn intrinsic(&mut self, ty: Type, ret: &Ty, name: &str, args: &[Expr]) -> ValueId {
        let key = qualified_key(self.tables, name, args);
        let args = self.exprs(args).into_boxed_slice();
        self.call_intrinsic(ty, ret, &key, args)
    }

    /// An [`Inst::CallIntrinsic`] of `key`, answering `ret`, which lowers to
    /// `ty`.
    fn call_intrinsic(&mut self, ty: Type, ret: &Ty, key: &str, args: Box<[ValueId]>) -> ValueId {
        let answer = self.answer(key, ret, ty);
        let key = self.keys.calls.intern(key);
        self.emit(ty, |dest| Inst::CallIntrinsic {
            dest,
            key,
            answer,
            args,
        })
    }

    /// How `key`'s result comes back ([`Types::answer`]).
    fn answer(&mut self, key: &str, ret: &Ty, ty: Type) -> Answer {
        self.types.answer(self.tables, key, ret, ty)
    }

    /// An interpolation: render every hole from its static type and join the
    /// parts (VALUE-MODEL.md §3.3).
    ///
    /// The join is `str.concat` with no context argument, because a context of
    /// zero-sized implementations is dropped from every signature
    /// (VALUE-MODEL.md §8) and `platform/host`'s are all zero-sized. A program
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
        if parts.len() > CONCAT_CHAIN_MAX {
            return self.template_pieces(ty, parts);
        }
        let (rendered, _) = self.render(ty, parts);
        let mut it = rendered.into_iter();
        // A template is written with at least one part; one with none would be
        // the empty string.
        let (mut acc, mut acc_is_mine) = it
            .next()
            .unwrap_or_else(|| (self.constant(ty, Const::Str("".into())), false));
        let str_ty = self.tables.prim(Prim::Str);
        for (next, next_is_mine) in it {
            let joined = self.call_intrinsic(ty, &str_ty, "str.concat", Box::new([acc, next]));
            // *After* the join, not before. On MEMORY.md §5.3's in-place path
            // the result is `acc`'s own block with one more count on it, so
            // this drop takes that count back and frees nothing; on the two
            // allocating paths the bytes have already been copied out. Either
            // way the operand is dead the instant the join has run, and a
            // `str.concat` chain of *k* parts ends holding exactly one block.
            if next_is_mine {
                self.push(Inst::DecRef { value: next });
            }
            if acc_is_mine {
                self.push(Inst::DecRef { value: acc });
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

    /// Each part as a string, and whether this template owns it, then which
    /// parts are literals, whose block is null and needs no count.
    fn render(&mut self, ty: Type, parts: &[TemplatePart]) -> (Vec<(ValueId, bool)>, Vec<bool>) {
        let mut rendered: Vec<(ValueId, bool)> = Vec::new();
        let mut literal: Vec<bool> = Vec::new();
        for p in parts {
            literal.push(matches!(p, TemplatePart::Text(_)));
            match p {
                TemplatePart::Text(t) => {
                    let v = self.constant(ty, Const::Str(t.as_str().into()));
                    rendered.push((v, false));
                }
                TemplatePart::Hole(h) => {
                    let v = self.expr(h);
                    rendered.push((v, false));
                }
                TemplatePart::Shown { prim, value } => {
                    let v = self.expr(value);
                    let shown = self.emit(ty, |dest| Inst::ShowPrim {
                        dest,
                        prim: *prim,
                        value: v,
                    });
                    rendered.push((shown, true));
                }
            }
        }
        (rendered, literal)
    }

    /// A template of more than [`CONCAT_CHAIN_MAX`] parts, rendered and joined
    /// a piece of [`JOIN_PIECE`] at a time, then the pieces
    /// ([`FnLower::template_joined`]). Rendering every part before the first
    /// join kept every converted string live at once, and `llc`'s register
    /// allocator grows with that (PERFORMANCE.md §6.33).
    fn template_pieces(&mut self, ty: Type, parts: &[TemplatePart]) -> ValueId {
        let mut pieces: Vec<(ValueId, bool)> = Vec::new();
        for piece in parts.chunks(JOIN_PIECE) {
            let (rendered, literal) = self.render(ty, piece);
            pieces.push((self.join_parts(ty, &rendered, &literal), true));
        }
        match pieces.as_slice() {
            [(one, _)] => *one,
            _ => {
                let owned = vec![false; pieces.len()];
                self.template_joined(ty, &pieces, &owned)
            }
        }
    }

    /// A template of more than [`CONCAT_CHAIN_MAX`] parts: the parts go into
    /// a `[Str]` and the runtime joins them in one pass (`list.join` with an
    /// empty separator).
    ///
    /// Both backends open-code each `str.concat`, at about a hundred LLVM
    /// instructions apiece, so a chain is code linear in the parts with a
    /// large constant. A derived `Show` on a 200-field struct joins 401 parts,
    /// and its chain was 42k instructions that took `--release` most of a
    /// minute to optimise. The list costs a store and a count per part.
    ///
    /// The list owns its elements, so a part this template only borrows gets
    /// a count of its own going in, and dropping the list afterwards gives
    /// every part back, including the rendered ones this template owned. A
    /// literal's block is null, so it needs no count either way.
    ///
    /// More than [`JOIN_PIECE`] parts are joined a piece at a time, and then the
    /// pieces. One list of every part is three stores a part in one straight
    /// line, and `llc`'s schedulers are quadratic in a line with no call in it:
    /// a template of 801 parts took 34.5 G instructions to emit
    /// (PERFORMANCE.md §6.32). A piece ends at its `list.join` call.
    fn template_joined(
        &mut self,
        ty: Type,
        rendered: &[(ValueId, bool)],
        literal: &[bool],
    ) -> ValueId {
        if rendered.len() <= JOIN_PIECE {
            return self.join_parts(ty, rendered, literal);
        }
        let mut pieces: Vec<(ValueId, bool)> = Vec::new();
        for (parts, literal) in rendered.chunks(JOIN_PIECE).zip(literal.chunks(JOIN_PIECE)) {
            pieces.push((self.join_parts(ty, parts, literal), true));
        }
        let owned = vec![false; pieces.len()];
        self.template_joined(ty, &pieces, &owned)
    }

    /// `rendered` into one `[Str]`, joined by the runtime.
    fn join_parts(&mut self, ty: Type, rendered: &[(ValueId, bool)], literal: &[bool]) -> ValueId {
        let str_t = self.tables.prim(Prim::Str);
        let list_t = self.type_of(&Ty::array(str_t));
        let list_ty = self.list_of(str_t);
        for ((v, mine), literal) in rendered.iter().zip(literal) {
            if !mine && !literal {
                self.push(Inst::IncRef { value: *v });
            }
        }
        let elems = rendered.iter().map(|(v, _)| *v).collect();
        let list = self.emit(list_t, |dest| Inst::MakeArray {
            dest,
            ty: list_ty,
            elems,
        });
        // `list.join` takes a context, and the runtime has no use for one
        // (`runtime_table`'s `Arg::Dropped`), so nothing stands in for it.
        let ctx = self.constant(Type::Unit, Const::Unit);
        let separator = self.constant(ty, Const::Str(Box::default()));
        let str_ty = self.tables.prim(Prim::Str);
        let joined =
            self.call_intrinsic(ty, &str_ty, "list.join", Box::new([list, ctx, separator]));
        self.push(Inst::DecRef { value: list });
        joined
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
                let frame = self
                    .loops
                    .last()
                    .or_ice("`tail_calls` writes a back edge only inside its loop");
                let (header, dispatch, vars) = (frame.header, frame.dispatch, frame.vars.clone());
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
                // Only a merged group is continued into, and it dispatches on
                // the entry.
                let mut call_args: Vec<ValueId> = vec![self.int(Type::I32, entry)];
                // The same padding as a back edge, for the same reason: a
                // member forwarding into the merged function has only its own
                // parameters and the function has the widest member's.
                call_args.extend(self.pad_to(&vs, f));
                let v = self.emit(ty, |dest| Inst::Call {
                    dest,
                    func: f,
                    args: call_args,
                });
                self.set_term(Term::Return(v));
                self.dead(ty)
            }
        }
    }

    /// The same, for a call into another function's loop, where the slots are
    /// that function's parameters rather than a block's.
    fn pad_to(&mut self, args: &[ValueId], func: FuncIdx) -> Vec<ValueId> {
        let callee = self.program.funcs.get(func);
        let slots: Vec<Ty> = callee
            .params
            .iter()
            .map(|p| callee.locals.get(*p).ty)
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
        let (join, join_v) = self.join(ty);
        if !self.switch(s, &scrutinee.ty, arms, join) {
            self.chain(s, arms, join);
        }
        self.cur = join;
        join_v
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
        let mut cases: Vec<(VariantId, &[FieldPat], &Expr)> = Vec::new();
        let mut default: Option<&Arm> = None;
        // The checker refuses an unreachable arm, so no unguarded variant
        // repeats and a catch-all is last.
        for arm in arms {
            if arm.guard.is_some() {
                return false;
            }
            match &arm.pattern.kind {
                PatKind::Variant {
                    variant, fields, ..
                } if fields.iter().all(|f| f.pattern.is_irrefutable(self.tables)) => {
                    cases.push((*variant, fields, &arm.body));
                }
                PatKind::Wild | PatKind::Bind { sub: None, .. } => default = Some(arm),
                _ => return false,
            }
        }
        if cases.len() < 2 {
            return false;
        }

        let enum_ty = self.enum_of(ty, con);
        let tag = self.emit(Type::I32, |dest| Inst::GetTag {
            dest,
            agg: s,
            ty: enum_ty,
        });
        let mut targets: Vec<(u64, Target)> = Vec::new();
        let mut bodies: Vec<(BlockId, VariantId, &[FieldPat], &Expr)> = Vec::new();
        for (variant, fields, body) in &cases {
            let b = self.block(&[]);
            targets.push((variant.index() as u64, Target::to(b)));
            bodies.push((b, *variant, fields, body));
        }
        let default_block = match default {
            Some(arm) => {
                let b = self.block(&[]);
                Some((b, arm))
            }
            // Total: exhaustiveness was proved, and with no catch-all every
            // variant has an arm. `Profile::defensive_aborts` is what decides
            // whether a backend emits an unreachable default anyway.
            None => None,
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
                    ty: enum_ty,
                    variant,
                    field: f.index,
                });
                let fail = self.unmatched();
                self.matched(v, &f.pattern, fail);
            }
            let v = self.expr(body);
            self.set_term(Term::Jump(Target::new(join, vec![v])));
        }
        if let Some((block, arm)) = default_block {
            self.cur = block;
            let fail = self.unmatched();
            self.matched(s, &arm.pattern, fail);
            let v = self.expr(&arm.body);
            self.set_term(Term::Jump(Target::new(join, vec![v])));
        }
        true
    }

    /// Arms in order: test, bind, guard, body. The general form, for
    /// everything the switch form declines.
    ///
    /// A failed guard goes on at the first later arm whose pattern isn't
    /// [`disjoint`] from its own, past the tests that can only fail.
    fn chain(&mut self, s: ValueId, arms: &[Arm], join: BlockId) {
        // Each arm's entry, made when first jumped to. `arms.len()` is the
        // abort past the last arm.
        let mut entries: Vec<Option<BlockId>> = vec![None; arms.len()];
        for (i, arm) in arms.iter().enumerate() {
            let next = self.arm_entry(&mut entries, i.saturating_add(1));
            self.matched(s, &arm.pattern, next);
            if let Some(g) = &arm.guard {
                let later = arms.get(i.saturating_add(1)..).unwrap_or_default();
                let skip = later
                    .iter()
                    .take_while(|a| disjoint(&arm.pattern, &a.pattern))
                    .count();
                let next = self.arm_entry(&mut entries, i.saturating_add(1).saturating_add(skip));
                let c = self.expr(g);
                let body_b = self.block(&[]);
                // What the arm bound for itself and still owns is released on
                // the way to the next arm (`rc::Position::Rejected`).
                let rejected = self
                    .sites
                    .id_of(g)
                    .filter(|n| !self.sites.get(*n, rc::Position::Rejected).is_empty());
                let else_b = if rejected.is_some() {
                    self.block(&[])
                } else {
                    next
                };
                self.set_term(Term::Branch {
                    cond: c,
                    then: Target::to(body_b),
                    else_: Target::to(else_b),
                });
                if let Some(n) = rejected {
                    self.cur = else_b;
                    self.rc(n, rc::Position::Rejected);
                    self.set_term(Term::Jump(Target::to(next)));
                }
                self.cur = body_b;
            }
            let v = self.expr(&arm.body);
            self.set_term(Term::Jump(Target::new(join, vec![v])));
            self.cur = self.arm_entry(&mut entries, i.saturating_add(1));
        }
    }

    /// Arm `i`'s entry block in [`FnLower::chain`], or the abort past the last.
    fn arm_entry(&mut self, entries: &mut [Option<BlockId>], i: usize) -> BlockId {
        let Some(slot) = entries.get_mut(i) else {
            return self.unmatched();
        };
        *slot.get_or_insert_with(|| self.block(&[]))
    }

    /// [`FnLower::pattern`], then the `..rest` slices it bound, once nothing
    /// is left that can fail.
    fn matched(&mut self, val: ValueId, pat: &Pattern, fail: BlockId) {
        let outer = std::mem::take(&mut self.slices);
        self.pattern(val, pat, fail);
        self.slice_pending();
        self.slices = outer;
    }

    fn slice_pending(&mut self) {
        for (l, array, list, at) in std::mem::take(&mut self.slices) {
            let ty = self.local_type(l);
            let from = self.int(Type::I64, at);
            let tail = self.emit(ty, |dest| Inst::ArraySlice {
                dest,
                array,
                ty: list,
                from,
            });
            self.bind(l, tail);
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
            PatKind::Number { prim, value } => {
                let ty = self.type_of(&pat.ty);
                let c = self.constant(
                    ty,
                    match *value {
                        NumberLit::Int(bits, negative) => Const::Int { bits, negative },
                        NumberLit::Float(v) => Const::Float(v),
                    },
                );
                self.test_eq(val, c, *prim, fail);
            }
            PatKind::Str(s) => {
                let ty = self.type_of(&pat.ty);
                let c = self.constant(ty, Const::Str(s.as_str().into()));
                self.test_eq(val, c, Prim::Str, fail);
            }
            PatKind::Char(c) => {
                let ty = self.type_of(&pat.ty);
                let c = self.constant(ty, Const::Char(*c));
                self.test_eq(val, c, Prim::Char, fail);
            }
            PatKind::Bool(b) => {
                let c = self.constant(Type::I1, Const::Bool(*b));
                self.test_eq(val, c, Prim::Bool, fail);
            }
            PatKind::Tuple(ps) => {
                let elements: Dense<FieldId, &Pattern> = ps.iter().collect();
                for (i, p) in elements.entries() {
                    let ty = self.type_of(&p.ty);
                    let rec = self.record_of(&pat.ty);
                    let f = self.emit(ty, |dest| Inst::GetField {
                        dest,
                        agg: val,
                        ty: rec,
                        field: i,
                    });
                    self.pattern(f, p, fail);
                }
            }
            PatKind::Struct { con, fields } => {
                let word = self.tables.flags_word(*con);
                for f in fields {
                    let ty = self.type_of(&f.pattern.ty);
                    let v = match word {
                        Some(prim) => self.flag(val, prim, *con, f.index.index()),
                        None => {
                            let rec = self.record_of(&pat.ty);
                            self.emit(ty, |dest| Inst::GetField {
                                dest,
                                agg: val,
                                ty: rec,
                                field: f.index,
                            })
                        }
                    };
                    self.pattern(v, &f.pattern, fail);
                }
            }
            PatKind::Variant {
                con,
                variant,
                fields,
            } => {
                let variant = *variant;
                let enum_ty = self.enum_of(&pat.ty, *con);
                // A single-variant enum has nothing to fall through to, so
                // there is no tag to test — only projections.
                if self.tables.tycon(*con).variants().len() > 1 {
                    let ok = self.tag_is(val, enum_ty, variant);
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
                        ty: enum_ty,
                        variant,
                        field: f.index,
                    });
                    self.pattern(v, &f.pattern, fail);
                }
            }
            PatKind::Array { elem, elems, rest } => {
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
                    let list = self.list_of(p.ty);
                    let idx = self.int(Type::I64, i);
                    let v = self.emit(ty, |dest| Inst::ArrayGet {
                        dest,
                        array: val,
                        ty: list,
                        index: idx,
                    });
                    self.pattern(v, p, fail);
                }
                if let ArrayRest::Bound(l) = rest {
                    let list = self.list_of(*elem);
                    self.slices.push((*l, val, list, elems.len()));
                }
            }
            // Alternatives bind the same names at the same types, and each
            // binds them to values of its own — so the continuation takes them
            // as block parameters. This is the case that would force a
            // mutable slot in an IR without block arguments, and it is one
            // line of `Target` here.
            PatKind::Or(alts) => {
                // An or-pattern binds what its first alternative does.
                let mut binds: Vec<LocalId> = Vec::new();
                pat.binds(&mut binds);
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
                    crate::diagnostics::invariant(
                        theirs == want,
                        "an or-pattern's alternatives bind different locals",
                    );
                }
                let types: Vec<Type> = binds.iter().map(|b| self.local_type(*b)).collect();
                let merge = self.block(&types);
                for (i, alt) in alts.iter().enumerate() {
                    let last = i.saturating_add(1) == alts.len();
                    let next = if last { fail } else { self.block(&[]) };
                    let outer = std::mem::take(&mut self.slices);
                    self.pattern(val, alt, next);
                    self.slice_pending();
                    self.slices = outer;
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
            // whatever keeps the CFG well formed. A number literal is one
            // too, by here: `monomorphize` makes each one it can a `Number`.
            PatKind::Int(..) | PatKind::Float(_) | PatKind::Error => {
                self.set_term(Term::Jump(Target::to(fail)));
                let _ = self.dead(Type::Unit);
            }
        }
    }

    /// `val == c` at `prim`, continuing where it holds and jumping to `fail`
    /// where it does not.
    fn test_eq(&mut self, val: ValueId, c: ValueId, prim: Prim, fail: BlockId) {
        let ok = self.emit(Type::I1, |dest| Inst::Binary {
            dest,
            op: BinOp::Eq,
            prim,
            lhs: val,
            rhs: c,
        });
        let next = self.block(&[]);
        self.set_term(Term::Branch {
            cond: ok,
            then: Target::to(next),
            else_: Target::to(fail),
        });
        self.cur = next;
    }

    /// `tag(v) == variant`.
    fn tag_is(&mut self, v: ValueId, ty: EnumTy, variant: VariantId) -> ValueId {
        let tag = self.emit(Type::I32, |dest| Inst::GetTag { dest, agg: v, ty });
        let want = self.int(Type::I32, variant.index());
        self.emit(Type::I1, |dest| Inst::Binary {
            dest,
            op: BinOp::Eq,
            prim: Prim::I32,
            lhs: tag,
            rhs: want,
        })
    }

    // -- reading the type tables --------------------------------------------

    /// Which variant of an `Option` or `Result` holds a value: `Some`, `Ok`.
    fn held_variant(kind: OptionOrResult) -> VariantId {
        match kind {
            OptionOrResult::Option => VariantId::SOME,
            OptionOrResult::Result => VariantId::OK,
        }
    }

    /// Whether any field of this struct is behind a box (`Layouts::boxes`).
    fn has_boxed_field(&self, owner: &Ty) -> bool {
        owner.head().is_some_and(|c| {
            self.tables
                .tycon(c)
                .fields()
                .iter()
                .any(|f| self.cycles.boxes(owner, &self.substituted(owner, f.ty)))
        })
    }

    /// `..base` of a struct with a field behind a box.
    ///
    /// The general lowering moves each kept field's count out of the base and
    /// lets the base itself go uncounted. A boxed field is a block of its own,
    /// though: reading it copies the value out of the box and building the
    /// result puts it in a new one, so the old box was released by nobody. So
    /// here every field the result keeps takes a count of its own, and then
    /// the base is released whole, boxes and replaced fields included. A field
    /// `middle::rc` moved into a replacement takes its count first, because
    /// the replacement consumes it and the base's release would be a second.
    fn boxed_update(
        &mut self,
        e: &Expr,
        b: ValueId,
        ids: impl Iterator<Item = FieldId>,
        updates: &[(FieldId, Expr)],
        ty: Type,
    ) -> ValueId {
        let base_ty = match &e.kind {
            ExprKind::StructUpdate { base, .. } => base.ty,
            _ => Ty::UNIT,
        };
        let node = self.sites.id_of(e);
        for (i, _) in updates {
            if self.sites.moved(node, *i) {
                let f = self.field_type(&base_ty, *i);
                let rec = self.record_of(&base_ty);
                let old = self.emit(f, |dest| Inst::GetField {
                    dest,
                    agg: b,
                    ty: rec,
                    field: *i,
                });
                self.push(Inst::IncRef { value: old });
            }
        }
        let mut fields = Vec::new();
        for i in ids {
            match updates.iter().find(|(j, _)| *j == i) {
                Some((_, v)) => {
                    let v = self.expr(v);
                    fields.push(v);
                }
                None => {
                    let f = self.field_type(&base_ty, i);
                    let rec = self.record_of(&base_ty);
                    let kept = self.emit(f, |dest| Inst::GetField {
                        dest,
                        agg: b,
                        ty: rec,
                        field: i,
                    });
                    self.push(Inst::IncRef { value: kept });
                    fields.push(kept);
                }
            }
        }
        let rec = self.record_of(&e.ty);
        let out = self.emit(ty, |dest| Inst::MakeStruct {
            dest,
            ty: rec,
            fields,
        });
        self.push(Inst::DecRef { value: b });
        out
    }

    fn field_type(&mut self, ty: &Ty, index: FieldId) -> Type {
        let field = ty
            .head()
            .map_or(Ty::UNIT, |c| self.tables.tycon(c).fields().get(index).ty);
        let field = self.substituted(ty, field);
        self.type_of(&field)
    }

    fn payload_type(&mut self, ty: &Ty, variant: VariantId, field: FieldId) -> Type {
        let field = ty.head().map_or(Ty::UNIT, |c| {
            self.tables.tycon(c).variant(variant).fields.get(field).ty
        });
        let field = self.substituted(ty, field);
        self.type_of(&field)
    }

    /// A declared field type, with the owning type's arguments substituted in.
    fn substituted(&self, owner: &Ty, field: Ty) -> Ty {
        match owner.kind() {
            TyKind::Con(_, args) if !args.is_empty() => {
                crate::compiler::semantics::types::substitute(&field, args, None)
            }
            _ => field,
        }
    }
}

/// The enum a `?` takes apart.
fn con_of(kind: OptionOrResult) -> TyConId {
    match kind {
        OptionOrResult::Option => TyConId::OPTION,
        OptionOrResult::Result => TyConId::RESULT,
    }
}

/// Whether no value matches both `p` and `q`. `false` where it can't tell, so
/// a `true` is what lets [`FnLower::chain`] skip an arm.
fn disjoint(p: &Pattern, q: &Pattern) -> bool {
    let same_fields = |a: &[FieldPat], b: &[FieldPat]| {
        a.iter().any(|f| {
            b.iter()
                .any(|g| g.index == f.index && disjoint(&f.pattern, &g.pattern))
        })
    };
    match (&p.kind, &q.kind) {
        (PatKind::Bind { sub: Some(s), .. }, _) => disjoint(s, q),
        (_, PatKind::Bind { sub: Some(s), .. }) => disjoint(p, s),
        (PatKind::Or(alts), _) => alts.iter().all(|a| disjoint(a, q)),
        (_, PatKind::Or(alts)) => alts.iter().all(|a| disjoint(p, a)),
        // Two patterns on one scrutinee name the same enum.
        (
            PatKind::Variant {
                variant: v,
                fields: f,
                ..
            },
            PatKind::Variant {
                variant: w,
                fields: g,
                ..
            },
        ) => v != w || same_fields(f, g),
        (PatKind::Struct { fields: f, .. }, PatKind::Struct { fields: g, .. }) => same_fields(f, g),
        (PatKind::Tuple(a), PatKind::Tuple(b)) => a.iter().zip(b).any(|(x, y)| disjoint(x, y)),
        // `-0` is `0`.
        (
            PatKind::Number {
                value: NumberLit::Int(a, m),
                ..
            },
            PatKind::Number {
                value: NumberLit::Int(b, n),
                ..
            },
        ) => a != b || (m != n && a.get() != 0),
        (PatKind::Str(a), PatKind::Str(b)) => a != b,
        (PatKind::Char(a), PatKind::Char(b)) => a != b,
        (PatKind::Bool(a), PatKind::Bool(b)) => a != b,
        _ => false,
    }
}

/// The intrinsic key, with the *return* type's primitive appended where the
/// operation is one of `Bounded`'s two.
///
/// `number.minValue` and `number.maxValue` take no argument at all: `Bounded`'s
/// methods reach their type through the return type (`js/intrinsics.rs`'s
/// `numeric_free` says the same thing on the other backend). By the time a
/// backend sees the call, that type is an IR scalar — `I8`, which is `I8` and
/// `U8` alike — and the two answers differ by 128.
///
/// So the key gains it, and becomes the three-segment form every other numeric
/// operation already has: `number.U8.minValue`. That is the same trick
/// [`qualified_key`] plays for `derivePrimShow`, for the same reason, and it is
/// applied here rather than in `monomorphize` because only `middle::lower`'s
/// output — which is the native backends' input and nothing else's — needs it.
fn bounded_key(tables: &Tables, key: &str, ret: &Ty) -> String {
    if !matches!(key, "number.minValue" | "number.maxValue") {
        return key.to_string();
    }
    match tables.as_prim(ret) {
        Some(p) => format!("number.{}.{}", p.name(), key.trim_start_matches("number.")),
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
fn qualified_key(tables: &Tables, name: &str, args: &[Expr]) -> Box<str> {
    let operand = match name {
        "derivePrimShow" | "derivePrimJson" => args.first(),
        "derivePrimHash" => args.get(1),
        _ => return name.into(),
    };
    // A `derive Flags` word is hashed as the primitive it is stored in.
    match operand.and_then(|a| {
        tables
            .as_prim(&a.ty)
            .or_else(|| tables.flags_word_of(&a.ty))
    }) {
        Some(p) => format!("{name}.{}", p.name()).into(),
        // A `derivePrim*` at something that is not a primitive is a bug in
        // `derives.rs` rather than in the program, and the unqualified key is
        // what reports it: no backend implements it, so it is named.
        None => name.into(),
    }
}
