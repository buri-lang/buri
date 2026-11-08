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

use crate::compiler::middle::ir::{Code, Inst, ValueId};
use crate::hash::{Map, Set};

/// One step of a path from a value to what it was read out of.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Step {
    Field(usize),
    Payload(usize, usize),
    /// Some element of a list.
    Elem,
}

/// Where each value was read from: the value at the root, and the steps.
///
/// A slice reads its list's elements, so it shares its list's path, and a
/// path through an element or a slice names more than one value.
#[derive(Clone, PartialEq, Eq)]
struct Path {
    root: ValueId,
    steps: Vec<Step>,
    /// Whether the path names one value: no element and no slice on it.
    exact: bool,
}

impl Path {
    /// Whether the two values are the same counts.
    fn same(&self, other: &Path) -> bool {
        self.exact && other.exact && self == other
    }

    /// Whether this value is a struct or payload the value at `x` is a field
    /// of, at any depth.
    fn holds(&self, x: &Path) -> bool {
        self.steps.len() < x.steps.len() && self.overlaps(x)
    }

    /// Whether one value holds the other, or they are the same.
    fn overlaps(&self, other: &Path) -> bool {
        self.root == other.root && self.steps.iter().zip(&other.steps).all(|(a, b)| a == b)
    }
}

/// Deletes every `incref`/`decref` pair in `code` that [the module
/// docs](self) license.
pub fn cancel(code: &mut Code) {
    let paths = paths(code);
    let mut uses = Vec::new();
    for block in code.blocks_mut() {
        let mut gone = vec![false; block.insts.len()];
        for (i, inst) in block.insts.iter().enumerate() {
            let Inst::IncRef { value: x } = inst else {
                continue;
            };
            if gone.get(i).copied().unwrap_or(true) {
                continue;
            }
            if let Some(j) = partner(&block.insts, &gone, i, &paths.of(*x), &paths, &mut uses) {
                for k in [i, j] {
                    if let Some(g) = gone.get_mut(k) {
                        *g = true;
                    }
                }
            }
        }
        if gone.contains(&true) {
            let mut flags = gone.iter();
            block
                .insts
                .retain(|_| !flags.next().copied().unwrap_or(false));
        }
    }
}

/// How many instructions past an `incref` its `decref` is looked for in, so
/// that a long block costs time in proportion to its length.
const WINDOW: usize = 32;

/// How many values [`Paths::reaches`] looks through before it answers yes,
/// which only ever keeps a pair.
const REACH: usize = 32;

/// The `decref` after `insts[i]` that cancels it, if one does. `x` is the
/// path of the value `insts[i]` retains.
fn partner(
    insts: &[Inst],
    gone: &[bool],
    i: usize,
    x: &Path,
    paths: &Paths,
    uses: &mut Vec<ValueId>,
) -> Option<usize> {
    // Whether a use that may give a count away has been seen. Until one has,
    // the value holds the struct's count, so reading it is safe; after one,
    // it may be gone.
    let mut given = false;
    let after = insts
        .iter()
        .zip(gone)
        .enumerate()
        .skip(i.saturating_add(1))
        .take(WINDOW);
    for (j, (inst, _)) in after.filter(|(_, (_, gone))| !**gone) {
        match inst {
            Inst::DecRef { value } if paths.of(*value).same(x) => return Some(j),
            // Another field of a struct that holds the value is no read of
            // the value at all.
            Inst::GetField { agg, .. } | Inst::GetPayload { agg, .. }
                if paths.of(*agg).holds(x) =>
            {
                continue
            }
            // A read takes no count, and what it reads is judged where it is
            // used. Another increment gives nothing away.
            Inst::GetField { .. }
            | Inst::GetPayload { .. }
            | Inst::GetTag { .. }
            | Inst::ArrayLen { .. }
            | Inst::ArrayGet { .. }
            | Inst::ArraySlice { .. }
            | Inst::IncRef { .. }
                if !given =>
            {
                continue
            }
            _ => {}
        }
        uses.clear();
        inst.operands(uses);
        let mut overlapping = uses.iter().filter(|v| paths.reaches(**v, x));
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
    paths: Map<ValueId, Path>,
    /// A struct, enum, list or closure's operands, and every value a block
    /// parameter is passed.
    holds: Map<ValueId, Vec<ValueId>>,
}

impl Paths {
    fn of(&self, v: ValueId) -> Path {
        self.paths.get(&v).cloned().unwrap_or(Path {
            root: v,
            steps: Vec::new(),
            exact: true,
        })
    }

    /// Whether reading `v` may read the value at `x`, or something in it.
    /// Past [`REACH`] values it answers yes.
    fn reaches(&self, v: ValueId, x: &Path) -> bool {
        let mut seen: Set<ValueId> = Set::default();
        let mut work = vec![v];
        let mut looked: usize = 0;
        while let Some(v) = work.pop() {
            looked = looked.saturating_add(1);
            if looked > REACH {
                return true;
            }
            let path = self.of(v);
            if path.overlaps(x) {
                return true;
            }
            if seen.insert(path.root) {
                if let Some(held) = self.holds.get(&path.root) {
                    work.extend(held.iter().copied());
                }
            }
        }
        false
    }
}

fn paths(code: &Code) -> Paths {
    let mut holds: Map<ValueId, Vec<ValueId>> = Map::default();
    // A read's source, its step, and whether the read keeps the path exact.
    let mut defs: Map<ValueId, (ValueId, Option<Step>, bool)> = Map::default();
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
                Inst::ArrayGet { dest, array, .. } => (*dest, (*array, Some(Step::Elem), false)),
                Inst::ArraySlice { dest, array, .. } => (*dest, (*array, None, false)),
                Inst::MakeStruct { dest, fields, .. } | Inst::MakeEnum { dest, fields, .. } => {
                    holds
                        .entry(*dest)
                        .or_default()
                        .extend(fields.iter().copied());
                    continue;
                }
                Inst::MakeArray { dest, elems, .. } => {
                    holds
                        .entry(*dest)
                        .or_default()
                        .extend(elems.iter().copied());
                    continue;
                }
                Inst::MakeClosure {
                    dest,
                    env: Some(env),
                    ..
                } => {
                    holds.entry(*dest).or_default().push(env.value);
                    continue;
                }
                _ => continue,
            };
            defs.insert(dest, def);
        }
        for target in block.term.targets() {
            let params = &code.get(target.block).params;
            for (param, arg) in params.iter().zip(&target.args) {
                holds.entry(*param).or_default().push(*arg);
            }
        }
    }
    let mut out = Map::default();
    let mut order: Vec<ValueId> = defs.keys().copied().collect();
    order.sort_unstable_by_key(|v| v.index());
    for v in order {
        resolve(v, &defs, &mut out);
    }
    Paths { paths: out, holds }
}

/// `v`'s path, memoised in `out`.
fn resolve(
    v: ValueId,
    defs: &Map<ValueId, (ValueId, Option<Step>, bool)>,
    out: &mut Map<ValueId, Path>,
) -> Path {
    if let Some(p) = out.get(&v) {
        return p.clone();
    }
    let Some(&(from, step, exact)) = defs.get(&v) else {
        return Path {
            root: v,
            steps: Vec::new(),
            exact: true,
        };
    };
    let mut path = resolve(from, defs, out);
    path.steps.extend(step);
    path.exact &= exact;
    out.insert(v, path.clone());
    path
}
