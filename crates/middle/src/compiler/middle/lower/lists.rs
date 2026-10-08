//! `core/list`'s closure operations, `get`, `zip`, `flatten` and the three
//! `deriveArray*` derives, lowered to loops in the IR.
//!
//! `cli/runtime/list.rs`'s header says why the archive has no
//! `buri_rt_list_map`: a closure's `code` takes the *flattened* parameters of
//! its element type, so a C function calling one would have to build a
//! parameter list that depends on `T`. The loop has to be generated where `T`
//! is known. It used to be generated twice, once by each native backend, out
//! of that backend's own instructions. It is generated once here instead, out
//! of ordinary blocks, and both backends compile it as they compile any loop.
//!
//! # Where the loop goes
//!
//! At the call site, whichever way the key arrives: an `ExprKind::Intrinsic`, or
//! a call to the `FuncKind::Intrinsic` function a method resolves to. There the
//! step is usually a lambda written at the call, which `middle::closures` left
//! as an `ExprKind::FnRef` to the lifted function, so the step is a **direct**
//! call rather than one through the closure's thunk. The intrinsic function
//! gets the same loop as its own body, for the one caller that cannot see a
//! step: a function value, called through its thunk.
//!
//! # The counts
//!
//! `middle::rc` plans the caller as it plans any intrinsic call, and two of its
//! sentences decide every count here:
//!
//!  * *"A runtime intrinsic borrows its arguments and returns a fresh count."*
//!    The list, the closure and the context arrive borrowed and nothing here
//!    releases them. A fold's seed is the exception (`rc::is_fold`): it arrives
//!    owned, and the first step takes that count.
//!  * *"A call through a function value owns its arguments."* So a borrowed
//!    value handed to a step through its thunk is retained first.
//!
//! A **direct** call follows the callee's own `Facts::params` instead, which is
//! what the thunk does on the far side of an indirect one: a borrowed value
//! handed to an owning parameter is retained, and an owned one handed to a
//! borrowing parameter is released after the call. So a step that only reads
//! its element costs no count at all.
//!
//! Whether a value carries a count is `middle::rc`'s classifier's answer
//! ([`Counts`]), never the layout table's, because a retain rc does not count
//! is one half of a pair nothing completes.
//!
//! # The blocks a loop builds
//!
//! `map`, `filter`, `zip`, `flatten`, `sortBy` and `deriveArrayShow` build
//! into a fresh block: [`Inst::ArrayAlloc`], then one [`Inst::ArraySet`] per
//! element, which moves the element's counts in. `filter` ends with
//! [`Inst::ArrayPrefix`], because it fills only the elements it keeps. A loop
//! stores only into a block it allocated, which nothing else holds yet, so no
//! other list can see the writes (MEMORY.md §5.3).

use std::sync::{Mutex, OnceLock};

use super::FnLower;
use crate::compiler::backend::intrinsic_keys::{self, Step};
use crate::compiler::middle::ir::{
    BinOp, BlockId, Code, Const, Inst, Ownership, Signature, Target, Term, Type, UnOp, ValueId,
};
use crate::compiler::middle::monomorphize::{FuncKind, Program};
use crate::compiler::middle::rc;
use crate::compiler::semantics::typed::{Callee, Expr, ExprKind};
use crate::compiler::semantics::types::{FuncIdx, LocalId, Prim, Ty, TyKind};

/// Whether `key` is lowered here.
pub(super) fn handles(key: &str) -> bool {
    matches!(
        key,
        "list.get"
            | "list.zip"
            | "list.flatten"
            | "deriveArrayEq"
            | "deriveArrayCompare"
            | "deriveArrayShow"
    ) || intrinsic_keys::list_call(key).is_some()
}

/// Where `key`'s step is among its operands. `middle::derives` builds the
/// `deriveArray*` calls as `(xs, ys, step)` and `(xs, step)`.
fn step_at(key: &str) -> Option<usize> {
    match key {
        "deriveArrayEq" | "deriveArrayCompare" => Some(2),
        "deriveArrayShow" => Some(1),
        _ => intrinsic_keys::list_call(key).map(|c| c.func),
    }
}

/// `middle::rc`'s classifier, built the first time a loop asks.
///
/// Shared by every function `lower` builds across the cores, so it sits behind
/// a lock; a loop asks it a handful of times.
pub(super) struct Counts<'p> {
    program: &'p Program,
    classifier: OnceLock<Mutex<rc::Syntactic>>,
}

impl<'p> Counts<'p> {
    pub(super) fn new(program: &'p Program) -> Counts<'p> {
        Counts { program, classifier: OnceLock::new() }
    }

    fn counted(&self, ty: &Ty) -> bool {
        let classifier =
            self.classifier.get_or_init(|| Mutex::new(rc::Syntactic::new(self.program)));
        let mut c = classifier.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        matches!(rc::Counted::counted(&mut *c, ty), rc::Answer::Yes)
    }
}

/// The step, resolved once before the loop.
enum Via {
    /// A lifted lambda or a named function the call site names: a direct call.
    Direct {
        func: FuncIdx,
        /// The empty environment a lifted lambda still takes first.
        env: Option<ValueId>,
        /// The callee's ownership of each value parameter.
        own: Vec<Ownership>,
    },
    /// Anything else: a call through the closure's thunk, which owns every
    /// argument.
    Thunk(ValueId),
}

/// How many elements `sortBy` sorts by insertion before it merges: a power of
/// two, so a run starts where an index's low bits are zero.
const SORT_RUN: usize = 4;

/// A `sortBy`'s comparator, and the tag of the `Greater` it answers.
struct Order<'a> {
    callee: &'a Via,
    elem: Ty,
    order_t: Type,
    greater: ValueId,
}

/// One argument to a step: the value, its source type, and whether this loop
/// owns the count it holds.
struct Arg {
    value: ValueId,
    ty: Ty,
    owned: bool,
}

/// One counted loop under construction.
struct Walk {
    header: BlockId,
    body: BlockId,
    /// The index, the header's first parameter.
    i: ValueId,
    /// The values carried across iterations, the header's other parameters.
    vars: Vec<ValueId>,
    /// `i >= n`, computed in the header.
    ended: ValueId,
}

