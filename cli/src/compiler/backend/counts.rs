//! Where the reference counts live inside a value (MEMORY.md §5.1), for both
//! native backends.
//!
//! Each backend walks [`Counts::sites`] once per operation ([`Op`]): retain,
//! release or copy. The table is memoised per type, so a unit asks each type's
//! question once however many reference operations name it.

use std::rc::Rc;

use crate::compiler::middle::layout::{self, EnumRepr, Layouts, Repr, Scalar};
use crate::compiler::semantics::types::{field_types, variant_types, Tables, Ty, TyKind};
use crate::hash::Map;

/// What a walk does at each count it reaches.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Op {
    Retain,
    Release,
    /// Replace each counted block with a fresh copy of its own (G5). Takes no
    /// count: a copy is not a share.
    Copy,
}

/// Whether a counted pointer can be null.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Counted {
    /// A `Str`'s `base`, a list's `ptr` or a closure's `env`: null means "no
    /// block here" (a literal, an empty list, a lambda that captured nothing).
    Nullable,
    /// The pointer a recursive type's field is behind.
    NonNull,
}

/// What drops or copies the *contents* of a counted block.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Glue {
    /// A `Str`'s bytes: nothing to release, but a copy also rebases the `ptr`
    /// that points into the block (`buri_rt_copy_str`).
    Str,
    /// A closure environment, which carries its own glue because `Ty::Fn` does
    /// not record what was captured.
    Env,
    /// A `[T]` block, element by element.
    Elems(Ty),
}

/// A field with counts in it, at a byte offset relative to its owner.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Field {
    pub offset: u32,
    /// Behind a pointer (VALUE-MODEL.md §5.2): one operation on the pointer
    /// and no descent, since the pointee is released by its type's own glue.
    pub boxed: bool,
    pub ty: Ty,
}

/// One live variant of a tagged enum, and its fields with counts in them.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Arm {
    pub variant: u32,
    pub fields: Box<[Field]>,
}

/// One place a count lives inside a value. Offsets are relative to the value.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Site {
    /// A nullable counted block pointer.
    Block { offset: u32, glue: Glue },
    Field(Field),
    /// Switch on the tag at offset zero, then walk only the live arm.
    Tagged { tag: Scalar, arms: Box<[Arm]> },
    /// A niche `Option`: walk the payload, in place, only where the pointer at
    /// `null_at` is not null. A `.None` writes that null and nothing else, so
    /// the rest of the payload is garbage.
    Guarded { null_at: u32, ty: Ty },
}

/// The memoised answers, one table per unit or worker.
#[derive(Default)]
pub struct Counts {
    counted: Map<Ty, bool>,
    sites: Map<Ty, Rc<[Site]>>,
    weights: Map<Ty, u32>,
}

impl Counts {
    /// Whether a value of this type owns any count at all. Most types don't,
    /// and `false` means no code.
    pub fn counted(&mut self, tables: &Tables, layouts: &mut Layouts<'_>, ty: &Ty) -> bool {
        if let Some(known) = self.counted.get(ty) {
            return *known;
        }
        // Recorded before the descent so a type graph is walked once per
        // distinct type. A recursive type reaches itself only through a box,
        // which answers `true` before descending.
        self.counted.insert(ty.clone(), false);
        let l = layouts.shared(ty);
        let answer = match &l.repr {
            Repr::Str | Repr::List | Repr::Closure => true,
            Repr::Aggregate => self.any(tables, layouts, ty, &field_types(tables, ty)),
            Repr::Enum { variants, .. } => (0..variants.len())
                .any(|v| self.any(tables, layouts, ty, &variant_types(tables, ty, v))),
            Repr::Zero | Repr::Scalar(_) => false,
        };
        self.counted.insert(ty.clone(), answer);
        answer
    }

    fn any(&mut self, tables: &Tables, layouts: &mut Layouts<'_>, owner: &Ty, fields: &[Ty]) -> bool {
        fields.iter().any(|f| layouts.boxes(owner, f) || self.counted(tables, layouts, f))
    }

    /// Every place a count lives inside one value of this type.
    pub fn sites(&mut self, tables: &Tables, layouts: &mut Layouts<'_>, ty: &Ty) -> Rc<[Site]> {
        if let Some(known) = self.sites.get(ty) {
            return known.clone();
        }
        let sites: Rc<[Site]> = self.build(tables, layouts, ty).into();
        self.sites.insert(ty.clone(), sites.clone());
        sites
    }

