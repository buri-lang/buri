//! The functions this backend generates for itself.
//!
//! Eight of them, and each is here for a reason the middle end cannot fix from
//! where it stands:
//!
//! | Helper | Why it is generated rather than called |
//! |---|---|
//! | [`Helper::Thunk`] | A closure's `code` takes its environment as a pointer; a lifted lambda takes it as leaves. Something has to convert — and it is also the one place the indirect-call ownership convention meets the callee's own (see [`thunk`]). |
//! | [`Helper::Concat`] | `str.concat` has no `buri_rt_*` entry, and the sequence is a uniqueness test, a copy, and an allocation on the path that needs one (MEMORY.md §5.3). |
//! | [`Helper::ShowInt`] | `derivePrimShow`'s integer arm (`middle/derives.rs`). |
//! | [`Helper::ShowBool`] | The same, for `Bool`. |
//! | [`Helper::Release`] | The per-type drop glue `Inst::DecRef` leaves `None` for the backend to fill in, because it is generated per layout (`middle/lower.rs`). |
//! | [`Helper::ReleaseElems`] | The same for a `[T]` block, whose element count is `cap / stride`. |
//! | [`Helper::RetainElem`] | The mirror, for one element, handed to `cli/runtime/list.rs` as a function pointer so a copied `[Str]` takes its counts. |
//! | [`Helper::EnvGlue`] | The one indirection that lets a closure environment carry its own drop glue (`emit.rs`'s header). |
//!
//! Every one is `Linkage::Local`, so a unit that needs `str.concat` has its
//! own and no two units collide.

use std::rc::Rc;

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{types, AbiParam, InstBuilder, Signature, Value};
use cranelift_frontend::FunctionBuilder;
use cranelift_module::Module;

use crate::compiler::backend::cranelift::abi::{Leaf, PTR};
use crate::compiler::backend::cranelift::emit::{
    mem_flags, word, Cx, Helper, Pending, Unit, ENV_FIELDS,
};
use crate::compiler::middle::ir::Ownership;
use crate::compiler::middle::layout::{
    GROWTH_FLOOR, HEADER_CAP_OFFSET, HEADER_RC_OFFSET, STR_ASCII_FLAG, STR_BASE, STR_LEN,
    STR_LEN_MASK, STR_PTR,
};
use crate::compiler::semantics::types::Ty;

/// Where an integer's digits are built, backwards. Twenty for `i64::MIN`'s
/// magnitude, one for the sign, and three spare.
const DIGITS: u32 = 24;

pub fn define(unit: &mut Unit<'_>, job: Pending) {
    let Some(sig) = unit.helper_signature(&job.key) else { return };
    let what = format!("{:?}", job.key);
    let key = job.key.clone();
    let ty = job.ty.clone();
    unit.build_function(job.id, sig, &what, move |unit, builder| {
        let cx = Cx::new(unit, builder);
        match key {
            Helper::Thunk { func, env } => thunk(cx, func, env),
            Helper::Concat => concat(cx),
            Helper::ShowInt { signed } => show_int(cx, signed),
            Helper::ShowBool => show_bool(cx),
            Helper::EnvGlue => env_glue(cx),
            Helper::Release { .. } => release(cx, ty),
            Helper::ReleaseElems { .. } => release_elems(cx, ty),
            Helper::RetainElem { .. } => retain_elem(cx, ty),
        }
    });
}

/// The entry block, with the function's parameters on it.
///
/// Not sealed here: `Unit::build_function` seals every block at the end, which
/// is the only point at which every branch to one exists.
fn entry(builder: &mut FunctionBuilder<'_>) -> Vec<Value> {
    let block = builder.create_block();
    builder.append_block_params_for_function_params(block);
    builder.switch_to_block(block);
    builder.block_params(block).to_vec()
}

/// Answer a `Str` through the out-pointer, which is the last parameter of
/// every helper that produces one (`abi.rs`'s header).
///
/// The three words go to the offsets the layout names rather than to 0, 8 and
/// 16, so this is the same statement `Lower::gather` makes about a returned
/// aggregate and not a second opinion about where a `Str`'s fields are.
fn return_str(cx: &mut Cx<'_, '_, '_>, out: Option<Value>, base: Value, ptr: Value, len: Value) {
    if let Some(out) = out {
        cx.store_at(out, word(STR_BASE), base);
        cx.store_at(out, word(STR_PTR), ptr);
        cx.store_at(out, word(STR_LEN), len);
    }
    cx.builder.ins().return_(&[]);
}

