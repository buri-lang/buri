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
//!
//! [`offsets`] then replaces a loop's back-edge `slice.owned` with an offset.

use crate::compiler::middle::ir::{
    BinOp, Block, BlockId, Code, Const, Inst, ListTy, PerBlock, Target, Term, Type, ValueId,
};
use crate::compiler::semantics::typed::Magnitude;
use crate::compiler::semantics::types::Prim;

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

/// Carries every list a loop drops the head of with an offset, in place of
/// the owned slice [`sink`] leaves on the back edge.
///
/// An owned slice of a list nothing else holds still moves the whole tail
/// down, so a recursion over `[x, ..rest]` moves a list of `n` elements `n`
/// times. Here the loop header takes the same list and an `I64` offset
/// instead:
///
/// ```text
/// b1(v5: [Entry], v69: i64):          b8():
///   ...                                 v73 = add.I64 v14, v69
/// b3():                                 v15 = elem v5, v73
///   v71 = len v5                        ...
///   v8 = sub.I64 v71, v69             b9():
///   ...                                 v74 = add.I64 v69, v16
///                                       jump b1(v5, v74)
/// ```
///
/// A read of the list in the loop adds the offset to its index and takes it
/// off its length. The elements before the offset are released with the list
/// rather than at each back edge, which changes no answer.
///
/// A block that reads the list any other way, such as a call or a `return`,
/// first makes it whole with one owned slice from the offset, and reads that
/// instead. Only a block on the way out of the loop may, and no block after it
/// may name the list, so the list is made whole at most once on any path.
pub fn offsets(code: &mut Code) {
    let mut lists = Vec::new();
    for (_, block) in code.iter() {
        for inst in &block.insts {
            if let Inst::ArraySlice {
                array,
                ty,
                owned: true,
                ..
            } = *inst
            {
                if !lists.iter().any(|&(a, _)| a == array) {
                    lists.push((array, ty));
                }
            }
        }
    }
    for (list, ty) in lists {
        // The entry's parameters are the function's, which nothing jumps to.
        let header = code.iter().skip(1).find_map(|(b, block)| {
            let idx = block.params.iter().position(|&p| p == list)?;
            Some((b, idx))
        });
        if let Some((header, idx)) = header {
            if let Some(whole) = plan(code, (header, idx, list)) {
                rewrite(code, (header, idx, list), ty, &whole);
            }
        }
    }
}

/// Where each block makes parameter `idx` of `header` whole, if it does, as
/// an instruction index; or `None` where [`offsets`] must leave it alone.
fn plan(
    code: &Code,
    (header, idx, list): (BlockId, usize, ValueId),
) -> Option<PerBlock<Option<usize>>> {
    // The blocks that can reach the header: the loop and its way in.
    let preds = code.preds();
    let mut looping = code.per_block(false);
    let mut stack = vec![header];
    while let Some(b) = stack.pop() {
        if !std::mem::replace(looping.get_mut(b), true) {
            stack.extend(preds.get(b).iter().copied());
        }
    }
    let mut whole = code.per_block(None);
    let mut ops = Vec::new();
    for (b, block) in code.iter() {
        let mut at = block.insts.iter().position(|inst| {
            ops.clear();
            inst.operands(&mut ops);
            ops.contains(&list) && !reads_at_offset(code, inst, header, idx)
        });
        if at.is_none() {
            ops.clear();
            block.term.operands(&mut ops);
            for t in block.term.targets() {
                for (k, &a) in t.args.iter().enumerate() {
                    if t.block != header || k != idx {
                        ops.push(a);
                    }
                }
            }
            if ops.contains(&list) {
                at = Some(block.insts.len());
            }
        }
        if at.is_some() {
            if *looping.get(b) {
                return None;
            }
            // No block after this one names the list.
            let mut seen = code.per_block(false);
            let mut stack: Vec<BlockId> = block.term.targets().map(|t| t.block).collect();
            while let Some(s) = stack.pop() {
                if !std::mem::replace(seen.get_mut(s), true) {
                    if names(code.get(s), list, &mut ops) {
                        return None;
                    }
                    stack.extend(code.get(s).term.targets().map(|t| t.block));
                }
            }
        }
        *whole.get_mut(b) = at;
    }
    Some(whole)
}

/// Whether `inst`, which reads parameter `idx` of `header`, can read it from
/// an offset: a length, an element, a slice, a `decref`, or a back edge's
/// owned slice.
fn reads_at_offset(code: &Code, inst: &Inst, header: BlockId, idx: usize) -> bool {
    match *inst {
        Inst::ArraySlice {
            dest, owned: true, ..
        } => back_edge(code, dest, header, idx),
        Inst::ArrayLen { .. }
        | Inst::ArrayGet { .. }
        | Inst::ArraySlice { .. }
        | Inst::DecRef { .. } => true,
        _ => false,
    }
}