    /// How many tests and pointer operations a walk of one value of this type
    /// writes out, counting every arm of every enum inside it.
    ///
    /// A walk is open-coded, so this is the size of the code one reference
    /// operation on the value becomes. A pointer is one, a tag or a niche test
    /// is one, and a field inside the value adds its own walk.
    pub fn weight(&mut self, tables: &Tables, layouts: &mut Layouts<'_>, ty: &Ty) -> u32 {
        if let Some(known) = self.weights.get(ty) {
            return *known;
        }
        // A type reaches itself only through a box, which weighs one without
        // descending, so the descent below terminates.
        let mut total = 0u32;
        for site in self.sites(tables, layouts, ty).iter() {
            let here = match site {
                Site::Block { .. } => 1,
                Site::Field(f) => self.field_weight(tables, layouts, f),
                Site::Tagged { arms, .. } => arms
                    .iter()
                    .flat_map(|arm| arm.fields.iter())
                    .map(|f| self.field_weight(tables, layouts, f))
                    .fold(1u32, u32::saturating_add),
                Site::Guarded { ty, .. } => self.weight(tables, layouts, ty).saturating_add(1),
            };
            total = total.saturating_add(here);
        }
        self.weights.insert(ty.clone(), total);
        total
    }

    fn field_weight(&mut self, tables: &Tables, layouts: &mut Layouts<'_>, f: &Field) -> u32 {
        if f.boxed {
            1
        } else {
            self.weight(tables, layouts, &f.ty)
        }
    }

    fn build(&mut self, tables: &Tables, layouts: &mut Layouts<'_>, ty: &Ty) -> Vec<Site> {
        let l = layouts.shared(ty);
        match &l.repr {
            Repr::Zero | Repr::Scalar(_) => Vec::new(),
            Repr::Str => vec![Site::Block { offset: l.field(layout::STR_BASE), glue: Glue::Str }],
            Repr::List => match ty.kind() {
                TyKind::Array(elem) => vec![Site::Block {
                    offset: l.field(layout::LIST_PTR),
                    glue: Glue::Elems(*elem),
                }],
                _ => Vec::new(),
            },
            Repr::Closure => {
                vec![Site::Block { offset: l.field(layout::CLOSURE_ENV), glue: Glue::Env }]
            }
            Repr::Aggregate => self
                .fields(tables, layouts, ty, &field_types(tables, ty), &l.fields)
                .into_iter()
                .map(Site::Field)
                .collect(),
            Repr::Enum { repr, variants } => match *repr {
                EnumRepr::Bare { .. } => Vec::new(),
                EnumRepr::Niche { null_at } => {
                    let TyKind::Con(_, args) = ty.kind() else { return Vec::new() };
                    match args.first() {
                        Some(payload) if self.counted(tables, layouts, payload) => {
                            vec![Site::Guarded { null_at, ty: payload.clone() }]
                        }
                        _ => Vec::new(),
                    }
                }
                EnumRepr::Tagged { tag, .. } => {
                    let arms: Box<[Arm]> = variants
                        .iter()
                        .enumerate()
                        .filter_map(|(v, offsets)| {
                            let fields = variant_types(tables, ty, v);
                            let fields = self.fields(tables, layouts, ty, &fields, offsets);
                            (!fields.is_empty()).then(|| Arm {
                                variant: u32::try_from(v).unwrap_or(0),
                                fields: fields.into(),
                            })
                        })
                        .collect();
                    if arms.is_empty() {
                        Vec::new()
                    } else {
                        vec![Site::Tagged { tag, arms }]
                    }
                }
            },
        }
    }

    /// The fields of `owner` that hold counts, at the given offsets.
    fn fields(
        &mut self,
        tables: &Tables,
        layouts: &mut Layouts<'_>,
        owner: &Ty,
        fields: &[Ty],
        offsets: &[u32],
    ) -> Vec<Field> {
        let mut out = Vec::new();
        for (i, f) in fields.iter().enumerate() {
            let boxed = layouts.boxes(owner, f);
            if boxed || self.counted(tables, layouts, f) {
                let offset = offsets.get(i).copied().unwrap_or(0);
                out.push(Field { offset, boxed, ty: f.clone() });
            }
        }
        out
    }
}