/// `code(env, args...)`, forwarding to the function the closure names.
///
/// With an environment, the record's leaves are loaded out of the block at
/// [`ENV_FIELDS`] and passed first, which is exactly the aggregate parameter
/// the lifted lambda declares. Without one, the environment is ignored: a
/// capture-free lambda is an ordinary `FnRef` by the time it reaches here
/// (`middle::closures`), and its callee has no environment parameter at all.
///
/// # This is where the two ownership conventions meet
///
/// `middle/rc.rs` states one of them as an assumption: *"a call through a
/// function value **owns** its arguments, because a code pointer cannot carry
/// a per-callee convention"*. The callee has the other one — `ir::Facts`'s
/// ownership column, inferred per parameter, where a `Str` a lambda only reads
/// is `Borrow` and is never released by the body.
///
/// A thunk is the only thing a code pointer ever points at, so it is the only
/// place the two can be reconciled, and reconciling them is two rules:
///
///  * **An argument the callee borrows is released here, after the call.** The
///    caller handed over a count and the callee will not consume it, so
///    without this every step of a `list.map` over a `[Str]` leaks one block
///    per element — which is exactly what it did.
///
///    "Borrows" is `ir::Facts`'s column *and* [`Cx::rc_counted`], because
///    `Own` has two meanings: a parameter rc promoted because the body
///    consumes it, and a parameter whose type rc could not see was counted at
///    all. Only the first releases anything, and only the second is a type the
///    caller never retained — so both are left alone here, and the question
///    that separates them is the one rc asked itself.
///  * **The environment record is retained where the callee *owns* it.** Those
///    bytes belong to the closure's block, which already holds a count of each
///    capture (`middle/rc.rs`'s loop tests), so handing them to a body that
///    will release them would free what the closure still points at.
///
/// Both are conditional on the type holding a count at all, so a thunk over
/// `fn(Int) => Int` is the same three instructions it always was.
fn thunk(mut cx: Cx<'_, '_, '_>, func: u32, env: bool) {
    let params = entry(cx.builder);
    let program = cx.unit.program;
    let Some(f) = program.funcs.get(func as usize) else {
        cx.builder.ins().trap(super::emit::UNREACHABLE);
        return;
    };
    let sig = f.sig.params.clone();
    let own = f.facts.params.clone();
    let mut args = Vec::new();
    // An argument the callee borrows: its type, where its leaves live, and
    // the values themselves, kept until after the call.
    let mut borrowed: Vec<(Ty, Rc<[Leaf]>, Vec<Value>)> = Vec::new();
    if env {
        let Some(env_ptr) = params.first().copied() else { return };
        let record = cx.offset(env_ptr, ENV_FIELDS);
        if let Some(first) = sig.first().copied() {
            if own.first() == Some(&Ownership::Own) {
                if let Some(ty) = cx.unit.abi.source_ty(program, first) {
                    if cx.rc_counted(&ty) {
                        cx.walk_rc(&ty, record, true, 0);
                    }
                }
            }
            for leaf in cx.unit.abi.leaves(program, first).iter() {
                let v = cx.load_at(leaf.ty, record, leaf.offset);
                args.push(v);
            }
        }
    }
    // The thunk's own parameters, after the environment pointer, are the
    // arguments flattened — and then, where the callee answers through one,
    // the out-pointer, which is forwarded unchanged because it is last in
    // both signatures (`abi.rs`'s header).
    let mut at = 1usize;
    for (j, p) in sig.iter().enumerate().skip(usize::from(env)) {
        let leaves = cx.unit.abi.leaves(program, *p);
        let taken: Vec<Value> =
            params.get(at..at.saturating_add(leaves.len())).unwrap_or_default().to_vec();
        at = at.saturating_add(leaves.len());
        args.extend(taken.iter().copied());
        if own.get(j) != Some(&Ownership::Borrow) {
            continue;
        }
        if let Some(ty) = cx.unit.abi.source_ty(program, *p) {
            if cx.rc_counted(&ty) {
                borrowed.push((ty, leaves, taken));
            }
        }
    }
    args.extend(params.get(at..).unwrap_or_default().iter().copied());
    let Some(r) = cx.func_ref(func as usize) else {
        cx.builder.ins().trap(super::emit::UNREACHABLE);
        return;
    };
    let inst = cx.builder.ins().call(r, &args);
    let results = cx.builder.inst_results(inst).to_vec();
    for (ty, leaves, vals) in borrowed {
        let l = cx.unit.abi.layouts.shared(&ty);
        let slot = cx.slot(l.size, l.align);
        for (leaf, v) in leaves.iter().zip(vals) {
            cx.store_at(slot, leaf.offset, v);
        }
        cx.walk_rc(&ty, slot, false, 0);
    }
    cx.builder.ins().return_(&results);
}

