//! The functions this backend generates for itself.
//!
//! Eight of them, and each is here for a reason the middle end cannot fix from
//! where it stands:
//!
//! | Helper | Why it is generated rather than called |
//! |---|---|
//! | [`Helper::Thunk`] | A closure's `code` takes its environment as a pointer; a lifted lambda takes it as leaves. Something has to convert. |
//! | [`Helper::Concat`] | `str.concat` has no `buri_rt_*` entry, and the sequence is a uniqueness test, a copy, and an allocation on the path that needs one (MEMORY.md §5.3). |
//! | [`Helper::ShowInt`] | `derivePrimShow`'s integer arm (`middle/derives.rs`). |
//! | [`Helper::ShowBool`] | The same, for `Bool`. |
//! | [`Helper::Release`] | The per-type drop glue `Inst::DecRef` leaves `None` for wave 2 to fill (`middle/lower.rs`). |
//! | [`Helper::ReleaseElems`] | The same for a `[T]` block, whose element count is `cap / stride`. |
//! | [`Helper::RetainElem`] | The mirror, for one element, handed to `cli/runtime/list.rs` as a function pointer so a copied `[Str]` takes its counts. |
//! | [`Helper::EnvGlue`] | The one indirection that lets a closure environment carry its own drop glue (`emit.rs`'s header). |
//!
//! Every one is `Linkage::Local`, so a unit that needs `str.concat` has its
//! own and no two units collide.

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{types, AbiParam, InstBuilder, Signature, Value};
use cranelift_frontend::FunctionBuilder;
use cranelift_module::Module;

