//! Which functions a unit other than their own names.
//!
//! The LLVM backend drops a unit's function once nothing in the unit uses it
//! and no other unit names it. So the answer here is an input to a unit's
//! object, and `build::actions::unit_hashes` folds it into the unit's key: a
//! cached object that dropped a function must not serve a program in which
//! another unit calls it.

use crate::compiler::middle::ir::{Body, Inst, Program};
use crate::compiler::semantics::layered::Dense;
use crate::compiler::semantics::types::FuncIdx;

/// One flag per function: whether a function in another unit calls
/// it, makes a closure of it, or drops a count through it, or whether the
/// runtime compares a cell's values with it.
///
/// The program's roots aren't here. A backend keeps those itself.
pub fn named_elsewhere(program: &Program) -> Dense<FuncIdx, bool> {
    named_from(program, false)
}

/// [`named_elsewhere`], counting a function's own unit too. A function no
/// one names, `middle::inline` having inlined it at every call, needs no
/// code at all.
pub fn named_anywhere(program: &Program) -> Dense<FuncIdx, bool> {
    named_from(program, true)
}

fn named_from(program: &Program, own_unit: bool) -> Dense<FuncIdx, bool> {
    let mut named = program.funcs.map(|_| false);
    for f in program.funcs.iter() {
        let Body::Code(code) = &f.body else { continue };
        for inst in code.blocks().iter().flat_map(|b| &b.insts) {
            let callee = match inst {
                Inst::Call { func, .. } | Inst::MakeClosure { func, .. } => *func,
                _ => continue,
            };
            if own_unit || program.funcs.get(callee).unit != f.unit {
                *named.get_mut(callee) = true;
            }
        }
    }
    // The comparison thunk a signal's write needs is built in whichever unit
    // writes it.
    for func in program.cell_equal.values() {
        *named.get_mut(*func) = true;
    }
    named
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::middle::ir::{
        BlockId, Code, Facts, Func, Purity, Signature, Term, Type, TypeList, Unit,
    };
    use crate::diagnostics::Span;

    /// The id the table mints for its function at `i`.
    fn id(i: u32) -> FuncIdx {
        let table: Dense<FuncIdx, ()> = std::iter::repeat_n((), i as usize + 1).collect();
        table
            .ids()
            .last()
            .expect("the table holds `i + 1` functions")
    }

    /// A function in `unit` that calls each of `calls`.
    fn func(unit: u32, calls: &[u32]) -> Func {
        let mut code = Code::new(&[]);
        let entry = BlockId::ENTRY;
        for c in calls {
            let dest = code.value(Type::I64);
            code.get_mut(entry).insts.push(Inst::Call {
                dest,
                func: id(*c),
                args: Vec::new(),
            });
        }
        let nothing = code.value(Type::Unit);
        code.get_mut(entry).term = Term::Return(nothing);
        Func {
            symbol: String::new(),
            debug_name: String::new(),
            sig: Signature {
                params: Vec::new(),
                ret: Type::Unit,
            },
            facts: Facts {
                params: Vec::new(),
                purity: Purity::Pure,
                can_abort: false,
            },
            unit,
            body: Body::Code(code),
            span: Span::NONE,
        }
    }

    /// A call from another unit names its callee elsewhere. A call from the
    /// callee's own unit names it only anywhere.
    #[test]
    fn only_a_call_across_units_names_a_function() {
        let program = Program {
            funcs: [func(0, &[1, 2]), func(0, &[]), func(1, &[]), func(1, &[0])]
                .into_iter()
                .collect(),
            units: ["a", "b"]
                .map(|name| Unit {
                    name: name.into(),
                    funcs: Vec::new(),
                })
                .into(),
            types: TypeList::default(),
            crosses_tasks: false,
            cell_equal: Default::default(),
            intrinsics: Default::default(),
            runtimes: Default::default(),
        };
        assert_eq!(
            named_elsewhere(&program).as_slice(),
            [true, false, true, false]
        );
        assert_eq!(
            named_anywhere(&program).as_slice(),
            [true, true, true, false]
        );
    }
}