/// `str.concat` (VALUE-MODEL.md §3), with MEMORY.md §5.3's in-place growth.
///
/// The ASCII flag is the *conjunction* of the two inputs' flags, which is
/// correct and not merely conservative: a concatenation of two all-ASCII views
/// is all ASCII, and clearing the flag only ever costs an O(n) scalar count.
///
/// # The three paths, and what licenses the first
///
///  1. **In place.** The left operand's block is uniquely owned — `rc == 1` —
///     and has room for the right operand's bytes past the end of the view.
///     The bytes are written there, the block takes one more reference (the
///     result's), and the result is the *same* `base` and the *same* `ptr`
///     with a longer length. Nothing is allocated and the left operand's own
///     bytes are not copied.
///  2. **Grown.** Uniquely owned but out of room: a fresh block of
///     `max(n * 2, GROWTH_FLOOR)` bytes rather than exactly `n`, so the next
///     concatenation in a chain takes path 1. A template of *k* holes, or a
///     fold that concatenates, therefore allocates O(log n) times instead of
///     once per step.
///  3. **Exact.** Shared, immortal, or a literal (`base == 0`): exactly `n`
///     bytes, which is what this helper did unconditionally before. A shared
///     string must not grow speculatively — it is not the one being built.
///
/// **Why path 1 is unobservable.** `rc == 1` means exactly one live `Str`
/// value refers to this block: every operation that produces a *new* view of a
/// block increfs its base before answering (`cli/runtime/text.rs`'s header), so
/// a second view would be a second count. Static elision never duplicates a
/// reference without an `incref` — a borrowed argument aliases the caller's
/// own reference rather than adding one — so the aliases it leaves behind are
/// copies of that one value, carrying the same `ptr` and the same `len`. The
/// write here starts at `ptr + len` and is therefore invisible to every one of
/// them. That is the whole argument, and it rests on the reference counting
/// being correct rather than on anything about this helper.
///
/// The copy is a `memmove` rather than a `memcpy` on that path: the right
/// operand is *nearly* always a different block, and where it is a second view
/// into this one the two ranges can touch. A `memmove` costs nothing measurable
/// and removes the case from the argument entirely.
fn concat(mut cx: Cx<'_, '_, '_>) {
    let p = entry(cx.builder);
    let out = p.get(6).copied();
    let (Some(a_base), Some(a_ptr), Some(a_len), Some(b_ptr), Some(b_len)) = (
        p.first().copied(),
        p.get(1).copied(),
        p.get(2).copied(),
        p.get(4).copied(),
        p.get(5).copied(),
    ) else {
        return;
    };
    let mask = cx.iconst(types::I64, STR_LEN_MASK as i64);
    let la = cx.builder.ins().band(a_len, mask);
    let lb = cx.builder.ins().band(b_len, mask);
    let n = cx.builder.ins().iadd(la, lb);

    let empty = cx.builder.create_block();
    let work = cx.builder.create_block();
    let is_empty = cx.builder.ins().icmp_imm(IntCC::Equal, n, 0);
    cx.brif(is_empty, empty, &[], work, &[]);

    cx.builder.switch_to_block(empty);
    let zero = cx.iconst(PTR, 0);
    let flag = cx.iconst(types::I64, STR_ASCII_FLAG as i64);
    let blank = empty_str(&mut cx);
    return_str(&mut cx, out, zero, blank, flag);

    // `probe` reads the header, which is only there when there is a base;
    // `check` is reached from both sides with the answer as a block parameter,
    // so the header load stays behind the null test.
    let probe = cx.builder.create_block();
    let check = cx.builder.create_block();
    cx.builder.append_block_param(check, types::I64);
    cx.builder.append_block_param(check, types::I64);
    let inplace = cx.builder.create_block();
    let fresh = cx.builder.create_block();
    cx.builder.append_block_param(fresh, types::I64);
    let ret = cx.builder.create_block();
    cx.builder.append_block_param(ret, PTR);
    cx.builder.append_block_param(ret, PTR);

    cx.builder.switch_to_block(work);
    let none = cx.iconst(types::I64, 0);
    let has_base = cx.builder.ins().icmp_imm(IntCC::NotEqual, a_base, 0);
    cx.brif(has_base, probe, &[], check, &[none, none]);

    cx.builder.switch_to_block(probe);
    let rc = cx.builder.ins().load(types::I64, mem_flags(), a_base, HEADER_RC_OFFSET);
    let cap = cx.builder.ins().load(types::I64, mem_flags(), a_base, HEADER_CAP_OFFSET);
    // `IMMORTAL` is `u64::MAX` and fails this by construction, which is what
    // keeps a literal and an interned constant out of both fast paths.
    let is_one = cx.builder.ins().icmp_imm(IntCC::Equal, rc, 1);
    let unique = cx.builder.ins().uextend(types::I64, is_one);
    cx.jump(check, &[unique, cap]);

    cx.builder.switch_to_block(check);
    let cp = cx.builder.block_params(check).to_vec();
    let (Some(unique), Some(cap)) = (cp.first().copied(), cp.get(1).copied()) else {
        return;
    };
    // The view may start inside the block, so what has to fit is the offset of
    // its start plus the whole result. With no base the offset is nonsense and
    // the capacity is zero, so this is false — and `unique` is zero anyway.
    let offset = cx.builder.ins().isub(a_ptr, a_base);
    let end = cx.builder.ins().iadd(offset, n);
    let fits = cx.builder.ins().icmp(IntCC::UnsignedLessThanOrEqual, end, cap);
    let fits = cx.builder.ins().uextend(types::I64, fits);
    let take = cx.builder.ins().band(unique, fits);
    cx.brif(take, inplace, &[], fresh, &[unique]);

    let cfg = cx.unit.module.isa().frontend_config();

    cx.builder.switch_to_block(inplace);
    let tail = cx.builder.ins().iadd(a_ptr, la);
    cx.builder.call_memmove(cfg, tail, b_ptr, lb);
    cx.incref(a_base);
    cx.jump(ret, &[a_base, a_ptr]);

    cx.builder.switch_to_block(fresh);
    let grow = cx.builder.block_params(fresh).first().copied().unwrap_or(none);
    let doubled = cx.builder.ins().imul_imm(n, 2);
    let floor = cx.iconst(types::I64, GROWTH_FLOOR as i64);
    let bigger = cx.builder.ins().icmp(IntCC::UnsignedGreaterThan, doubled, floor);
    let wanted = cx.builder.ins().select(bigger, doubled, floor);
    let growing = cx.builder.ins().icmp_imm(IntCC::NotEqual, grow, 0);
    let size = cx.builder.ins().select(growing, wanted, n);
    let block = cx.alloc(size);
    cx.builder.call_memcpy(cfg, block, a_ptr, la);
    let tail = cx.builder.ins().iadd(block, la);
    cx.builder.call_memcpy(cfg, tail, b_ptr, lb);
    cx.jump(ret, &[block, block]);

    cx.builder.switch_to_block(ret);
    let rp = cx.builder.block_params(ret).to_vec();
    let (Some(base), Some(ptr)) = (rp.first().copied(), rp.get(1).copied()) else {
        return;
    };
    let both = cx.builder.ins().band(a_len, b_len);
    let ascii = cx.builder.ins().band_imm(both, STR_ASCII_FLAG as i64);
    let len = cx.builder.ins().bor(n, ascii);
    return_str(&mut cx, out, base, ptr, len);
}

