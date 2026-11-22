//! A value a loop computes only to hand on, computed where it is handed on.
//!
//! A loop step that moves past an index on every way back often computes the
//! next index before it branches, as a merge does:
//!
//! ```text
//! b5(v1, v2, v3):
//!   v9 = add v3, 1
//!   ...
//!   branch v12, b6(), b7()
//! b6():
//!   set v4[v3] = ...
//!   jump b5(v13, v2, v9)
//! b7():
//!   set v4[v3] = ...
//!   jump b5(v1, v14, v9)
//! ```
//!
//! `v9` is read by both ways back, so it can't share `v3`'s frame slot, and
//! each way back copies it there: a load and a store an element on the
//! copy-and-patch backend. This pass writes `v9`'s addition out again in
//! each block that hands it on, right before the jump, and drops the first.
//! Each copy is read once, by its own jump, after the block's last read of
//! `v3`, so the backend gives it `v3`'s slot and the copy is gone.
//!
//! A block reached only from another that hands the value on, such as the
//! way a test's other side falls through to, reads that block's copy instead
//! of computing its own: the copy may already sit in `v3`'s slot by then, so
//! computing it again from `v3` would read the wrong value.
//!
//! It moves only an integer operation that can't abort, whose every read is
//! an argument of a jump out of another block. Each of those blocks must be
//! reached only from the first, or only from a block reached only from it,
//! so it runs at most once for each time the first did, and the copies never
//! run in a loop the first is outside of. LLVM gets the same code, and does
//! the same work on each path through it.

use crate::compiler::middle::ir::{BinOp, BlockId, Code, Inst, Term, Type, ValueId};
use crate::diagnostics::Invariant as _;

/// Writes out again, at each jump that hands it on, every value of `code`'s
/// loops that only jumps hand on ([the module docs](self)).
pub fn sink(code: &mut Code) {
    // Only a way back into a block with parameters makes a candidate's copy
    // worth anything, and most functions have none.
    let loops = code
        .iter()
        .any(|(p, b)| b.term.targets().any(|t| t.block <= p && !t.args.is_empty()));
    if !loops {
        return;
    }
    // Each value a way back hands on: only those cost a copy a step. Each
    // pass below reads less than the one before, and most functions stop
    // before the last, which reads every operand.
    let mut back: Vec<ValueId> = Vec::new();
    for (b, block) in code.iter() {
        for t in block.term.targets().filter(|t| t.block <= b) {
            back.extend(&t.args);
        }
    }
    back.sort_unstable();
    back.dedup();
    // Where each of those is defined, by an operation that may move.
    let mut def: Vec<(ValueId, BlockId)> = Vec::new();
    for (b, block) in code.iter() {
        for inst in &block.insts {
            if let Inst::Binary { dest, op, prim, .. } = inst {
                let moves = !matches!(op, BinOp::Div | BinOp::Rem)
                    && !op.is_comparison()
                    && Type::of_prim(*prim) == Some(Type::I64);
                if moves && back.binary_search(dest).is_ok() {
                    def.push((*dest, b));
                }
            }
        }
    }
    if def.is_empty() {
        return;
    }
    def.sort_unstable();
    let def_of = |v: ValueId| {
        def.binary_search_by_key(&v, |d| d.0)
            .ok()
            .and_then(|i| def.get(i))
            .map(|d| d.1)
    };
    // Every jump that hands one on, with its block and where the value is
    // defined. A value its own block hands on stays there.
    let mut readers: Vec<(ValueId, BlockId, BlockId)> = Vec::new();
    let mut kept: Vec<ValueId> = Vec::new();
    for (b, block) in code.iter() {
        for t in block.term.targets() {
            for a in &t.args {
                match def_of(*a) {
                    Some(d) if d == b => kept.push(*a),
                    Some(d) => readers.push((*a, b, d)),
                    None => {}
                }
            }
        }
    }
    kept.sort_unstable();
    readers.retain(|(v, _, _)| kept.binary_search(v).is_err());
    if readers.is_empty() {
        return;
    }
    // A value anything but a jump reads stays too.
    let mut ops = Vec::new();
    for (_, block) in code.iter() {
        for inst in &block.insts {
            ops.clear();
            inst.operands(&mut ops);
            kept.extend(ops.iter().filter(|o| def_of(**o).is_some()));
        }
        ops.clear();
        block.term.operands(&mut ops);
        kept.extend(ops.iter().filter(|o| def_of(**o).is_some()));
    }
    kept.sort_unstable();
    kept.dedup();
    readers.retain(|(v, _, _)| kept.binary_search(v).is_err());
    readers.sort_unstable();
    readers.dedup();
    // Only the entry has no predecessors, and no block asked about here is
    // the entry: each reads a value another block defines, or is the only
    // way into one that does.
    let preds = code.preds();
    let only_from = |b: BlockId, from: BlockId| preds.get(b).iter().all(|p| *p == from);
    let near = |b: BlockId, d: BlockId| {
        only_from(b, d)
            || preds
                .get(b)
                .first()
                .is_some_and(|p| only_from(b, *p) && only_from(*p, d))
    };
    let mut moved: Vec<(BlockId, ValueId)> = Vec::new();
    for group in readers.chunk_by(|x, y| x.0 == y.0) {
        let &(v, _, d) = group.first().or_ice("`chunk_by` makes no empty group");
        if !group.iter().all(|(_, b, _)| near(*b, d)) {
            continue;
        }
        // By what it defines: a copy put in `d` earlier moved the rest along.
        let defines = |i: &Inst| match i {
            Inst::Binary {
                dest,
                op,
                prim,
                lhs,
                rhs,
            } if *dest == v => Some((*op, *prim, *lhs, *rhs)),
            _ => None,
        };
        let op = code.get(d).insts.iter().find_map(defines);
        let (op, prim, lhs, rhs) = op.or_ice("`d` defines `v` until every group is done");
        // A block reached only from another reader takes that reader's copy,
        // which is computed there already, so that nothing past the reader
        // reads the operands the copy was computed from.
        let after = |b: BlockId| {
            let p = preds.get(b).first().copied()?;
            (!only_from(b, d) && group.iter().any(|(_, r, _)| *r == p)).then_some(p)
        };
        let mut copies: Vec<(BlockId, ValueId)> = Vec::new();
        for (_, b, _) in group.iter().filter(|(_, b, _)| after(*b).is_none()) {
            let copy = if copies.is_empty() {
                v
            } else {
                code.value(Type::I64)
            };
            let inst = Inst::Binary {
                dest: copy,
                op,
                prim,
                lhs,
                rhs,
            };
            let block = code.get_mut(*b);
            // Before the test the block branches on, if that is its last
            // instruction, so that the branch still follows it.
            let tested = match (&block.term, block.insts.last()) {
                (Term::Branch { cond, .. }, Some(last)) => last.results().contains(cond),
                _ => false,
            };
            let at = block.insts.len().saturating_sub(usize::from(tested));
            block.insts.insert(at, inst);
            block.term.rename(v, copy);
            copies.push((*b, copy));
        }
        for (_, b, _) in group {
            let copy = after(*b).and_then(|p| copies.iter().find(|(r, _)| *r == p));
            if let Some((_, copy)) = copy {
                code.get_mut(*b).term.rename(v, *copy);
            }
        }
        moved.push((d, v));
    }
    // By what each defines, not where: a copy put in a block that defines
    // another candidate moved that one along. No copy is in its own first's
    // block, so `v` is defined there once.
    for (d, v) in moved {
        code.get_mut(d)
            .insts
            .retain(|i| !matches!(i, Inst::Binary { dest, .. } if *dest == v));
    }
}