use crate::compiler::backend::cranelift::abi::PTR;
use crate::compiler::backend::cranelift::emit::{
    mem, Cx, Helper, Pending, Unit, ENV_FIELDS,
};
use crate::compiler::middle::layout::{
    GROWTH_FLOOR, HEADER_CAP_OFFSET, HEADER_RC_OFFSET, STR_ASCII_FLAG, STR_LEN_MASK,
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
    unit.build_function(job.id, sig, &what, move |unit, b| {
        let cx = Cx { unit, b };
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
fn entry(b: &mut FunctionBuilder<'_>) -> Vec<Value> {
    let block = b.create_block();
    b.append_block_params_for_function_params(block);
    b.switch_to_block(block);
    b.block_params(block).to_vec()
}

/// `code(env, args...)`, forwarding to the function the closure names.
///
/// With an environment, the record's leaves are loaded out of the block at
/// [`ENV_FIELDS`] and passed first, which is exactly the aggregate parameter
/// the lifted lambda declares. Without one, the environment is ignored: a
/// capture-free lambda is an ordinary `FnRef` by the time it reaches here
/// (`middle::closures`), and its callee has no environment parameter at all.
fn thunk(mut cx: Cx<'_, '_, '_>, func: u32, env: bool) {
    let params = entry(cx.b);
    let Some(f) = cx.unit.program.funcs.get(func as usize) else {
        cx.b.ins().trap(super::emit::UNREACHABLE);
        return;
    };
    let sig = f.sig.params.clone();
    let mut args = Vec::new();
    if env {
        let Some(env_ptr) = params.first().copied() else { return };
        let record = cx.offset(env_ptr, ENV_FIELDS);
        if let Some(first) = sig.first().copied() {
            let program = cx.unit.program;
            for leaf in cx.unit.abi.leaves(program, first) {
                let v = cx.load_at(leaf.ty, record, leaf.offset);
                args.push(v);
            }
        }
    }
    args.extend(params.iter().skip(1).copied());
    let Some(r) = cx.func_ref(func as usize) else {
        cx.b.ins().trap(super::emit::UNREACHABLE);
        return;
    };
    let inst = cx.b.ins().call(r, &args);
    let results = cx.b.inst_results(inst).to_vec();
    cx.b.ins().return_(&results);
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
    let p = entry(cx.b);
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
    let la = cx.b.ins().band(a_len, mask);
    let lb = cx.b.ins().band(b_len, mask);
    let n = cx.b.ins().iadd(la, lb);

    let empty = cx.b.create_block();
    let work = cx.b.create_block();
    let is_empty = cx.b.ins().icmp_imm(IntCC::Equal, n, 0);
    cx.brif(is_empty, empty, &[], work, &[]);

    cx.b.switch_to_block(empty);
    let zero = cx.iconst(PTR, 0);
    let flag = cx.iconst(types::I64, STR_ASCII_FLAG as i64);
    let blank = empty_str(&mut cx);
    cx.b.ins().return_(&[zero, blank, flag]);

    // `probe` reads the header, which is only there when there is a base;
    // `check` is reached from both sides with the answer as a block parameter,
    // so the header load stays behind the null test.
    let probe = cx.b.create_block();
    let check = cx.b.create_block();
    cx.b.append_block_param(check, types::I64);
    cx.b.append_block_param(check, types::I64);
    let inplace = cx.b.create_block();
    let fresh = cx.b.create_block();
    cx.b.append_block_param(fresh, types::I64);
    let ret = cx.b.create_block();
    cx.b.append_block_param(ret, PTR);
    cx.b.append_block_param(ret, PTR);

    cx.b.switch_to_block(work);
    let none = cx.iconst(types::I64, 0);
    let has_base = cx.b.ins().icmp_imm(IntCC::NotEqual, a_base, 0);
    cx.brif(has_base, probe, &[], check, &[none, none]);

    cx.b.switch_to_block(probe);
    let rc = cx.b.ins().load(types::I64, mem(), a_base, HEADER_RC_OFFSET);
    let cap = cx.b.ins().load(types::I64, mem(), a_base, HEADER_CAP_OFFSET);
    // `IMMORTAL` is `u64::MAX` and fails this by construction, which is what
    // keeps a literal and an interned constant out of both fast paths.
    let is_one = cx.b.ins().icmp_imm(IntCC::Equal, rc, 1);
    let unique = cx.b.ins().uextend(types::I64, is_one);
    cx.jump(check, &[unique, cap]);

    cx.b.switch_to_block(check);
    let cp = cx.b.block_params(check).to_vec();
    let (Some(unique), Some(cap)) = (cp.first().copied(), cp.get(1).copied()) else {
        return;
    };
    // The view may start inside the block, so what has to fit is the offset of
    // its start plus the whole result. With no base the offset is nonsense and
    // the capacity is zero, so this is false — and `unique` is zero anyway.
    let offset = cx.b.ins().isub(a_ptr, a_base);
    let end = cx.b.ins().iadd(offset, n);
    let fits = cx.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, end, cap);
    let fits = cx.b.ins().uextend(types::I64, fits);
    let take = cx.b.ins().band(unique, fits);
    cx.brif(take, inplace, &[], fresh, &[unique]);

    let cfg = cx.unit.module.isa().frontend_config();

    cx.b.switch_to_block(inplace);
    let tail = cx.b.ins().iadd(a_ptr, la);
    cx.b.call_memmove(cfg, tail, b_ptr, lb);
    cx.incref(a_base);
    cx.jump(ret, &[a_base, a_ptr]);

    cx.b.switch_to_block(fresh);
    let grow = cx.b.block_params(fresh).first().copied().unwrap_or(none);
    let doubled = cx.b.ins().imul_imm(n, 2);
    let floor = cx.iconst(types::I64, GROWTH_FLOOR as i64);
    let bigger = cx.b.ins().icmp(IntCC::UnsignedGreaterThan, doubled, floor);
    let wanted = cx.b.ins().select(bigger, doubled, floor);
    let growing = cx.b.ins().icmp_imm(IntCC::NotEqual, grow, 0);
    let size = cx.b.ins().select(growing, wanted, n);
    let block = cx.alloc(size);
    cx.b.call_memcpy(cfg, block, a_ptr, la);
    let tail = cx.b.ins().iadd(block, la);
    cx.b.call_memcpy(cfg, tail, b_ptr, lb);
    cx.jump(ret, &[block, block]);

    cx.b.switch_to_block(ret);
    let rp = cx.b.block_params(ret).to_vec();
    let (Some(base), Some(ptr)) = (rp.first().copied(), rp.get(1).copied()) else {
        return;
    };
    let both = cx.b.ins().band(a_len, b_len);
    let ascii = cx.b.ins().band_imm(both, STR_ASCII_FLAG as i64);
    let len = cx.b.ins().bor(n, ascii);
    cx.b.ins().return_(&[base, ptr, len]);
}

/// The address of a byte that is not a string, for the empty `Str`.
///
/// It has to be non-null: `ptr` is the field the `Option<Str>` niche spends
/// (VALUE-MODEL.md §6), so an empty string with a null `ptr` would read as
/// `.None`.
fn empty_str(cx: &mut Cx<'_, '_, '_>) -> Value {
    match cx.unit.bytes("") {
        Some(data) => {
            let gv = cx.unit.module.declare_data_in_func(data, cx.b.func);
            cx.b.ins().symbol_value(PTR, gv)
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
    let p = entry(cx.b);
    let Some(v) = p.first().copied() else { return };
    let buf = cx.slot(DIGITS, 1);

    let negative = if signed {
        cx.b.ins().icmp_imm(IntCC::SignedLessThan, v, 0)
    } else {
        cx.iconst(types::I8, 0)
    };
    let flipped = cx.b.ins().ineg(v);
    let magnitude = cx.b.ins().select(negative, flipped, v);

    let header = cx.b.create_block();
    cx.b.append_block_param(header, types::I64);
    cx.b.append_block_param(header, types::I64);
    let after = cx.b.create_block();
    cx.b.append_block_param(after, types::I64);
    let sign = cx.b.create_block();
    cx.b.append_block_param(sign, types::I64);
    let fin = cx.b.create_block();
    cx.b.append_block_param(fin, types::I64);

    let start = cx.iconst(types::I64, i64::from(DIGITS));
    cx.jump(header, &[start, magnitude]);

    cx.b.switch_to_block(header);
    let hp = cx.b.block_params(header).to_vec();
    let (Some(i), Some(u)) = (hp.first().copied(), hp.get(1).copied()) else { return };
    let ten = cx.iconst(types::I64, 10);
    let digit = cx.b.ins().urem(u, ten);
    let rest = cx.b.ins().udiv(u, ten);
    let at = cx.b.ins().iadd_imm(i, -1);
    let ch = cx.b.ins().iadd_imm(digit, 48);
    let byte = cx.b.ins().ireduce(types::I8, ch);
    let addr = cx.b.ins().iadd(buf, at);
    cx.b.ins().store(mem(), byte, addr, 0);
    let done = cx.b.ins().icmp_imm(IntCC::Equal, rest, 0);
    let then = [at];
    let els = [at, rest];
    cx.brif(done, after, &then, header, &els);

    cx.b.switch_to_block(after);
    let ap = cx.b.block_params(after).first().copied().unwrap_or(start);
    let one = [ap];
    cx.brif(negative, sign, &one, fin, &one);

    cx.b.switch_to_block(sign);
    let sp = cx.b.block_params(sign).first().copied().unwrap_or(start);
    let minus_at = cx.b.ins().iadd_imm(sp, -1);
    let minus = cx.iconst(types::I8, 45);
    let maddr = cx.b.ins().iadd(buf, minus_at);
    cx.b.ins().store(mem(), minus, maddr, 0);
    let arg = [minus_at];
    cx.jump(fin, &arg);

    cx.b.switch_to_block(fin);
    let fp = cx.b.block_params(fin).first().copied().unwrap_or(start);
    let total = cx.iconst(types::I64, i64::from(DIGITS));
    let n = cx.b.ins().isub(total, fp);
    let block = cx.alloc(n);
    let src = cx.b.ins().iadd(buf, fp);
    let cfg = cx.unit.module.isa().frontend_config();
    cx.b.call_memcpy(cfg, block, src, n);
    // Decimal digits and a minus sign are all below 0x80, so the rendering is
    // ASCII by construction and `str.len()` on it is a mask
    // (VALUE-MODEL.md §3.1).
    let len = cx.b.ins().bor_imm(n, STR_ASCII_FLAG as i64);
    cx.b.ins().return_(&[block, block, len]);
}

/// `true` and `false`, as literals: two statics and a branch.
fn show_bool(mut cx: Cx<'_, '_, '_>) {
    let p = entry(cx.b);
    let Some(v) = p.first().copied() else { return };
    let yes = cx.b.create_block();
    let no = cx.b.create_block();
    cx.brif(v, yes, &[], no, &[]);
    for (block, text) in [(yes, "true"), (no, "false")] {
        cx.b.switch_to_block(block);
        let Some(data) = cx.unit.bytes(text) else { continue };
        let gv = cx.unit.module.declare_data_in_func(data, cx.b.func);
        let ptr = cx.b.ins().symbol_value(PTR, gv);
        let zero = cx.iconst(PTR, 0);
        let len = cx.iconst(types::I64, (text.len() as u64 | STR_ASCII_FLAG) as i64);
        cx.b.ins().return_(&[zero, ptr, len]);
    }
}

/// The drop glue every closure environment shares: the block's first word is
/// the type-specific release function, and the record follows it.
fn env_glue(mut cx: Cx<'_, '_, '_>) {
    let p = entry(cx.b);
    let Some(block) = p.first().copied() else { return };
    let f = cx.load_at(PTR, block, 0);
    let live = cx.b.create_block();
    let done = cx.b.create_block();
    let none = cx.b.ins().icmp_imm(IntCC::Equal, f, 0);
    cx.brif(none, done, &[], live, &[]);
    cx.b.switch_to_block(live);
    let mut sig = Signature::new(cx.unit.abi.call_conv);
    sig.params.push(AbiParam::new(PTR));
    let sr = cx.b.import_signature(sig);
    let record = cx.offset(block, ENV_FIELDS);
    cx.b.ins().call_indirect(sr, f, &[record]);
    cx.jump(done, &[]);
    cx.b.switch_to_block(done);
    cx.b.ins().return_(&[]);
}

/// Release the contents of one value of a type: the per-type drop glue.
fn release(mut cx: Cx<'_, '_, '_>, ty: Option<Ty>) {
    let p = entry(cx.b);
    let Some(addr) = p.first().copied() else { return };
    if let Some(ty) = ty {
        cx.walk_rc(&ty, addr, false, 0);
    }
    cx.b.ins().return_(&[]);
}

/// Release every element of a `[T]` block.
///
/// The count is `cap / stride`, and `cap` is the second header word
/// (VALUE-MODEL.md §2) — which is what makes a drop glue taking only a pointer
/// enough for a list.
fn release_elems(mut cx: Cx<'_, '_, '_>, ty: Option<Ty>) {
    let p = entry(cx.b);
    let Some(addr) = p.first().copied() else { return };
    if let Some(elem) = ty {
        let stride = cx.unit.abi.layouts.of(elem.clone()).stride.max(1);
        let cap = cx.b.ins().load(types::I64, mem(), addr, HEADER_CAP_OFFSET);
        let count = cx.b.ins().udiv_imm(cap, i64::from(stride));
        cx.each_element(addr, count, stride, &elem, false);
    }
    cx.b.ins().return_(&[]);
}

/// One element of a `[T]`, retained in place.
///
/// The whole body is the counted-pointer walk with `retain = true`, which is
/// the same walk `release` runs with `retain = false`. Writing it as a second
/// helper rather than as a flag on `Release` keeps the two symbols distinct in
/// the object, which is what lets `cli/runtime/list.rs` take one as a plain
/// function pointer.
fn retain_elem(mut cx: Cx<'_, '_, '_>, ty: Option<Ty>) {
    let p = entry(cx.b);
    if let (Some(addr), Some(elem)) = (p.first().copied(), ty) {
        cx.walk_rc(&elem, addr, true, 0);
    }
    cx.b.ins().return_(&[]);
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