/// Whether the one read of `v` in `code` hands it to `header` as parameter
/// `idx`.
fn back_edge(code: &Code, v: ValueId, header: BlockId, idx: usize) -> bool {
    let mut ops = Vec::new();
    let mut edge = false;
    for (_, block) in code.iter() {
        for inst in &block.insts {
            inst.operands(&mut ops);
        }
        block.term.operands(&mut ops);
        for t in block.term.targets() {
            ops.extend(&t.args);
            edge |= t.block == header && t.args.get(idx) == Some(&v);
        }
    }
    edge && ops.iter().filter(|&&o| o == v).count() == 1
}

/// Whether `block` reads `v` anywhere.
fn names(block: &Block, v: ValueId, ops: &mut Vec<ValueId>) -> bool {
    ops.clear();
    for inst in &block.insts {
        inst.operands(ops);
    }
    block.term.operands(ops);
    ops.extend(block.term.targets().flat_map(|t| t.args.iter().copied()));
    ops.contains(&v)
}

/// [`offsets`]' rewrite of parameter `idx` of `header`, by [`plan`]'s
/// `whole`.
fn rewrite(
    code: &mut Code,
    (header, idx, list): (BlockId, usize, ValueId),
    ty: ListTy,
    whole: &PerBlock<Option<usize>>,
) {
    let offset = code.value(Type::I64);
    code.get_mut(header).params.push(offset);
    let ids: Vec<BlockId> = code.iter().map(|(b, _)| b).collect();
    for b in ids {
        let at = *whole.get(b);
        let insts = std::mem::take(&mut code.get_mut(b).insts);
        let mut out = Vec::new();
        let mut made = None;
        // Each back edge's slice, and the offset that replaces it.
        let mut rests = Vec::new();
        for (j, mut inst) in insts.into_iter().enumerate() {
            if at == Some(j) {
                made = Some(make_whole(code, &mut out, list, ty, offset));
            }
            if let Some(w) = made {
                inst.rename(list, w);
                out.push(inst);
                continue;
            }
            match inst {
                Inst::ArrayLen { dest, array } if array == list => {
                    let len = code.value(Type::I64);
                    out.push(Inst::ArrayLen { dest: len, array });
                    out.push(binary(BinOp::Sub, dest, len, offset));
                }
                Inst::ArrayGet {
                    dest,
                    array,
                    ty,
                    index,
                } if array == list => {
                    let at = code.value(Type::I64);
                    out.push(binary(BinOp::Add, at, index, offset));
                    out.push(Inst::ArrayGet {
                        dest,
                        array,
                        ty,
                        index: at,
                    });
                }
                Inst::ArraySlice {
                    dest,
                    array,
                    ty,
                    from,
                    owned,
                } if array == list => {
                    let at = code.value(Type::I64);
                    out.push(binary(BinOp::Add, at, from, offset));
                    if owned {
                        rests.push((dest, at));
                    } else {
                        out.push(Inst::ArraySlice {
                            dest,
                            array,
                            ty,
                            from: at,
                            owned,
                        });
                    }
                }
                inst => out.push(inst),
            }
        }
        if at.is_some() && made.is_none() {
            made = Some(make_whole(code, &mut out, list, ty, offset));
        }
        let mut zero = None;
        let mut term = std::mem::replace(&mut code.get_mut(b).term, Term::Unreachable);
        for t in term.targets_mut().filter(|t| t.block == header) {
            let mut next = offset;
            for arg in t.args.iter_mut().skip(idx).take(1) {
                if let Some(&(_, at)) = rests.iter().find(|&&(s, _)| s == *arg) {
                    *arg = list;
                    next = at;
                } else if *arg != list {
                    // Another list, from its first element.
                    next = *zero.get_or_insert_with(|| {
                        let z = code.value(Type::I64);
                        out.push(Inst::Const {
                            dest: z,
                            value: Const::Int {
                                bits: Magnitude::new(0),
                                negative: false,
                            },
                        });
                        z
                    });
                }
            }
            t.args.push(next);
        }
        if let Some(w) = made {
            term.rename(list, w);
        }
        let block = code.get_mut(b);
        block.insts = out;
        block.term = term;
    }
}

/// `list` from `offset` on, as a list of its own that takes `list`'s count.
fn make_whole(
    code: &mut Code,
    out: &mut Vec<Inst>,
    list: ValueId,
    ty: ListTy,
    offset: ValueId,
) -> ValueId {
    let whole = code.value(code.ty_of(list));
    out.push(Inst::ArraySlice {
        dest: whole,
        array: list,
        ty,
        from: offset,
        owned: true,
    });
    whole
}

fn binary(op: BinOp, dest: ValueId, lhs: ValueId, rhs: ValueId) -> Inst {
    Inst::Binary {
        dest,
        op,
        prim: Prim::I64,
        lhs,
        rhs,
    }
}
