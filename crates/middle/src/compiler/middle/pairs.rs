//! Cancelling an `incref` against a later `decref` of the same counts.
//!
//! `middle::rc` plans counts on the tree, one expression at a time, so a
//! function that reads a field of a struct it owns and then lets the struct
//! go pays twice: the read takes a count of its own, and the struct's drop
//! gives the struct's back a few instructions later.
//!
//! ```text
//! v15 = field.0 v1
//! incref v15
//! v16 = call core_list$replaceAt(v15, …)
//! v18 = field.0 v1
//! decref v18
//! ```
//!
//! The pair costs two count updates, and worse, the call between them sees
//! the list shared and copies it. `v15` and `v18` are the same list, so the
//! read can take the struct's count instead. This pass deletes both halves.
//!
//! # When it is sound
//!
//! Deleting the pair moves the struct's count to the value the `incref` was
//! for, so between the two the block is held once rather than twice. Within
//! one block, that is safe when:
//!
//!  * the `decref` releases exactly what the `incref` retained: the same
//!    value, or the same field path of the same value;
//!  * nothing between them can give that count away and then touch the
//!    value again. Any use of the value, or of something read out of it or
//!    holding it, other than a read that takes no count — a field, a payload,
//!    a tag, a length, an element — or another `incref`, may give it away:
//!    passing it on, or releasing a struct that holds it. After such a use,
//!    only the `decref` may touch the value, and that one use may name it
//!    only once.
//!
//! Past the `decref`, nothing changes: the value had only the `incref`'s
//! count there before, and it has only the struct's now.

use crate::compiler::middle::ir::{Block, Code, Inst, PerValue, ValueId};

/// One step of a path from a value to what it was read out of.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Field(usize),
    Payload(usize, usize),
    /// Some element of a list.
    Elem,
}

/// Where a value was read from: the value at the root, and the steps, which
/// are `len` steps of [`Paths::steps`] from `start`.
///
/// A slice reads its list's elements, so it shares its list's path, and a
/// path through an element or a slice names more than one value.
#[derive(Clone, Copy)]
struct Path {
    root: ValueId,
    start: u32,
    len: u32,
    /// Whether the path names one value: no element and no slice on it.
    exact: bool,
}

impl Path {
    /// The path of a value read out of nothing.
    fn root(v: ValueId) -> Path {
        Path {
            root: v,
            start: 0,
            len: 0,
            exact: true,
        }
    }
}

/// Deletes every `incref`/`decref` pair in `code` that [the module
/// docs](self) license.
pub fn cancel(code: &mut Code) {
    // Most functions have no block with an `incref` before a `decref`, and
    // for those there is nothing to build.
    if !code.blocks().iter().any(pairable) {
        return;
    }
    let paths = Paths::new(code);
    let mut scratch = Scratch::default();
    // The instructions already paired. An `incref` is never a `decref`
    // already paired: it pairs only with a `decref` after it.
    let mut gone: Vec<bool> = Vec::new();
    for block in code.blocks_mut() {
        if !pairable(block) {
            continue;
        }
        gone.clear();
        gone.resize(block.insts.len(), false);
        let mut any = false;
        for (i, inst) in block.insts.iter().enumerate() {
            let Inst::IncRef { value: x } = inst else {
                continue;
            };
            if let Some(j) = partner(&block.insts, &gone, i, paths.of(*x), &paths, &mut scratch) {
                // `j` is past `i`, so this marks the two of them.
                for g in gone
                    .iter_mut()
                    .skip(i)
                    .step_by(j.saturating_sub(i).max(1))
                    .take(2)
                {
                    *g = true;
                }
                any = true;
            }
        }
        if any {
            let mut k = 0usize;
            block.insts.retain(|_| {
                let keep = !gone.get(k).copied().unwrap_or(false);
                k = k.saturating_add(1);
                keep
            });
        }
    }
}

/// Whether a block has an `incref` with a `decref` after it.
fn pairable(block: &Block) -> bool {
    let mut inc = false;
    for inst in &block.insts {
        match inst {
            Inst::IncRef { .. } => inc = true,
            Inst::DecRef { .. } if inc => return true,
            _ => {}
        }
    }
    false
}

