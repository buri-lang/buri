//! `core/list`'s closure operations, lowered to loops in the IR.
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
//! `map`, `filter`, `zip` and `flatten` build their answer in a fresh block:
//! [`Inst::ArrayAlloc`], then one [`Inst::ArraySet`] per element, which moves
//! the element's counts in. `filter` ends with [`Inst::ArrayPrefix`], because
//! it fills only the elements it keeps. A loop stores only into a block it
//! allocated, which nothing else holds yet, so no other list can see the
//! writes (MEMORY.md §5.3).

use std::sync::{Mutex, OnceLock};

use super::FnLower;
use crate::compiler::backend::intrinsic_keys::{self, Step};
use crate::compiler::middle::ir::{
    BinOp, BlockId, Code, Const, Inst, Ownership, Signature, Target, Term, Type, UnOp, ValueId,
};
use crate::compiler::middle::monomorphize::{FuncKind, Program};
use crate::compiler::middle::rc;
use crate::compiler::semantics::typed::{Callee, Expr, ExprKind};
use crate::compiler::semantics::types::{FuncIdx, LocalId, Prim, Ty};

/// Whether `key` is lowered here.
pub(super) fn handles(key: &str) -> bool {
    key == "list.get" || intrinsic_keys::list_call(key).is_some_and(|c| c.kind != Step::Sort)
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
        let tys: Vec<Ty> = args.iter().map(|a| a.ty.clone()).collect();
        // The step a call site names: a lambda `middle::closures` lifted, or a
        // function passed by name.
        let step = intrinsic_keys::list_call(key)
            .and_then(|c| args.get(c.func))
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
            .map(|p| self.locals.get(p.index()).map(|l| l.ty.clone()).unwrap_or(Ty::Unit))
            .collect();
        let ret = self.ret.clone();
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
        if key == "list.get" {
            return self.list_get(vals, tys, ret);
        }
        let call = intrinsic_keys::list_call(key)?;
        let xs = *vals.first()?;
        let elem = match tys.first()? {
            Ty::Array(e) => (**e).clone(),
            _ => return None,
        };
        let f = *vals.get(call.func)?;
        let Ty::Fn(params, step_ret) = tys.get(call.func)? else { return None };
        let step_ret = (**step_ret).clone();
        let callee = self.callee(f, step, params.len());
        let ctx = match call.ctx {
            Some(c) => Some(Arg { value: *vals.get(c)?, ty: tys.get(c)?.clone(), owned: false }),
            None => None,
        };
        let n = self.emit(Type::I64, |dest| Inst::ArrayLen { dest, array: xs });
        let elem_t = self.type_of(&elem);
        let step_t = self.type_of(&step_ret);
        let ret_t = self.type_of(ret);
        let element = |l: &mut Self, i: ValueId| {
            let value = l.emit(elem_t, |dest| Inst::ArrayGet { dest, array: xs, index: i });
            Arg { value, ty: elem.clone(), owned: false }
        };
        let with_ctx = |ctx: &Option<Arg>, rest: Vec<Arg>| -> Vec<Arg> {
            let mut all: Vec<Arg> = ctx
                .iter()
                .map(|c| Arg { value: c.value, ty: c.ty.clone(), owned: false })
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
            Step::Fold => {
                let init = *vals.get(call.init?)?;
                let init_ty = tys.get(call.init?)?.clone();
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
                let init_ty = tys.get(call.init?)?.clone();
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
            Step::Sort => return None,
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
        let Ty::Array(elem) = tys.first()? else { return None };
        let elem = (**elem).clone();
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
                let ty = g.params.first().and_then(|p| g.locals.get(p.index())).map(|l| l.ty.clone());
                match ty {
                    Some(t) if t == Ty::Tuple(Vec::new()) => {
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
                self.emit(ret, |dest| Inst::Call { dests: vec![dest], func, args: all })
            }
            Via::Thunk(f) => {
                let callee = *f;
                self.emit(ret, |dest| Inst::CallIndirect { dests: vec![dest], callee, args: values })
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
