//! Moving a `..rest` slice to where its list dies.
//!
//! `lower` copies the `..rest` of an array pattern as soon as the pattern
//! matches, before the arm runs. In a recursion over a list that owns its
//! argument, the arm then lets the list go and hands the copy on:
//!
//! ```text
//! b8():
//!   v15 = slice v2, v14
//!   branch v16, b9(), b10()
//! b9():
//!   decref v15
//!   jump b11(v2)
//! b10():
//!   decref v2
//!   jump b1(v15, v3)
//! ```
//!
//! Every step allocates a block, copies the tail into it and retains each
//! element, and the `decref v2` releases them all again. This pass deletes
//! the copy where nothing reads it (`b9`), and where the list dies right
//! before the copy is handed on (`b10`) it puts one slice there that takes
//! the list's count: `v20 = slice.owned v2, v14`. A list nothing else holds
//! then loses its head in place, and a shared one is copied as before
//! (`buri_rt_list_slice_owned`).
//!
//! # When it is sound
//!
//! An owned slice is a slice followed by the `decref` it replaces, and the
//! list's elements can't change while the list is held, so the copy may run
//! anywhere between the two. The pass moves it only when:
//!
//!  * every block that reads it either only drops it, or hands it on in its
//!    jump after a `decref` of the list.
//!
//! Each of those gives the copy's one count away, so no path reads it twice,
//! and each block can make its own.

use crate::compiler::middle::ir::{Block, BlockId, Code, Inst, ListTy, Target, Term, ValueId};

/// Moves every `..rest` slice in `code` that [the module docs](self) license.
pub fn sink(code: &mut Code) {
    let mut slices = Vec::new();
    for (b, block) in code.iter() {
        for inst in &block.insts {
            if let Inst::ArraySlice {
                dest,
                array,
                ty,
                from,
                owned: false,
            } = *inst
            {
                slices.push((
                    b,
                    Slice {
                        dest,
                        array,
                        ty,
                        from,
                    },
                ));
            }
        }
    }
    for (d, slice) in slices {
        sink_one(code, d, &slice);
    }
}

/// A `..rest` slice: `dest = slice array, from`.
struct Slice {
    dest: ValueId,
    array: ValueId,
    ty: ListTy,
    from: ValueId,
}

/// What a block that reads a slice does with it.
enum Use {
    /// Drops it, at this instruction.
    Drop(usize),
    /// Hands it on in this jump, after the list's `decref` at this
    /// instruction.
    Fuse(usize, Target),
}

/// Moves the slice `d` makes, if [the module docs](self) license it.
fn sink_one(code: &mut Code, d: BlockId, slice: &Slice) {
    let &Slice {
        dest: v,
        array,
        ty,
        from,
    } = slice;
    let mut uses = Vec::new();
    for (b, block) in code.iter() {
        match use_in(block, v, array) {
            Some(None) => {}
            Some(Some(u)) => uses.push((b, u)),
            None => return,
        }
    }
    for (b, u) in uses {
        match u {
            Use::Drop(j) => {
                code.get_mut(b).insts.remove(j);
            }
            Use::Fuse(j, mut jump) => {
                let owned = code.value(code.ty_of(v));
                for arg in &mut jump.args {
                    if *arg == v {
                        *arg = owned;
                    }
                }
                let block = code.get_mut(b);
                for inst in block.insts.iter_mut().skip(j).take(1) {
                    *inst = Inst::ArraySlice {
                        dest: owned,
                        array,
                        ty,
                        from,
                        owned: true,
                    };
                }
                block.term = Term::Jump(jump);
            }
        }
    }
    // Last, because a use in `d` itself was found by its index.
    code.get_mut(d)
        .insts
        .retain(|inst| !matches!(inst, Inst::ArraySlice { dest, .. } if *dest == v));
}

/// What `block` does with `v`, a slice of `array`: `Some(None)` where it
/// doesn't read it, and `None` where it reads it some way this pass leaves
/// alone.
fn use_in(block: &Block, v: ValueId, array: ValueId) -> Option<Option<Use>> {
    let mut operands = Vec::new();
    let mut read = Vec::new();
    for (j, inst) in block.insts.iter().enumerate() {
        operands.clear();
        inst.operands(&mut operands);
        if operands.contains(&v) {
            read.push(j);
        }
    }
    operands.clear();
    block.term.operands(&mut operands);
    operands.extend(block.term.targets().flat_map(|t| t.args.iter().copied()));
    let jump = match &block.term {
        Term::Jump(t) => Some(t),
        _ => None,
    };
    match (read.as_slice(), operands.contains(&v), jump) {
        ([], false, _) => Some(None),
        // A `decref` reads one value, so one that reads `v` drops `v`.
        ([j], false, _) => {
            matches!(block.insts.get(*j), Some(Inst::DecRef { .. })).then_some(Some(Use::Drop(*j)))
        }
        ([], true, Some(t)) => block
            .insts
            .iter()
            .rposition(|inst| matches!(inst, Inst::DecRef { value } if *value == array))
            .map(|j| Some(Use::Fuse(j, t.clone()))),
        _ => None,
    }
}