/// How many instructions past an `incref` its `decref` is looked for in, so
/// that a long block costs time in proportion to its length.
const WINDOW: usize = 32;

/// How many values [`Paths::reaches`] looks through before it answers yes,
/// which only ever keeps a pair.
const REACH: usize = 32;

/// The buffers [`partner`] and [`Paths::reaches`] reuse from one call to
/// the next.
#[derive(Default)]
struct Scratch {
    uses: Vec<ValueId>,
    seen: Vec<ValueId>,
    work: Vec<ValueId>,
}

/// The `decref` after `insts[i]` that cancels it, if one does. `x` is the
/// path of the value `insts[i]` retains.
fn partner(
    insts: &[Inst],
    gone: &[bool],
    i: usize,
    x: Path,
    paths: &Paths,
    scratch: &mut Scratch,
) -> Option<usize> {
    // Whether a use that may give a count away has been seen. Until one has,
    // the value holds the struct's count, so reading it is safe; after one,
    // it may be gone.
    let mut given = false;
    let after = insts
        .iter()
        .enumerate()
        .skip(i.saturating_add(1))
        .take(WINDOW);
    for (j, inst) in after.filter(|(j, _)| !gone.get(*j).copied().unwrap_or(false)) {
        // Whether what the instruction does with its operands is judged.
        let judged = match inst {
            Inst::DecRef { value } if paths.same(paths.of(*value), x) => return Some(j),
            // A read takes no count, and what it reads is judged where it is
            // used. Another field of a struct that holds the value is no read
            // of the value at all.
            Inst::GetField { agg, .. } | Inst::GetPayload { agg, .. } => {
                given && !paths.holds(paths.of(*agg), x)
            }
            // Another increment gives nothing away either.
            Inst::GetTag { .. }
            | Inst::ArrayLen { .. }
            | Inst::ArrayGet { .. }
            | Inst::ArraySlice { owned: false, .. }
            | Inst::IncRef { .. } => given,
            _ => true,
        };
        if !judged {
            continue;
        }
        let Scratch { uses, seen, work } = &mut *scratch;
        uses.clear();
        inst.operands(uses);
        let mut overlapping = uses.iter().filter(|v| paths.reaches(**v, x, seen, work));
        let first = overlapping.next().is_some();
        // A use after the count may have gone reads a freed block, and an
        // instruction that may take the count while it reads the value
        // through a second operand would find it unshared, and write it.
        if first && (given || overlapping.next().is_some()) {
            return None;
        }
        given |= first;
    }
    None
}

/// The [`Path`] of every value read out of another one, and what each value
/// built from others holds.
struct Paths {
    /// Every value's path.
    paths: PerValue<Path>,
    /// The steps every path is a run of. A path one step longer than the
    /// last one made extends it in place, so a chain of reads shares one run.
    steps: Vec<Step>,
    /// A struct, enum, list or closure's operands, and every value a block
    /// parameter is passed, as `(holder, held)`: by holder, and each holder's
    /// in the order they were found.
    held: Vec<(ValueId, ValueId)>,
}

/// A read's source, its step, and whether the read keeps the path exact.
type Def = (ValueId, Option<Step>, bool);

impl Paths {
    fn of(&self, v: ValueId) -> Path {
        *self.paths.get(v)
    }

    fn steps(&self, p: Path) -> &[Step] {
        let start = p.start as usize;
        self.steps
            .get(start..start.saturating_add(p.len as usize))
            .unwrap_or(&[])
    }

    /// Whether the two values are the same counts.
    fn same(&self, a: Path, b: Path) -> bool {
        a.exact && b.exact && a.root == b.root && self.steps(a) == self.steps(b)
    }

    /// Whether the value at `a` is a struct or payload the value at `x` is a
    /// field of, at any depth.
    fn holds(&self, a: Path, x: Path) -> bool {
        a.len < x.len && self.overlaps(a, x)
    }

    /// Whether one value holds the other, or they are the same.
    fn overlaps(&self, a: Path, b: Path) -> bool {
        a.root == b.root && self.steps(a).iter().zip(self.steps(b)).all(|(s, t)| s == t)
    }