/// The address of a byte that is not a string, for the empty `Str`.
///
/// It has to be non-null: `ptr` is the field the `Option<Str>` niche spends
/// (VALUE-MODEL.md §6), so an empty string with a null `ptr` would read as
/// `.None`.
fn empty_str(cx: &mut Cx<'_, '_, '_>) -> Value {
    match cx.unit.bytes("") {
        Some(data) => {
            let gv = cx.unit.module.declare_data_in_func(data, cx.builder.func);
            cx.builder.ins().symbol_value(PTR, gv)
        }
        None => cx.iconst(PTR, 1),
    }
}

/// An integer in decimal, as a fresh `Str`.
///
/// `derivePrimShow`'s integer arm. The digits are written backwards into a
/// stack buffer and the used tail is copied into one allocation, which is the
/// shape every decimal renderer has and needs no division by a constant the
/// backend cannot fold.
///
/// The magnitude of `i64::MIN` does not fit in an `i64`, and it does not have
/// to: the negation is taken in two's complement and read back as unsigned,
/// which is exactly `9223372036854775808`.
fn show_int(mut cx: Cx<'_, '_, '_>, signed: bool) {
    let p = entry(cx.builder);
    let out = p.get(1).copied();
    let Some(v) = p.first().copied() else { return };
    let buf = cx.slot(DIGITS, 1);

    let negative = if signed {
        cx.builder.ins().icmp_imm(IntCC::SignedLessThan, v, 0)
    } else {
        cx.iconst(types::I8, 0)
    };
    let flipped = cx.builder.ins().ineg(v);
    let magnitude = cx.builder.ins().select(negative, flipped, v);

    let header = cx.builder.create_block();
    cx.builder.append_block_param(header, types::I64);
    cx.builder.append_block_param(header, types::I64);
    let after = cx.builder.create_block();
    cx.builder.append_block_param(after, types::I64);
    let sign = cx.builder.create_block();
    cx.builder.append_block_param(sign, types::I64);
    let fin = cx.builder.create_block();
    cx.builder.append_block_param(fin, types::I64);

    let start = cx.iconst(types::I64, i64::from(DIGITS));
    cx.jump(header, &[start, magnitude]);

    cx.builder.switch_to_block(header);
    let hp = cx.builder.block_params(header).to_vec();
    let (Some(i), Some(u)) = (hp.first().copied(), hp.get(1).copied()) else { return };
    let ten = cx.iconst(types::I64, 10);
    let digit = cx.builder.ins().urem(u, ten);
    let rest = cx.builder.ins().udiv(u, ten);
    let at = cx.builder.ins().iadd_imm(i, -1);
    let ch = cx.builder.ins().iadd_imm(digit, 48);
    let byte = cx.builder.ins().ireduce(types::I8, ch);
    let addr = cx.builder.ins().iadd(buf, at);
    cx.builder.ins().store(mem_flags(), byte, addr, 0);
    let done = cx.builder.ins().icmp_imm(IntCC::Equal, rest, 0);
    let then = [at];
    let els = [at, rest];
    cx.brif(done, after, &then, header, &els);

    cx.builder.switch_to_block(after);
    let ap = cx.builder.block_params(after).first().copied().unwrap_or(start);
    let one = [ap];
    cx.brif(negative, sign, &one, fin, &one);

    cx.builder.switch_to_block(sign);
    let sp = cx.builder.block_params(sign).first().copied().unwrap_or(start);
    let minus_at = cx.builder.ins().iadd_imm(sp, -1);
    let minus = cx.iconst(types::I8, 45);
    let maddr = cx.builder.ins().iadd(buf, minus_at);
    cx.builder.ins().store(mem_flags(), minus, maddr, 0);
    let arg = [minus_at];
    cx.jump(fin, &arg);

    cx.builder.switch_to_block(fin);
    let fp = cx.builder.block_params(fin).first().copied().unwrap_or(start);
    let total = cx.iconst(types::I64, i64::from(DIGITS));
    let n = cx.builder.ins().isub(total, fp);
    let block = cx.alloc(n);
    let src = cx.builder.ins().iadd(buf, fp);
    let cfg = cx.unit.module.isa().frontend_config();
    cx.builder.call_memcpy(cfg, block, src, n);
    // Decimal digits and a minus sign are all below 0x80, so the rendering is
    // ASCII by construction and `str.len()` on it is a mask
    // (VALUE-MODEL.md §3.1).
    let len = cx.builder.ins().bor_imm(n, STR_ASCII_FLAG as i64);
    return_str(&mut cx, out, block, block, len);
}

