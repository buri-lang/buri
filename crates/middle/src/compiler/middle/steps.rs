//! A list loop's small step, written out in the loop.
//!
//! `xs.map(ctx, fn(x) => x * 2 + 1)` calls the lambda once an element, and
//! `lower` makes that a direct call (`lower/lists.rs`). A call costs a frame
//! and moves each argument and the answer through the calling convention, and
//! on the copy-and-patch backend a loop that calls anything keeps its
//! variables in frame slots rather than registers. So once both functions are
//! lowered, a step whose body is one block of a few instructions is copied
//! into the loop in place of the call.
//!
//! The copy does what the call did: the counts are instructions in the step's
//! body, so it retains and releases the same values, however each parameter
//! was passed. Only a step of the caller's own codegen unit is copied, because
//! a unit's fingerprint covers its own members' bodies and no other unit's.

use crate::compiler::middle::ir::{
    Block, BlockId, Code, Func, Inst, PerValue, Term, Type, ValueId,
};
use crate::compiler::semantics::layered::Dense;
use crate::compiler::semantics::types::FuncIdx;
use crate::diagnostics::Invariant as _;

/// The most instructions a step's body may have to be copied: fields,
/// arithmetic and a struct or two, where the call was most of the cost. A
/// lifted lambda is named once, where it was written, so copying its body
/// adds nothing; a function passed by name is copied into each loop.
const SMALL: usize = 16;

/// Copies each step `sites` names into its loop ([the module docs](self)).
/// `sites` holds, for each function, the answer of each direct call one of
/// its list loops makes to its step.
pub fn inline(funcs: &mut Dense<FuncIdx, Func>, sites: &Dense<FuncIdx, Vec<ValueId>>) {
    let mut found: Vec<(BlockId, usize, Body)> = Vec::new();
    for (caller, dests) in sites.entries() {
        if dests.is_empty() {
            continue;
        }
        let f = funcs.get(caller);
        let code = f.code().or_ice("a function with list loops has blocks");
        // A call lowering made may sit in a block nothing reached, which is
        // gone now, so the calls are found by walking what is left.
        for (b, block) in code.iter() {
            for (i, inst) in block.insts.iter().enumerate() {
                let Inst::Call { dest, func, args } = inst else {
                    continue;
                };
                if !dests.contains(dest) {
                    continue;
                }
                if let Some(body) = small(f, funcs.get(*func), *dest, args) {
                    found.push((b, i, body));
                }
            }
        }
        // Last first, so each splice leaves the positions before it alone.
        let code = funcs.get_mut(caller).code_mut();
        let code = code.or_ice("a function with list loops has blocks");
        for (b, i, body) in found.drain(..).rev() {
            splice(code, b, i, body);
        }
    }
}

/// A step's body, copied out of its function, with what the call it replaces
/// passed and answered.
struct Body {
    insts: Vec<Inst>,
    /// The type of each of the step's values.
    types: PerValue<Type>,
    /// What each of the step's values is called in the loop, where that is
    /// known already: each parameter the argument the call passed, and the
    /// answer the call's own.
    names: PerValue<Option<ValueId>>,
}

/// `g`'s body, if `f` may write it out in place of a call to it that passes
/// `args` and answers `dest`: one block of at most [`SMALL`] instructions,
/// answering a value one of them defines.
fn small(f: &Func, g: &Func, dest: ValueId, args: &[ValueId]) -> Option<Body> {
    if f.unit != g.unit {
        return None;
    }
    let code = g
        .code()
        .or_ice("`lower` calls a step directly only where it has a body");
    let [Block {
        params,
        insts,
        term: Term::Return(answer),
    }] = code.blocks()
    else {
        return None;
    };
    if insts.len() > SMALL || params.contains(answer) {
        return None;
    }
    let mut names = code.per_value(None);
    for (p, a) in params.iter().zip(args) {
        *names.get_mut(*p) = Some(*a);
    }
    *names.get_mut(*answer) = Some(dest);
    Some(Body {
        insts: insts.clone(),
        types: code.per_value_from(&mut PerValue::default(), |v| code.ty_of(v)),
        names,
    })
}

/// Replaces the call at instruction `i` of block `b` with `body`. The value
/// the body answers takes the call's answer's name, so nothing that read it
/// changes.
fn splice(code: &mut Code, b: BlockId, i: usize, body: Body) {
    let Body {
        mut insts,
        types,
        mut names,
    } = body;
    for inst in &mut insts {
        // Every operand is a parameter or an earlier instruction's result,
        // both named by now.
        inst.operands_mut(|v| *v = names.get(*v).unwrap_or(*v));
        if let Some(r) = inst.results_mut() {
            let v = match *names.get(*r) {
                Some(v) => v,
                None => code.value(*types.get(*r)),
            };
            *names.get_mut(*r) = Some(v);
            *r = v;
        }
    }
    code.get_mut(b).insts.splice(i..=i, insts);
}
