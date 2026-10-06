//! Which functions a unit other than their own names.
//!
//! The LLVM backend drops a unit's function once nothing in the unit uses it
//! and no other unit names it. So the answer here is an input to a unit's
//! object, and `build::actions::unit_hashes` folds it into the unit's key: a
//! cached object that dropped a function must not serve a program in which
//! another unit calls it.

use crate::compiler::middle::ir::{Body, Inst, Program};

/// One flag per function, by index: whether a function in another unit calls
/// it, makes a closure of it, or drops a count through it, or whether the
/// runtime compares a cell's values with it.
///
/// The program's roots aren't here. A backend keeps those itself.
pub fn named_elsewhere(program: &Program) -> Vec<bool> {
    let mut named = vec![false; program.funcs.len()];
    let mut mark = |at: usize| {
        if let Some(slot) = named.get_mut(at) {
            *slot = true;
        }
    };
    for f in &program.funcs {
        let Body::Code(code) = &f.body else { continue };
        for inst in code.blocks.iter().flat_map(|b| &b.insts) {
            let callee = match inst {
                Inst::Call { func, .. } | Inst::MakeClosure { func, .. } => *func,
                Inst::DecRef { drop: Some(func), .. } => *func,
                _ => continue,
            };
            if program.funcs.get(callee.index()).is_some_and(|c| c.unit != f.unit) {
                mark(callee.index());
            }
        }
    }
    // The comparison thunk a signal's write needs is built in whichever unit
    // writes it.
    for func in program.cell_equal.values() {
        mark(func.index());
    }
    named
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::middle::ir::{Code, Facts, Func, Purity, Signature, Term, Type};
    use crate::compiler::semantics::types::FuncIdx;
    use crate::diagnostics::Span;

    /// A function in `unit` that calls each of `calls`.
    fn func(unit: u32, calls: &[u32]) -> Func {
        let mut code = Code::new();
        let entry = code.block(&[]);
        for c in calls {
            let dest = code.value(Type::I64);
            code.get_mut(entry).insts.push(Inst::Call { dest, func: FuncIdx(*c), args: Vec::new() });
        }
        code.get_mut(entry).term = Term::Return(Vec::new());
        Func {
            symbol: String::new(),
            debug_name: String::new(),
            sig: Signature { params: Vec::new(), rets: Vec::new() },
            facts: Facts { params: Vec::new(), purity: Purity::Pure, can_abort: false },
            unit,
            body: Body::Code(code),
            span: Span::NONE,
        }
    }

    /// A call from another unit names its callee. A call from the callee's own
    /// unit doesn't.
    #[test]
    fn only_a_call_across_units_names_a_function() {
        let program = Program {
            funcs: vec![func(0, &[1, 2]), func(0, &[]), func(1, &[]), func(1, &[0])],
            units: vec!["a".into(), "b".into()],
            types: Vec::new(),
            crosses_tasks: false,
            cell_equal: Default::default(),
        };
        assert_eq!(named_elsewhere(&program), vec![true, false, true, false]);
    }
}