    /// What value `v` holds.
    fn held(&self, v: ValueId) -> &[(ValueId, ValueId)] {
        let from = self.held.partition_point(|(h, _)| *h < v);
        let to = self.held.partition_point(|(h, _)| *h <= v);
        self.held.get(from..to).unwrap_or(&[])
    }

    /// Whether reading `v` may read the value at `x`, or something in it.
    /// Past [`REACH`] values it answers yes.
    fn reaches(
        &self,
        v: ValueId,
        x: Path,
        seen: &mut Vec<ValueId>,
        work: &mut Vec<ValueId>,
    ) -> bool {
        seen.clear();
        work.clear();
        work.push(v);
        let mut looked: usize = 0;
        while let Some(v) = work.pop() {
            looked = looked.saturating_add(1);
            if looked > REACH {
                return true;
            }
            let path = self.of(v);
            if self.overlaps(path, x) {
                return true;
            }
            if !seen.contains(&path.root) {
                seen.push(path.root);
                work.extend(self.held(path.root).iter().map(|(_, v)| *v));
            }
        }
        false
    }

    fn new(code: &Code) -> Paths {
        let mut held: Vec<(ValueId, ValueId)> = Vec::new();
        let mut defs: PerValue<Option<Def>> = code.per_value(None);
        for block in code.blocks() {
            for inst in &block.insts {
                let (dest, def) = match inst {
                    Inst::GetField {
                        dest, agg, field, ..
                    } => (*dest, (*agg, Some(Step::Field(field.index())), true)),
                    Inst::GetPayload {
                        dest,
                        agg,
                        variant,
                        field,
                        ..
                    } => (
                        *dest,
                        (
                            *agg,
                            Some(Step::Payload(variant.index(), field.index())),
                            true,
                        ),
                    ),
                    Inst::ArrayGet { dest, array, .. } => {
                        (*dest, (*array, Some(Step::Elem), false))
                    }
                    Inst::ArraySlice { dest, array, .. } => (*dest, (*array, None, false)),
                    Inst::MakeStruct { dest, fields, .. } | Inst::MakeEnum { dest, fields, .. } => {
                        held.extend(fields.iter().map(|f| (*dest, *f)));
                        continue;
                    }
                    Inst::MakeArray { dest, elems, .. } => {
                        held.extend(elems.iter().map(|e| (*dest, *e)));
                        continue;
                    }
                    Inst::MakeClosure {
                        dest,
                        env: Some(env),
                        ..
                    } => {
                        held.push((*dest, env.value));
                        continue;
                    }
                    _ => continue,
                };
                *defs.get_mut(dest) = Some(def);
            }
            for target in block.term.targets() {
                let params = &code.get(target.block).params;
                for (param, arg) in params.iter().zip(&target.args) {
                    held.push((*param, *arg));
                }
            }
        }
        // A stable sort keeps each holder's values in the order they were found.
        held.sort_by_key(|(h, _)| *h);

        let mut paths = Paths {
            paths: code.per_value_from(&mut PerValue::default(), Path::root),
            steps: Vec::new(),
            held,
        };
        // Each read's path, its source's first: a chain is walked up to the
        // first value already on one, then resolved back down. An id defined
        // by a read of itself is no SSA, and ends its own walk that way rather
        // than being walked for ever.
        let mut walked = code.per_value(false);
        let mut chain: Vec<(ValueId, Def)> = Vec::new();
        for v in code.value_ids() {
            let mut at = v;
            while let Some(def) = *defs.get(at) {
                let w = walked.get_mut(at);
                if *w {
                    break;
                }
                *w = true;
                chain.push((at, def));
                at = def.0;
            }
            while let Some((v, (from, step, exact))) = chain.pop() {
                let mut path = paths.of(from);
                path.exact &= exact;
                if let Some(step) = step {
                    let end = path.start.saturating_add(path.len) as usize;
                    if end != paths.steps.len() {
                        let start = path.start as usize;
                        let len = paths.steps.len();
                        paths
                            .steps
                            .extend_from_within(start..start.saturating_add(path.len as usize));
                        path.start = len as u32;
                    }
                    paths.steps.push(step);
                    path.len = path.len.saturating_add(1);
                }
                *paths.paths.get_mut(v) = path;
            }
        }
        paths
    }
}