impl FnLower<'_> {
    /// A call of `key`, lowered as its loop.
    pub(super) fn list_call(&mut self, key: &str, args: &[Expr], ret: &Ty) -> ValueId {
        let vals = self.exprs(args);
        let tys: Vec<Ty> = args.iter().map(|a| a.ty).collect();
        // The step a call site names: a lambda `middle::closures` lifted, or a
        // function passed by name.
        let step = step_at(key)
            .and_then(|at| args.get(at))
            .and_then(|a| match &a.kind {
                ExprKind::FnRef(Callee::Func(f)) => Some(*f),
                _ => None,
            });
        match self.list_loop(key, &vals, &tys, step, ret) {
            Some(v) => v,
            None => self.abort("a `core/list` call whose operands are not the types it declares"),
        }
    }

    /// The intrinsic function's own body: the loop over its parameters.
    pub(super) fn list_body(&mut self, sig: &Signature, key: &str, params: &[LocalId]) -> Code {
        let entry = self.code.block(&sig.params);
        self.cur = entry;
        let vals = self.code.get(entry).params.clone();
        let tys: Vec<Ty> = params
            .iter()
            .map(|p| self.locals.get(p.index()).map(|l| l.ty).unwrap_or(Ty::UNIT))
            .collect();
        let ret = self.ret;
        let v = match self.list_loop(key, &vals, &tys, None, &ret) {
            Some(v) => v,
            None => self.abort("a `core/list` function whose parameters are not the types it declares"),
        };
        self.set_term(Term::Return(vec![v]));
        let mut code = std::mem::take(&mut self.code);
        code.retain_reachable();
        code
    }

    /// `key`'s loop over these operands, answering the result. `tys` are the
    /// operands' source types and `step` is the function the closure operand
    /// names where the call site shows one.
    pub(super) fn list_loop(
        &mut self,
        key: &str,
        vals: &[ValueId],
        tys: &[Ty],
        step: Option<FuncIdx>,
        ret: &Ty,
    ) -> Option<ValueId> {
        match key {
            "list.get" => return self.list_get(vals, tys, ret),
            "list.zip" => return self.list_zip(vals, tys, ret),
            "list.flatten" => return self.list_flatten(vals, tys, ret),
            "deriveArrayEq" | "deriveArrayCompare" => {
                return self.derive_array(key == "deriveArrayEq", vals, tys, step, ret)
            }
            "deriveArrayShow" => return self.derive_show(vals, tys, step, ret),
            _ => {}
        }
        let call = intrinsic_keys::list_call(key)?;
        let xs = *vals.first()?;
        let elem = match (tys.first()?).kind() {
            TyKind::Array(e) => *e,
            _ => return None,
        };
        let f = *vals.get(call.func)?;
        let TyKind::Fn(params, step_ret) = (tys.get(call.func)?).kind() else { return None };
        let step_ret = *step_ret;
        let callee = self.callee(f, step, params.len());
        let ctx = match call.ctx {
            Some(c) => Some(Arg { value: *vals.get(c)?, ty: *tys.get(c)?, owned: false }),
            None => None,
        };
        let n = self.emit(Type::I64, |dest| Inst::ArrayLen { dest, array: xs });
        let elem_t = self.type_of(&elem);
        let step_t = self.type_of(&step_ret);
        let ret_t = self.type_of(ret);
        let element = |l: &mut Self, i: ValueId| {
            let value = l.emit(elem_t, |dest| Inst::ArrayGet { dest, array: xs, index: i });
            Arg { value, ty: elem, owned: false }
        };
        let with_ctx = |ctx: &Option<Arg>, rest: Vec<Arg>| -> Vec<Arg> {
            let mut all: Vec<Arg> = ctx
                .iter()
                .map(|c| Arg { value: c.value, ty: c.ty, owned: false })
                .collect();
            all.extend(rest);
            all
        };

        Some(match call.kind {
            Step::Map => {
                let out = self.emit(ret_t, |dest| Inst::ArrayAlloc { dest, len: n });
                let w = self.walk(n, &[]);
                let e = element(self, w.i);
                let r = self.step(&callee, with_ctx(&ctx, vec![e]), step_t);
                self.push(Inst::ArraySet { array: out, index: w.i, value: r });
                self.again(&w, Vec::new());
                self.end(&w);
                out
            }
            Step::Filter => {
                let out = self.emit(ret_t, |dest| Inst::ArrayAlloc { dest, len: n });
                let zero = self.int(Type::I64, 0);
                let w = self.walk(n, &[zero]);
                let k = *w.vars.first()?;
                let e = element(self, w.i);
                let keep = self.step(&callee, with_ctx(&ctx, vec![e]), step_t);
                let (kept, skip) = self.fork(keep);
                // The element is read again rather than kept across the call,
                // so that the read the predicate is handed is used once and
                // can be made where the predicate takes it. The copy is a
                // second owner of what the element holds; the count the
                // predicate was handed was its own.
                self.cur = kept;
                let e = element(self, w.i);
                if self.counts.counted(&e.ty) {
                    self.push(Inst::IncRef { value: e.value });
                }
                self.push(Inst::ArraySet { array: out, index: k, value: e.value });
                let k1 = self.add_one(k);
                let latch = self.block(&[Type::I64]);
                self.set_term(Term::Jump(Target::new(latch, vec![k1])));
                self.cur = skip;
                self.set_term(Term::Jump(Target::new(latch, vec![k])));
                self.cur = latch;
                let k2 = *self.code.get(latch).params.first()?;
                self.again(&w, vec![k2]);
                self.end(&w);
                self.emit(ret_t, |dest| Inst::ArrayPrefix { dest, array: out, len: k })
            }
            // `map` and `filter` in one loop. The step's `Option` is this loop's,
            // so a `.Some` payload moves into the block with the count it already
            // holds, and a `.None` holds nothing to give back.
            Step::FilterMap => {
                let some = self.variant_of(&step_ret, "Some", 0);
                let some_tag = self.int(Type::I32, some as usize);
                let TyKind::Array(b) = ret.kind() else { return None };
                let b_t = self.type_of(b);
                let out = self.emit(ret_t, |dest| Inst::ArrayAlloc { dest, len: n });
                let zero = self.int(Type::I64, 0);
                let w = self.walk(n, &[zero]);
                let k = *w.vars.first()?;
                let e = element(self, w.i);
                let r = self.step(&callee, with_ctx(&ctx, vec![e]), step_t);
                let tag = self.emit(Type::I32, |dest| Inst::GetTag { dest, agg: r });
                let held = self.emit(Type::I1, |dest| Inst::Binary {
                    dest,
                    op: BinOp::Eq,
                    prim: Prim::I32,
                    lhs: tag,
                    rhs: some_tag,
                });
                let (kept, skip) = self.fork(held);
                self.cur = kept;
                let x = self.emit(b_t, |dest| Inst::GetPayload { dest, agg: r, variant: some, index: 0 });
                self.push(Inst::ArraySet { array: out, index: k, value: x });
                let k1 = self.add_one(k);
                let latch = self.block(&[Type::I64]);
                self.set_term(Term::Jump(Target::new(latch, vec![k1])));
                self.cur = skip;
                self.set_term(Term::Jump(Target::new(latch, vec![k])));
                self.cur = latch;
                let k2 = *self.code.get(latch).params.first()?;
                self.again(&w, vec![k2]);
                self.end(&w);
                self.emit(ret_t, |dest| Inst::ArrayPrefix { dest, array: out, len: k })
            }
            Step::Fold => {
                let init = *vals.get(call.init?)?;
                let init_ty = *tys.get(call.init?)?;
                let w = self.walk(n, &[init]);
                let acc = *w.vars.first()?;
                let e = element(self, w.i);
                let a = Arg { value: acc, ty: init_ty, owned: true };
                let next = self.step(&callee, with_ctx(&ctx, vec![a, e]), step_t);
                self.again(&w, vec![next]);
                self.end(&w);
                acc
            }
            // `any` and `all` leave at the first element that decides the
            // answer, which their declarations allow by taking no context: a
            // step that cannot have an effect cannot notice it was not run.
            Step::Any | Step::All => {
                let any = call.kind == Step::Any;
                let decided = self.constant(Type::I1, Const::Bool(any));
                let exhausted = self.constant(Type::I1, Const::Bool(!any));
                let w = self.walk(n, &[]);
                let e = element(self, w.i);
                let b = self.step(&callee, vec![e], step_t);
                // `all` leaves on a `false`, which is `b == false`: a
                // comparison the branch folds into itself.
                let cond = if any {
                    b
                } else {
                    self.emit(Type::I1, |dest| Inst::Binary {
                        dest,
                        op: BinOp::Eq,
                        prim: Prim::Bool,
                        lhs: b,
                        rhs: decided,
                    })
                };
                let exit = self.block(&[Type::I1]);
                let more = self.block(&[]);
                self.set_term(Term::Branch {
                    cond,
                    then: Target::new(exit, vec![decided]),
                    else_: Target::to(more),
                });
                self.cur = more;
                self.again(&w, Vec::new());
                self.end(&w);
                self.set_term(Term::Jump(Target::new(exit, vec![exhausted])));
                self.cur = exit;
                *self.code.get(exit).params.first()?
            }
            Step::Count => {
                let zero = self.int(Type::I64, 0);
                let w = self.walk(n, &[zero]);
                let c = *w.vars.first()?;
                let e = element(self, w.i);
                let b = self.step(&callee, vec![e], step_t);
                let one = self.emit(Type::I64, |dest| Inst::Unary {
                    dest,
                    op: UnOp::FromBool,
                    prim: Prim::Bool,
                    arg: b,
                });
                let c1 = self.emit(Type::I64, |dest| Inst::Binary {
                    dest,
                    op: BinOp::Add,
                    prim: Prim::I64,
                    lhs: c,
                    rhs: one,
                });
                self.again(&w, vec![c1]);
                self.end(&w);
                c
            }
            // `find` and `findIndex` leave at the first element the predicate
            // keeps, for `any`'s reason. `find`'s answer is a second owner of
            // the element it carries.
            Step::Find | Step::FindIndex => {
                let some = self.variant_of(ret, "Some", 0);
                let none = self.variant_of(ret, "None", 1);
                let w = self.walk(n, &[]);
                let e = element(self, w.i);
                let b = self.step(&callee, vec![e], step_t);
                let exit = self.block(&[ret_t]);
                let (hit, more) = self.fork(b);
                self.cur = hit;
                let payload = if call.kind == Step::Find {
                    let e = element(self, w.i);
                    if self.counts.counted(&e.ty) {
                        self.push(Inst::IncRef { value: e.value });
                    }
                    e.value
                } else {
                    w.i
                };
                let found = self.emit(ret_t, |dest| Inst::MakeEnum {
                    dest,
                    variant: some,
                    fields: vec![payload],
                });
                self.set_term(Term::Jump(Target::new(exit, vec![found])));
                self.cur = more;
                self.again(&w, Vec::new());
                self.end(&w);
                let missing =
                    self.emit(ret_t, |dest| Inst::MakeEnum { dest, variant: none, fields: Vec::new() });
                self.set_term(Term::Jump(Target::new(exit, vec![missing])));
                self.cur = exit;
                *self.code.get(exit).params.first()?
            }
            // A fold that stops at the first `.Err`, which it answers exactly as
            // the step did; an empty list answers `.Ok(init)`. Each step takes
            // the accumulator's count and answers another inside its `.Ok`, so
            // moving the payload out takes no count either.
            Step::FoldResult => {
                let init = *vals.get(call.init?)?;
                let init_ty = *tys.get(call.init?)?;
                let acc_t = self.type_of(&init_ty);
                let ok = self.variant_of(ret, "Ok", 0);
                let ok_tag = self.int(Type::I32, ok as usize);
                let w = self.walk(n, &[init]);
                let acc = *w.vars.first()?;
                let e = element(self, w.i);
                let a = Arg { value: acc, ty: init_ty, owned: true };
                let r = self.step(&callee, with_ctx(&ctx, vec![a, e]), step_t);
                let tag = self.emit(Type::I32, |dest| Inst::GetTag { dest, agg: r });
                let failed = self.emit(Type::I1, |dest| Inst::Binary {
                    dest,
                    op: BinOp::Ne,
                    prim: Prim::I32,
                    lhs: tag,
                    rhs: ok_tag,
                });
                let exit = self.block(&[ret_t]);
                let carry = self.block(&[]);
                self.set_term(Term::Branch {
                    cond: failed,
                    then: Target::new(exit, vec![r]),
                    else_: Target::to(carry),
                });
                self.cur = carry;
                let next = self.emit(acc_t, |dest| Inst::GetPayload {
                    dest,
                    agg: r,
                    variant: ok,
                    index: 0,
                });
                self.again(&w, vec![next]);
                self.end(&w);
                let done =
                    self.emit(ret_t, |dest| Inst::MakeEnum { dest, variant: ok, fields: vec![acc] });
                self.set_term(Term::Jump(Target::new(exit, vec![done])));
                self.cur = exit;
                *self.code.get(exit).params.first()?
            }
            Step::Sort => self.list_sort(xs, &elem, n, &callee, ret_t, &step_ret)?,
        })
    }

    /// `zip(xs, ctx, ys)`: one block of pairs, as long as the shorter list.
    /// Both halves of every pair are a second owner of what they hold.
    fn list_zip(&mut self, vals: &[ValueId], tys: &[Ty], ret: &Ty) -> Option<ValueId> {
        let (&xs, &ys) = (vals.first()?, vals.get(2)?);
        let (TyKind::Array(a), TyKind::Array(b)) = (tys.first()?.kind(), tys.get(2)?.kind()) else { return None };
        let (a, b) = (*a, *b);
        let TyKind::Array(pair) = ret.kind() else { return None };
        let (a_t, b_t, pair_t, ret_t) =
            (self.type_of(&a), self.type_of(&b), self.type_of(pair), self.type_of(ret));
        let la = self.emit(Type::I64, |dest| Inst::ArrayLen { dest, array: xs });
        let lb = self.emit(Type::I64, |dest| Inst::ArrayLen { dest, array: ys });
        let n = self.shorter(la, lb);
        let out = self.emit(ret_t, |dest| Inst::ArrayAlloc { dest, len: n });
        let w = self.walk(n, &[]);
        let x = self.emit(a_t, |dest| Inst::ArrayGet { dest, array: xs, index: w.i });
        let y = self.emit(b_t, |dest| Inst::ArrayGet { dest, array: ys, index: w.i });
        for (v, t) in [(x, &a), (y, &b)] {
            if self.counts.counted(t) {
                self.push(Inst::IncRef { value: v });
            }
        }
        let p = self.emit(pair_t, |dest| Inst::MakeStruct { dest, fields: vec![x, y] });
        self.push(Inst::ArraySet { array: out, index: w.i, value: p });
        self.again(&w, Vec::new());
        self.end(&w);
        Some(out)
    }

    /// `flatten(xs, ctx)`: one block holding every element of every inner
    /// list, sized by a first pass over the inner lengths. Each element copied
    /// out is a second owner; the inner lists themselves are only read.
    fn list_flatten(&mut self, vals: &[ValueId], tys: &[Ty], ret: &Ty) -> Option<ValueId> {
        let xs = *vals.first()?;
        let TyKind::Array(inner) = (tys.first()?).kind() else { return None };
        let TyKind::Array(elem) = inner.kind() else { return None };
        let (inner, elem) = (*inner, *elem);
        let (inner_t, elem_t, ret_t) =
            (self.type_of(&inner), self.type_of(&elem), self.type_of(ret));
        let counted = self.counts.counted(&elem);
        let n = self.emit(Type::I64, |dest| Inst::ArrayLen { dest, array: xs });
        let zero = self.int(Type::I64, 0);
        let sizing = self.walk(n, &[zero]);
        let total = *sizing.vars.first()?;
        let ys = self.emit(inner_t, |dest| Inst::ArrayGet { dest, array: xs, index: sizing.i });
        let l = self.emit(Type::I64, |dest| Inst::ArrayLen { dest, array: ys });
        let grown = self.add(total, l);
        self.again(&sizing, vec![grown]);
        self.end(&sizing);

        let out = self.emit(ret_t, |dest| Inst::ArrayAlloc { dest, len: total });
        let zero = self.int(Type::I64, 0);
        let outer = self.walk(n, &[zero]);
        let k = *outer.vars.first()?;
        let ys = self.emit(inner_t, |dest| Inst::ArrayGet { dest, array: xs, index: outer.i });
        let l = self.emit(Type::I64, |dest| Inst::ArrayLen { dest, array: ys });
        let one = self.walk(l, &[]);
        let e = self.emit(elem_t, |dest| Inst::ArrayGet { dest, array: ys, index: one.i });
        if counted {
            self.push(Inst::IncRef { value: e });
        }
        let at = self.add(k, one.i);
        self.push(Inst::ArraySet { array: out, index: at, value: e });
        self.again(&one, Vec::new());
        self.end(&one);
        let after = self.add(k, l);
        self.again(&outer, vec![after]);
        self.end(&outer);
        Some(out)
    }

    /// `deriveArrayEq` and `deriveArrayCompare`: the derived `Equal` and
    /// `Ordered` of a `[T]` field, through the element's own generated
    /// function (`middle::derives`). Equality refuses two lengths first;
    /// order is lexicographic over the shorter length, and where every shared
    /// element is `Equal` the lengths decide.
    fn derive_array(
        &mut self,
        equal: bool,
        vals: &[ValueId],
        tys: &[Ty],
        step: Option<FuncIdx>,
        ret: &Ty,
    ) -> Option<ValueId> {
        let (&xs, &ys, &f) = (vals.first()?, vals.get(1)?, vals.get(2)?);
        let TyKind::Array(elem) = (tys.first()?).kind() else { return None };
        let elem = *elem;
        let elem_t = self.type_of(&elem);
        let ret_t = self.type_of(ret);
        let callee = self.callee(f, step, 2);
        let la = self.emit(Type::I64, |dest| Inst::ArrayLen { dest, array: xs });
        let lb = self.emit(Type::I64, |dest| Inst::ArrayLen { dest, array: ys });
        let exit = self.block(&[ret_t]);
        let pair = |l: &mut Self, i: ValueId| {
            let x = l.emit(elem_t, |dest| Inst::ArrayGet { dest, array: xs, index: i });
            let y = l.emit(elem_t, |dest| Inst::ArrayGet { dest, array: ys, index: i });
            vec![
                Arg { value: x, ty: elem, owned: false },
                Arg { value: y, ty: elem, owned: false },
            ]
        };
        if equal {
            let no = self.constant(Type::I1, Const::Bool(false));
            let yes = self.constant(Type::I1, Const::Bool(true));
            let differ = self.emit(Type::I1, |dest| Inst::Binary {
                dest,
                op: BinOp::Ne,
                prim: Prim::I64,
                lhs: la,
                rhs: lb,
            });
            let paired = self.block(&[]);
            self.set_term(Term::Branch {
                cond: differ,
                then: Target::new(exit, vec![no]),
                else_: Target::to(paired),
            });
            self.cur = paired;
            let w = self.walk(la, &[]);
            let args = pair(self, w.i);
            let same = self.step(&callee, args, Type::I1);
            let unequal = self.emit(Type::I1, |dest| Inst::Binary {
                dest,
                op: BinOp::Eq,
                prim: Prim::Bool,
                lhs: same,
                rhs: no,
            });
            let more = self.block(&[]);
            self.set_term(Term::Branch {
                cond: unequal,
                then: Target::new(exit, vec![no]),
                else_: Target::to(more),
            });
            self.cur = more;
            self.again(&w, Vec::new());
            self.end(&w);
            self.set_term(Term::Jump(Target::new(exit, vec![yes])));
        } else {
            let less = self.variant_of(ret, "Less", 0);
            let same = self.variant_of(ret, "Equal", 1);
            let greater = self.variant_of(ret, "Greater", 2);
            let same_tag = self.int(Type::I32, same as usize);
            let n = self.shorter(la, lb);
            let w = self.walk(n, &[]);
            let args = pair(self, w.i);
            let order = self.step(&callee, args, ret_t);
            let tag = self.emit(Type::I32, |dest| Inst::GetTag { dest, agg: order });
            let decided = self.emit(Type::I1, |dest| Inst::Binary {
                dest,
                op: BinOp::Ne,
                prim: Prim::I32,
                lhs: tag,
                rhs: same_tag,
            });
            let more = self.block(&[]);
            self.set_term(Term::Branch {
                cond: decided,
                then: Target::new(exit, vec![order]),
                else_: Target::to(more),
            });
            self.cur = more;
            self.again(&w, Vec::new());
            self.end(&w);
            // Every shared element was `Equal`, so the shorter list is `Less`.
            for (op, v) in [(BinOp::Lt, less), (BinOp::Gt, greater)] {
                let c = self.emit(Type::I1, |dest| Inst::Binary {
                    dest,
                    op,
                    prim: Prim::I64,
                    lhs: la,
                    rhs: lb,
                });
                let (decides, next) = self.fork(c);
                self.cur = decides;
                let o = self
                    .emit(ret_t, |dest| Inst::MakeEnum { dest, variant: v, fields: Vec::new() });
                self.set_term(Term::Jump(Target::new(exit, vec![o])));
                self.cur = next;
            }
            let o =
                self.emit(ret_t, |dest| Inst::MakeEnum { dest, variant: same, fields: Vec::new() });
            self.set_term(Term::Jump(Target::new(exit, vec![o])));
        }
        self.cur = exit;
        self.code.get(exit).params.first().copied()
    }

    /// `deriveArrayShow`: `[a, b]`, from the element's own generated `show`.
    /// Each element is rendered into a `[Str]` built here, the runtime joins
    /// them (`buri_rt_show_list`), and the `[Str]` goes, every rendering with
    /// it.
    fn derive_show(
        &mut self,
        vals: &[ValueId],
        tys: &[Ty],
        step: Option<FuncIdx>,
        ret: &Ty,
    ) -> Option<ValueId> {
        let (&xs, &f) = (vals.first()?, vals.get(1)?);
        let TyKind::Array(elem) = (tys.first()?).kind() else { return None };
        let elem = *elem;
        let shown = Ty::array(*ret);
        let (elem_t, ret_t, shown_t) =
            (self.type_of(&elem), self.type_of(ret), self.type_of(&shown));
        let callee = self.callee(f, step, 1);
        let n = self.emit(Type::I64, |dest| Inst::ArrayLen { dest, array: xs });
        let strs = self.emit(shown_t, |dest| Inst::ArrayAlloc { dest, len: n });
        let w = self.walk(n, &[]);
        let e = self.emit(elem_t, |dest| Inst::ArrayGet { dest, array: xs, index: w.i });
        let s = self.step(&callee, vec![Arg { value: e, ty: elem, owned: false }], ret_t);
        self.push(Inst::ArraySet { array: strs, index: w.i, value: s });
        self.again(&w, Vec::new());
        self.end(&w);
        let joined = self.emit(ret_t, |dest| Inst::CallIntrinsic {
            dest,
            key: "show.list".into(),
            args: Box::new([strs]),
        });
        self.push(Inst::DecRef { value: strs, drop: None });
        Some(joined)
    }

    /// `sortBy`: a scan for a list already in order, then runs of [`SORT_RUN`]
    /// sorted by insertion, then a **stable bottom-up merge** over two blocks,
    /// which take turns being read and written. Every step keeps the left
    /// element unless the comparator answers `Greater`, which is what makes the
    /// sort stable.
    ///
    /// - A list in order is its copy, and a strictly descending one its copy
    ///   from the end, so the scan's comparisons are the whole cost.
    /// - Two runs in order are copied rather than merged, and two whose right
    ///   run sorts wholly before the left are copied swapped.
    ///
    /// Both blocks start as a copy of the source, each a second owner of every
    /// element. Insertion permutes the first in place, and a pass *moves*
    /// elements between them, so each holds every element exactly once after
    /// every pass, the block read last is the answer, and the other goes back
    /// with its own release of each. The two copies are why the block that goes
    /// back is never one nothing wrote. An element with no counts needs neither
    /// the second copy nor the release's walk.
    fn list_sort(
        &mut self,
        xs: ValueId,
        elem: &Ty,
        n: ValueId,
        callee: &Via,
        list_t: Type,
        order_ty: &Ty,
    ) -> Option<ValueId> {
        let elem_t = self.type_of(elem);
        let order_t = self.type_of(order_ty);
        let greater = self.variant_of(order_ty, "Greater", 2);
        let greater = self.int(Type::I32, greater as usize);
        let order = Order { callee, elem: *elem, order_t, greater };
        let counted = self.counts.counted(elem);
        let zero = self.int(Type::I64, 0);
        let one = self.int(Type::I64, 1);
        let two = self.int(Type::I64, 2);
        let get = |l: &mut Self, array: ValueId, index: ValueId| {
            l.emit(elem_t, |dest| Inst::ArrayGet { dest, array, index })
        };

        // The scan answers `1` for a list in order, `2` for one strictly
        // descending, and `0` at the first pair that is neither. `up` goes on
        // while no pair falls and `down` while every pair does, and a first
        // pair that falls turns `up` into `down`.
        let scanned = self.block(&[Type::I64]);
        let up = self.block(&[Type::I64]);
        let down = self.block(&[Type::I64]);
        self.set_term(Term::Jump(Target::new(up, vec![one])));
        for (scan, done, keeps_on) in [(up, one, BinOp::Ne), (down, two, BinOp::Eq)] {
            self.cur = scan;
            let at = *self.code.get(scan).params.first()?;
            let ended = self.bin(BinOp::Ge, Prim::U64, at, n);
            let next = self.block(&[]);
            self.set_term(Term::Branch {
                cond: ended,
                then: Target::new(scanned, vec![done]),
                else_: Target::to(next),
            });
            self.cur = next;
            let before = self.bin(BinOp::Sub, Prim::I64, at, one);
            let x = get(self, xs, before);
            let y = get(self, xs, at);
            let more = self.order(&order, keeps_on, x, y);
            let again = self.block(&[]);
            let stopped = self.block(&[]);
            self.set_term(Term::Branch { cond: more, then: Target::to(again), else_: Target::to(stopped) });
            self.cur = again;
            let at1 = self.add_one(at);
            self.set_term(Term::Jump(Target::new(scan, vec![at1])));
            self.cur = stopped;
            let neither = Target::new(scanned, vec![zero]);
            if scan == up {
                let first = self.bin(BinOp::Eq, Prim::I64, at, one);
                let falls = Target::new(down, vec![two]);
                self.set_term(Term::Branch { cond: first, then: falls, else_: neither });
            } else {
                self.set_term(Term::Jump(neither));
            }
        }
        self.cur = scanned;
        let shape = *self.code.get(scanned).params.first()?;

        let a0 = self.emit(list_t, |dest| Inst::ArrayAlloc { dest, len: n });
        let b0 = self.emit(list_t, |dest| Inst::ArrayAlloc { dest, len: n });
        // A descending list is copied from its end.
        let descending = self.bin(BinOp::Eq, Prim::I64, shape, two);
        let last = self.bin(BinOp::Sub, Prim::I64, n, one);
        let copied = self.block(&[]);
        let (backwards, forwards) = self.fork(descending);
        for (side, from_end) in [(backwards, true), (forwards, false)] {
            self.cur = side;
            let copy = self.walk(n, &[]);
            let to = if from_end { self.bin(BinOp::Sub, Prim::I64, last, copy.i) } else { copy.i };
            let e = get(self, xs, copy.i);
            self.push(Inst::ArraySet { array: a0, index: to, value: e });
            // The second copy is only for the release at the end to walk, and a
            // release walks nothing in a block of uncounted elements.
            if counted {
                self.push(Inst::IncRef { value: e });
                self.push(Inst::IncRef { value: e });
                self.push(Inst::ArraySet { array: b0, index: to, value: e });
            }
            self.again(&copy, Vec::new());
            self.end(&copy);
            self.set_term(Term::Jump(Target::to(copied)));
        }
        self.cur = copied;
        let widths = self.block(&[Type::I64, list_t, list_t]);
        let unsorted = self.bin(BinOp::Eq, Prim::I64, shape, zero);
        let runs_start = self.block(&[]);
        self.set_term(Term::Branch {
            cond: unsorted,
            then: Target::to(runs_start),
            else_: Target::new(widths, vec![n, a0, b0]),
        });
        self.cur = runs_start;

        // Each run of `SORT_RUN` in `a0`, by insertion: `a0[i]` moves left
        // past every element of its run that is `Greater`.
        let run_mask = self.int(Type::I64, SORT_RUN - 1);
        let ins = self.walk(n, &[]);
        let i = ins.i;
        let into_run = self.bin(BinOp::BitAnd, Prim::I64, i, run_mask);
        let run_lo = self.bin(BinOp::Sub, Prim::I64, i, into_run);
        let first = self.bin(BinOp::Eq, Prim::I64, into_run, zero);
        let inserted = self.block(&[]);
        let insert = self.block(&[]);
        self.set_term(Term::Branch { cond: first, then: Target::to(inserted), else_: Target::to(insert) });
        self.cur = insert;
        let x = get(self, a0, i);
        let shift = self.block(&[Type::I64]);
        self.set_term(Term::Jump(Target::new(shift, vec![i])));
        self.cur = shift;
        let j = *self.code.get(shift).params.first()?;
        let at_lo = self.bin(BinOp::Ge, Prim::U64, run_lo, j);
        let place = self.block(&[Type::I64]);
        let look = self.block(&[]);
        self.set_term(Term::Branch { cond: at_lo, then: Target::new(place, vec![j]), else_: Target::to(look) });
        self.cur = look;
        let j1 = self.bin(BinOp::Sub, Prim::I64, j, one);
        let y = get(self, a0, j1);
        let stays = self.order(&order, BinOp::Ne, y, x);
        let moved = self.block(&[]);
        self.set_term(Term::Branch { cond: stays, then: Target::new(place, vec![j]), else_: Target::to(moved) });
        self.cur = moved;
        self.push(Inst::ArraySet { array: a0, index: j, value: y });
        self.set_term(Term::Jump(Target::new(shift, vec![j1])));
        self.cur = place;
        let k = *self.code.get(place).params.first()?;
        self.push(Inst::ArraySet { array: a0, index: k, value: x });
        self.set_term(Term::Jump(Target::to(inserted)));
        self.cur = inserted;
        self.again(&ins, Vec::new());
        self.end(&ins);

        // `w = SORT_RUN, 2 * SORT_RUN, ...`, reading `a` and writing `b`.
        let run_len = self.int(Type::I64, SORT_RUN);
        self.set_term(Term::Jump(Target::new(widths, vec![run_len, a0, b0])));
        self.cur = widths;
        let [w, a, b] = self.code.get(widths).params.as_slice() else { return None };
        let (w, a, b) = (*w, *a, *b);
        let sorted = self.bin(BinOp::Ge, Prim::U64, w, n);
        let exit = self.block(&[]);
        let pass = self.block(&[]);
        self.set_term(Term::Branch { cond: sorted, then: Target::to(exit), else_: Target::to(pass) });
        self.cur = pass;
        let span = self.add(w, w);

        // One pass: `lo = 0, 2w, 4w, ...`.
        let runs = self.block(&[Type::I64]);
        self.set_term(Term::Jump(Target::new(runs, vec![zero])));
        self.cur = runs;
        let lo = *self.code.get(runs).params.first()?;
        let passed = self.bin(BinOp::Ge, Prim::U64, lo, n);
        let swap = self.block(&[]);
        let run = self.block(&[]);
        self.set_term(Term::Branch { cond: passed, then: Target::to(swap), else_: Target::to(run) });
        self.cur = swap;
        self.set_term(Term::Jump(Target::new(widths, vec![span, b, a])));
        self.cur = run;
        let lo_w = self.add(lo, w);
        let mid = self.shorter(lo_w, n);
        let lo_span = self.add(lo, span);
        let hi = self.shorter(lo_span, n);
        let next_run = self.block(&[]);
        let copied = self.block(&[]);
        let paired = self.block(&[]);
        let crossed = self.block(&[]);
        let swapped = self.block(&[]);
        let merging = self.block(&[]);
        // A run with no partner to its right, or two in order, is copied.
        let lone = self.bin(BinOp::Ge, Prim::U64, mid, hi);
        self.set_term(Term::Branch { cond: lone, then: Target::to(copied), else_: Target::to(paired) });
        self.cur = paired;
        let mid1 = self.bin(BinOp::Sub, Prim::I64, mid, one);
        let left_last = get(self, a, mid1);
        let right_first = get(self, a, mid);
        let in_order = self.order(&order, BinOp::Ne, left_last, right_first);
        self.set_term(Term::Branch { cond: in_order, then: Target::to(copied), else_: Target::to(crossed) });
        // The right run's last sorts before the left run's first, so every
        // right element sorts strictly before every left one.
        self.cur = crossed;
        let hi1 = self.bin(BinOp::Sub, Prim::I64, hi, one);
        let left_first = get(self, a, lo);
        let right_last = get(self, a, hi1);
        let reversed = self.order(&order, BinOp::Eq, left_first, right_last);
        self.set_term(Term::Branch { cond: reversed, then: Target::to(swapped), else_: Target::to(merging) });

        // One copy loop serves every run moved whole: `len` elements from
        // `a[from..]` to `b[to..]`, then `rest` more from `a[rest_from..]`.
        let copy = self.block(&[Type::I64; 5]);
        self.cur = copied;
        let len = self.bin(BinOp::Sub, Prim::I64, hi, lo);
        self.set_term(Term::Jump(Target::new(copy, vec![lo, lo, len, zero, zero])));
        self.cur = swapped;
        let right_len = self.bin(BinOp::Sub, Prim::I64, hi, mid);
        let left_len = self.bin(BinOp::Sub, Prim::I64, mid, lo);
        self.set_term(Term::Jump(Target::new(copy, vec![mid, lo, right_len, left_len, lo])));
        self.cur = copy;
        let [from, to, len, rest, rest_from] = self.code.get(copy).params.as_slice() else { return None };
        let (from, to, len, rest, rest_from) = (*from, *to, *len, *rest, *rest_from);
        let moving = self.walk(len, &[]);
        let at = self.add(from, moving.i);
        let e = get(self, a, at);
        let dest_at = self.add(to, moving.i);
        self.push(Inst::ArraySet { array: b, index: dest_at, value: e });
        self.again(&moving, Vec::new());
        self.end(&moving);
        let after = self.add(to, len);
        let done = self.bin(BinOp::Eq, Prim::I64, rest, zero);
        self.set_term(Term::Branch {
            cond: done,
            then: Target::to(next_run),
            else_: Target::new(copy, vec![rest_from, after, rest, zero, zero]),
        });
        self.cur = next_run;
        self.set_term(Term::Jump(Target::new(runs, vec![lo_span])));
        self.cur = merging;

        // One merge: `a[lo..mid)` and `a[mid..hi)` into `b[lo..hi)`, both
        // non-empty. Once one side runs out, the copy loop moves the other.
        let merge = self.block(&[Type::I64; 3]);
        self.set_term(Term::Jump(Target::new(merge, vec![lo, mid, lo])));
        self.cur = merge;
        let [li, ri, out] = self.code.get(merge).params.as_slice() else { return None };
        let (li, ri, out) = (*li, *ri, *out);
        let x = get(self, a, li);
        let y = get(self, a, ri);
        let right_first = self.order(&order, BinOp::Eq, x, y);
        let (take_right, take_left) = self.fork(right_first);
        let out1 = self.add_one(out);
        for (side, from, end) in [(take_left, li, mid), (take_right, ri, hi)] {
            self.cur = side;
            // Read again rather than kept across the call: the copy-and-patch
            // backend runs about 5% faster on a shuffled list.
            let e = get(self, a, from);
            self.push(Inst::ArraySet { array: b, index: out, value: e });
            let from1 = self.add_one(from);
            let ran_out = self.bin(BinOp::Ge, Prim::U64, from1, end);
            let more = if side == take_left { vec![from1, ri, out1] } else { vec![li, from1, out1] };
            let tail = self.block(&[]);
            self.set_term(Term::Branch { cond: ran_out, then: Target::to(tail), else_: Target::new(merge, more) });
            // The other side's rest, copied.
            self.cur = tail;
            let (other, other_end) = if side == take_left { (ri, hi) } else { (li, mid) };
            let rest = self.bin(BinOp::Sub, Prim::I64, other_end, other);
            self.set_term(Term::Jump(Target::new(copy, vec![other, out1, rest, zero, zero])));
        }

        self.cur = exit;
        self.push(Inst::DecRef { value: b, drop: None });
        Some(a)
    }

    /// `order(x, y)`'s tag `op` `Greater`: [`BinOp::Eq`] asks whether `y` sorts
    /// strictly before `x`, and [`BinOp::Ne`] whether it doesn't.
    fn order(&mut self, order: &Order<'_>, op: BinOp, x: ValueId, y: ValueId) -> ValueId {
        let args = vec![
            Arg { value: x, ty: order.elem, owned: false },
            Arg { value: y, ty: order.elem, owned: false },
        ];
        let o = self.step(order.callee, args, order.order_t);
        let tag = self.emit(Type::I32, |dest| Inst::GetTag { dest, agg: o });
        self.bin(op, Prim::I32, tag, order.greater)
    }

    /// `lhs op rhs`: a `Bool` for a comparison, and an `I64` otherwise.
    fn bin(&mut self, op: BinOp, prim: Prim, lhs: ValueId, rhs: ValueId) -> ValueId {
        let ty = if op.is_comparison() { Type::I1 } else { Type::I64 };
        self.emit(ty, |dest| Inst::Binary { dest, op, prim, lhs, rhs })
    }

    /// The shorter of two lengths, as a join.
    fn shorter(&mut self, a: ValueId, b: ValueId) -> ValueId {
        let first = self.emit(Type::I1, |dest| Inst::Binary {
            dest,
            op: BinOp::Lt,
            prim: Prim::I64,
            lhs: a,
            rhs: b,
        });
        let join = self.block(&[Type::I64]);
        let (left, right) = self.fork(first);
        self.cur = left;
        self.set_term(Term::Jump(Target::new(join, vec![a])));
        self.cur = right;
        self.set_term(Term::Jump(Target::new(join, vec![b])));
        self.cur = join;
        self.code.get(join).params.first().copied().unwrap_or(a)
    }

    fn add(&mut self, a: ValueId, b: ValueId) -> ValueId {
        self.emit(Type::I64, |dest| Inst::Binary {
            dest,
            op: BinOp::Add,
            prim: Prim::I64,
            lhs: a,
            rhs: b,
        })
    }

    /// `list.get(xs, i)`: the bounds test and the load `xs[i]` lowers to, with
    /// the retain that makes the `Option` a second owner of the element.
    ///
    /// One unsigned comparison covers both bounds: a negative index is a very
    /// large unsigned one. The element is the `else` arm, for [`FnLower::walk`]'s
    /// reason.
    fn list_get(&mut self, vals: &[ValueId], tys: &[Ty], ret: &Ty) -> Option<ValueId> {
        let (&xs, &i) = (vals.first()?, vals.get(1)?);
        let TyKind::Array(elem) = (tys.first()?).kind() else { return None };
        let elem = *elem;
        let (elem_t, ret_t) = (self.type_of(&elem), self.type_of(ret));
        let some = self.variant_of(ret, "Some", 0);
        let none = self.variant_of(ret, "None", 1);
        let n = self.emit(Type::I64, |dest| Inst::ArrayLen { dest, array: xs });
        let outside = self.emit(Type::I1, |dest| Inst::Binary {
            dest,
            op: BinOp::Ge,
            prim: Prim::U64,
            lhs: i,
            rhs: n,
        });
        let exit = self.block(&[ret_t]);
        let (missing, found) = self.fork(outside);
        self.cur = found;
        let e = self.emit(elem_t, |dest| Inst::ArrayGet { dest, array: xs, index: i });
        if self.counts.counted(&elem) {
            self.push(Inst::IncRef { value: e });
        }
        let v = self.emit(ret_t, |dest| Inst::MakeEnum { dest, variant: some, fields: vec![e] });
        self.set_term(Term::Jump(Target::new(exit, vec![v])));
        self.cur = missing;
        let v = self.emit(ret_t, |dest| Inst::MakeEnum { dest, variant: none, fields: Vec::new() });
        self.set_term(Term::Jump(Target::new(exit, vec![v])));
        self.cur = exit;
        self.code.get(exit).params.first().copied()
    }

    /// How the loop calls its step: directly where the call site names a
    /// function this lowering can call, through the thunk otherwise.
    fn callee(&mut self, f: ValueId, step: Option<FuncIdx>, want: usize) -> Via {
        let thunk = Via::Thunk(f);
        let Some(func) = step else { return thunk };
        let Some(g) = self.program.funcs.get(func.index()) else { return thunk };
        // An intrinsic has no body to call, and a merged tail-call group takes
        // a dispatch index its signature does not show.
        if !matches!(g.kind, FuncKind::Body(_)) || self.entries.get(func.index()).copied().unwrap_or(0) > 1 {
            return thunk;
        }
        let env = match g.params.len() {
            n if n == want => None,
            // A lambda that captured nothing still takes its environment
            // first, as an empty tuple (`middle::closures`).
            n if Some(n) == want.checked_add(1) => {
                let ty = g.params.first().and_then(|p| g.locals.get(p.index())).map(|l| l.ty);
                match ty {
                    Some(t) if t == Ty::tuple([]) => {
                        let env_t = self.type_of(&t);
                        Some(self.emit(env_t, |dest| Inst::MakeStruct { dest, fields: Vec::new() }))
                    }
                    _ => return thunk,
                }
            }
            _ => return thunk,
        };
        // The column `lower::facts` copies onto the callee, and the
        // conservative answer where it would not trust the plan.
        let own = match self.plan.func(func) {
            Some(p) if p.params.len() == g.params.len() => p.params.clone(),
            _ => vec![Ownership::Own; g.params.len()],
        };
        let own = own.into_iter().skip(usize::from(env.is_some())).collect();
        Via::Direct { func, env, own }
    }

    /// One call of the step, with the counts its convention needs on each side.
    fn step(&mut self, callee: &Via, args: Vec<Arg>, ret: Type) -> ValueId {
        let owns = |i: usize| match callee {
            Via::Direct { own, .. } => own.get(i).copied().unwrap_or(Ownership::Own),
            Via::Thunk(_) => Ownership::Own,
        };
        let mut release = Vec::new();
        for (i, a) in args.iter().enumerate() {
            match (owns(i), a.owned) {
                (Ownership::Own, false) if self.counts.counted(&a.ty) => {
                    self.push(Inst::IncRef { value: a.value });
                }
                (Ownership::Borrow, true) if self.counts.counted(&a.ty) => release.push(a.value),
                _ => {}
            }
        }
        let values: Vec<ValueId> = args.iter().map(|a| a.value).collect();
        let r = match callee {
            Via::Direct { func, env, .. } => {
                let mut all: Vec<ValueId> = env.iter().copied().collect();
                all.extend(values);
                let func = *func;
                self.emit(ret, |dest| Inst::Call { dest, func, args: all })
            }
            Via::Thunk(f) => {
                let callee = *f;
                self.emit(ret, |dest| Inst::CallIndirect { dest, callee, args: values })
            }
        };
        for value in release {
            self.push(Inst::DecRef { value, drop: None });
        }
        r
    }

    /// Opens `for i in 0..n`, carrying `carried`, and leaves the lowering in
    /// the body. The header's branch is set by [`FnLower::end`], once the
    /// block after the loop exists.
    ///
    /// Every branch a loop here makes puts the path that **continues** the
    /// loop in its `else` arm. The stencil backend lays blocks out in reverse
    /// postorder, `then` first, which puts the `else` arm right after the
    /// test, and a branch to the block laid out next costs nothing.
    fn walk(&mut self, n: ValueId, carried: &[ValueId]) -> Walk {
        let zero = self.int(Type::I64, 0);
        let mut types = vec![Type::I64];
        types.extend(carried.iter().map(|v| self.code.ty_of(*v)));
        let header = self.block(&types);
        let mut first = vec![zero];
        first.extend_from_slice(carried);
        self.set_term(Term::Jump(Target::new(header, first)));
        self.cur = header;
        let params = self.code.get(header).params.clone();
        let (i, vars) = match params.split_first() {
            Some((i, vars)) => (*i, vars.to_vec()),
            None => (zero, Vec::new()),
        };
        // Unsigned, which is the same answer for an index and a length, and
        // is the comparison the stencil backend fuses with the increment.
        let ended = self.emit(Type::I1, |dest| Inst::Binary {
            dest,
            op: BinOp::Ge,
            prim: Prim::U64,
            lhs: i,
            rhs: n,
        });
        let body = self.block(&[]);
        self.cur = body;
        Walk { header, body, i, vars, ended }
    }

    /// The back edge, from the current block, with the next iteration's
    /// carried values. Each arm of a body that forks takes its own, so that
    /// every carried value is a temporary the backend can give its parameter's
    /// slot.
    fn again(&mut self, w: &Walk, carried: Vec<ValueId>) {
        let i1 = self.add_one(w.i);
        let mut args = vec![i1];
        args.extend(carried);
        self.set_term(Term::Jump(Target::new(w.header, args)));
    }

    /// Closes the loop, and leaves the lowering in the block after it.
    fn end(&mut self, w: &Walk) {
        let done = self.block(&[]);
        self.code.get_mut(w.header).term =
            Term::Branch { cond: w.ended, then: Target::to(done), else_: Target::to(w.body) };
        self.cur = done;
    }

    /// Two arms on `cond`: `(then, else)`.
    fn fork(&mut self, cond: ValueId) -> (BlockId, BlockId) {
        let then = self.block(&[]);
        let else_ = self.block(&[]);
        self.set_term(Term::Branch { cond, then: Target::to(then), else_: Target::to(else_) });
        (then, else_)
    }

    fn add_one(&mut self, v: ValueId) -> ValueId {
        let one = self.int(Type::I64, 1);
        self.emit(Type::I64, |dest| Inst::Binary { dest, op: BinOp::Add, prim: Prim::I64, lhs: v, rhs: one })
    }
}