/// `true` and `false`, as literals: two statics and a branch.
fn show_bool(mut cx: Cx<'_, '_, '_>) {
    let p = entry(cx.builder);
    let out = p.get(1).copied();
    let Some(v) = p.first().copied() else { return };
    let yes = cx.builder.create_block();
    let no = cx.builder.create_block();
    cx.brif(v, yes, &[], no, &[]);
    for (block, text) in [(yes, "true"), (no, "false")] {
        cx.builder.switch_to_block(block);
        let Some(data) = cx.unit.bytes(text) else { continue };
        let gv = cx.unit.module.declare_data_in_func(data, cx.builder.func);
        let ptr = cx.builder.ins().symbol_value(PTR, gv);
        let zero = cx.iconst(PTR, 0);
        let len = cx.iconst(types::I64, (text.len() as u64 | STR_ASCII_FLAG) as i64);
        return_str(&mut cx, out, zero, ptr, len);
    }
}

/// The drop glue every closure environment shares: the block's first word is
/// the type-specific release function, and the record follows it.
fn env_glue(mut cx: Cx<'_, '_, '_>) {
    let p = entry(cx.builder);
    let Some(block) = p.first().copied() else { return };
    let f = cx.load_at(PTR, block, 0);
    let live = cx.builder.create_block();
    let done = cx.builder.create_block();
    let none = cx.builder.ins().icmp_imm(IntCC::Equal, f, 0);
    cx.brif(none, done, &[], live, &[]);
    cx.builder.switch_to_block(live);
    let mut sig = Signature::new(cx.unit.abi.call_conv);
    sig.params.push(AbiParam::new(PTR));
    let sr = cx.builder.import_signature(sig);
    let record = cx.offset(block, ENV_FIELDS);
    cx.builder.ins().call_indirect(sr, f, &[record]);
    cx.jump(done, &[]);
    cx.builder.switch_to_block(done);
    cx.builder.ins().return_(&[]);
}

/// Release the contents of one value of a type: the per-type drop glue.
fn release(mut cx: Cx<'_, '_, '_>, ty: Option<Ty>) {
    let p = entry(cx.builder);
    let Some(addr) = p.first().copied() else { return };
    if let Some(ty) = ty {
        cx.walk_rc(&ty, addr, false, 0);
    }
    cx.builder.ins().return_(&[]);
}

/// Release every element of a `[T]` block.
///
/// The count is `cap / stride`, and `cap` is the second header word
/// (VALUE-MODEL.md §2) — which is what makes a drop glue taking only a pointer
/// enough for a list.
fn release_elems(mut cx: Cx<'_, '_, '_>, ty: Option<Ty>) {
    let p = entry(cx.builder);
    let Some(addr) = p.first().copied() else { return };
    if let Some(elem) = ty {
        let stride = cx.unit.abi.layouts.shared(&elem).stride.max(1);
        let cap = cx.builder.ins().load(types::I64, mem_flags(), addr, HEADER_CAP_OFFSET);
        let count = cx.builder.ins().udiv_imm(cap, i64::from(stride));
        cx.each_element(addr, count, stride, &elem, false);
    }
    cx.builder.ins().return_(&[]);
}

/// One element of a `[T]`, retained in place.
///
/// The whole body is the counted-pointer walk with `retain = true`, which is
/// the same walk `release` runs with `retain = false`. Writing it as a second
/// helper rather than as a flag on `Release` keeps the two symbols distinct in
/// the object, which is what lets `cli/runtime/list.rs` take one as a plain
/// function pointer.
fn retain_elem(mut cx: Cx<'_, '_, '_>, ty: Option<Ty>) {
    let p = entry(cx.builder);
    if let (Some(addr), Some(elem)) = (p.first().copied(), ty) {
        cx.walk_rc(&elem, addr, true, 0);
    }
    cx.builder.ins().return_(&[]);
}

#[cfg(test)]
mod tests {
    use crate::compiler::backend::cranelift::emit::word;
    use crate::compiler::middle::layout::STR_PTR;

    /// `concat` takes two `Str`s flattened — `(base, ptr, len)` twice — and
    /// copies from the second word of each. That the second word is `ptr` is
    /// the layout's statement and not this file's, so it is checked here
    /// rather than asserted in a comment.
    #[test]
    fn the_concatenation_reads_the_word_the_layout_names() {
        assert_eq!(word(STR_PTR), 8);
    }
}
