//! A generated `Show`, `Equal`, `Ordered`, `Hash`, `ToJson` and `FromJson` per
//! type.
//!
//! JavaScript walks a type descriptor at run time — `$D0`, `$D1`, and the
//! generic `$eq`/`$show`/`$json_of` that read them — because a megamorphic walk
//! is cheaper than the code a per-type expansion would ship, and artifact size
//! is what a JavaScript build is judged on. Natively neither half of that
//! holds: there is no engine to be megamorphic at, and the code is generated
//! once at compile time.
//!
//! So the native branch generates a function per type per operation, and the
//! descriptor tables do not exist in a native artifact at all.
//!
//! Design: `design/native/VALUE-MODEL.md` §9, `ARCHITECTURE.md` §2.1.
//!
//! # What a derive *is*, after monomorphization
//!
//! There is no `derive` node in the tree. `monomorphize::structural_call`
//! has already turned every derived conformance into one of:
//!
//! * `ExprKind::Intrinsic { name: "structuralEq" | "structuralCompare" |
//!   "structuralShow" | "structuralToJson" | "structuralHash", args }`, whose
//!   **last argument is an `Int` literal naming a descriptor** — the shape the
//!   run-time walker would have read;
//! * `ExprKind::StructuralEq`, whose descriptor is `Program::desc_index` at the
//!   first argument's type;
//! * a `FuncKind::Intrinsic` with `Func::desc` set — `json.decode` (`FromJson`)
//!   and the test runner's `report`, which are handed a descriptor rather than
//!   a value.
//!
//! This pass replaces the first two with direct calls to generated functions
//! and gives the third a body: `json.decode` calls a generated decoder
//! (§ *Decoding*), and the test reporter calls a generated `Show`
//! (§ *What is scaffolded*).
//!
//! # The shape of a generated function
//!
//! One function per `(operation, structural shape)`, over the layout rather
//! than over a descriptor:
//!
//! ```text
//! eq_Point(a: Point, b: Point): Bool      = a.0 == b.0 && a.1 == b.1
//! cmp_Point(a: Point, b: Point): Order    = match cmp_I64(a.0, b.0) { .Equal => .., c => c }
//! show_Point(x: Point): Str               = "Point { x: ${show_I64(x.0)}, y: ${show_Str(x.1)} }"
//! json_Point(x: Point): Json              = .Object([("x", json_I64(x.0)), ..])
//! hash_Point(h: U64, x: Point): U64       = hash_Str(hash_I64(mix(h, 2), x.0), x.1)
//! dec_Point(j: Json, p: Str): Result<Point, DecodeError>
//!     = match j { .Object(es) => { let x = dec_I64(member(es, "x", p)?, "${p}.x")?; .. }, .. }
//! ```
//!
//! Field access is by index, which is what `middle::layout` turns into an
//! offset; nothing in a generated body reads a name at run time.
//!
//! ## Where the recursion bottoms out
//!
//! Two places, and both are deliberately small enough that a backend can
//! implement them once rather than per type.
//!
//! **Primitives** become one type-directed intrinsic each, carrying the
//! operand type in `targs`:
//!
//! | Intrinsic | Signature | Meaning |
//! |---|---|---|
//! | `derivePrimShow` | `(T) -> Str` | `$show`'s primitive arm: a `Str` is quoted and escaped, a `Char` is `'c'`, a `Float` is `$f64`, an integer is decimal |
//! | `derivePrimJson` | `(T) -> Json` | `$json_of`'s primitive arm: `Bool` to `.Bool`, `Str`/`Char` to `.Str`, numbers to `.Num` |
//! | `derivePrimHash` | `(U64, T) -> U64` | `$mix`: FNV-1a over the value's bytes, `Str` character by character |
//!
//! Equality and ordering need no intrinsic: they are `ExprKind::Prim` at the
//! primitive the descriptor names, which every backend already emits.
//!
//! That is also why SPEC 7.2's `NaN == NaN` ruling cost this pass nothing. A
//! float field lowers to `PrimOp::Eq`, which is `BinOp::Eq`, which is the one
//! place each backend spells float equality — `stencil/emit.rs`'s `Binary` at
//! `Float` and `llvm/emit.rs`'s `float_equality`. The JavaScript backend needed its own
//! edit because it does *not* come through here: `eq_decl` in
//! `backend/js/generate.rs` is a second implementation of derived equality,
//! and `agreement.rs` is the only thing that compares the two.
//!
//! **Arrays** become one helper each, taking a **code pointer to the element's
//! generated function**, because a loop is not expressible in the layer-A tree
//! and every backend has the loop already:
//!
//! | Intrinsic | Signature |
//! |---|---|
//! | `deriveArrayEq` | `([T], [T], fn(T, T) -> Bool) -> Bool` |
//! | `deriveArrayCompare` | `([T], [T], fn(T, T) -> Order) -> Order` |
//! | `deriveArrayShow` | `([T], fn(T) -> Str) -> Str` — renders `[a, b]`, separator included |
//! | `deriveArrayJson` | `([T], fn(T) -> Json) -> [Json]` — the caller wraps it in `.Array` |
//! | `deriveArrayHash` | `(U64, [T], fn(U64, T) -> U64) -> U64` — mixes the length, then each element |
//!
//! Eight names in total, and they are the entire run-time surface a derived
//! conformance needs. It is stated here because this pass is the only thing
//! that emits them. `middle::lower` builds every `deriveArray*` loop but
//! `deriveArrayHash` (`lower/lists.rs`), and the backends answer the rest.
//!
//! ## Agreement with JavaScript
//!
//! The generated walk mirrors `backend/js/runtime.js` step for step — including
//! that a struct or tuple mixes its field *count* into a hash before its
//! fields, and that an enum with payloads mixes its arity and its tag. A
//! program that prints `x.hash()` prints the same number on both backends, and
//! the conformance suite is what would notice if it stopped doing so. Where
//! JavaScript's own representation leaks into its answer — `Some(None)` is a
//! sentinel object there and a niche-encoded pointer natively
//! (VALUE-MODEL.md §6) — the native side follows the *value*, and that
//! divergence is named in `design/native/DECISIONS.md`'s terms rather than
//! silently reproduced.
//!
//! # Sharing
//!
//! One function per **shape**, not per type: `struct Meters(I64)` and
//! `struct Seconds(I64)` share `equal`, `cmp` and `hash`, because a derived
//! comparison reads offsets and the two layouts are identical — VALUE-MODEL.md
//! §5 fixes layout as declaration order with natural alignment and no
//! reordering, so "same field types in the same order" *is* "same layout".
//! `show` and `toJson` print names, so their shape key carries the names and
//! the two do not share.
//!
//! The parameter type recorded on a shared function names whichever of the
//! sharing types was reached first. That is deliberate and is safe for exactly
//! the reason above; a backend that keyed on nominal identity rather than on
//! layout would be the thing that broke, and `layout::of` is not that.
//!
//! # Decoding
//!
//! `json.decode(ctx, value)` gets the body `dec_T(value, "$")`, and `dec_T`
//! is `runtime.js`'s `$json_into` at one shape: the same mapping, and on
//! failure the same `DecodeError` with the same path and the same words. The
//! path is the second parameter, so a nested decoder is handed `"${p}.x"` or
//! `"${p}[0]"`.
//!
//! The types a decoder names come off the `json.decode` it answers —
//! `Json` is its parameter, and `Result` and `DecodeError` are its result —
//! plus `Option`, which indexing a list answers and which
//! `monomorphize::Shapes::option` carries. Three helpers are minted once
//! ([`Helper`]), and a list decodes in a loop of its own
//! ([`Generator::decode_each`]).
//!
//! `ToJson::toJson` called on a primitive itself (`str.toJson`) is an
//! intrinsic too, and gets `derivePrimJson` as its body.
//!
//! # What is scaffolded
//!
//! * **The test reporter.** `testing_assert.report` is handed a descriptor the
//!   same way, and this pass answers it in place: where a `Show` was generated
//!   at that descriptor's type, `reporter_body` gives the intrinsic a body that
//!   calls it, so what reaches the runtime is a rendered `Str` rather than a
//!   value and a walk. Only a descriptor with no generated `Show` is left as it
//!   arrived.
//! * **`ExprKind::StructuralCmp`** is left alone: nothing in the front end
//!   constructs one today (`monomorphize` only rewrites it), so generating for
//!   it would be generating for a shape no test could reach.
//!
//! # The JavaScript path
//!
//! This pass runs from `middle::native` and nowhere else. `middle::run` — what
//! the JavaScript backend is handed — does not call it, so `$D0` and the
//! generic walkers remain exactly what a JavaScript artifact contains.
//! `derives::tests::the_js_path_still_carries_descriptor_walks` is the guard.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "every counter here is bounded by something already in memory: a \
              field index within one descriptor, a variant index within one \
              enum, a local index within one generated function. The one \
              subtraction is a `saturating_sub` on a path depth."
)]

use crate::compiler::backend::intrinsic_keys;
use crate::compiler::middle::flags;
use crate::compiler::middle::lower;
use crate::compiler::middle::monomorphize::{
    self, short_hash, ConShape, Desc, DescVariant, Func, FuncKind, Program,
};
use crate::compiler::semantics::name::Name;
use crate::compiler::semantics::typed::{
    self, Arm, Callee, Expr, ExprKind, FieldPat, PatKind, Pattern, PrimOp, TemplatePart,
};
use crate::compiler::semantics::types::{FuncIdx, LocalId, Prim, Ty, TyKind, TyConId};
use crate::diagnostics::Span;
use crate::hash::Map as HashMap;

/// FNV-1a's offset basis, which is where `$hash` starts. Written here as well
/// as in `runtime.js` because the two have to agree and neither can import the
/// other.
const HASH_SEED: u128 = 0x811c_9dc5;

/// The most variants a derived `compare` matches pairwise. Past it, the two
/// tags are ranked and the ranks compared ([`Generator::compare_enum`]).
const RANKED_COMPARE_MIN: usize = 8;

/// The most payload fields a derived function binds on entering a variant's
/// arm. Past it, the payload is read this many fields at a time, where they're
/// used ([`Generator::payload`]). One at a time re-tests the tag per field, and
/// `opt`'s jump threading is quadratic in those tests.
const EAGER_FIELDS_MAX: usize = 8;

/// The six operations a `derive` can stand for, once monomorphization has
/// resolved it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Op {
    Eq,
    Compare,
    Show,
    ToJson,
    Hash,
    /// Reached through `json.decode` rather than through a call site, so it is
    /// not in [`Op::all`].
    FromJson,
}

impl Op {
    /// The intrinsic name monomorphization left at a call site.
    pub fn intrinsic(self) -> &'static str {
        match self {
            Op::Eq => "structuralEq",
            Op::Compare => "structuralCompare",
            Op::Show => "structuralShow",
            Op::ToJson => "structuralToJson",
            Op::Hash => "structuralHash",
            Op::FromJson => JSON_DECODE,
        }
    }

    /// The short tag in a generated symbol.
    fn tag(self) -> &'static str {
        match self {
            Op::Eq => "eq",
            Op::Compare => "cmp",
            Op::Show => "show",
            Op::ToJson => "json",
            Op::Hash => "hash",
            Op::FromJson => "dec",
        }
    }

    /// Whether the operation *prints* names, which is what decides whether two
    /// layout-identical types may share one generated function.
    fn reads_names(self) -> bool {
        matches!(self, Op::Show | Op::ToJson | Op::FromJson)
    }

    /// How many values of the described type the generated function takes.
    /// `Hash` takes one, behind an accumulator.
    fn values(self) -> usize {
        match self {
            Op::Eq | Op::Compare => 2,
            Op::Show | Op::ToJson | Op::Hash => 1,
            // It makes one rather than taking one.
            Op::FromJson => 0,
        }
    }

    pub fn all() -> [Op; 5] {
        [Op::Eq, Op::Compare, Op::Show, Op::ToJson, Op::Hash]
    }
}

/// One generated function.
#[derive(Clone, Debug)]
pub struct Instance {
    pub op: Op,
    /// The descriptor the shape came from — the first one to reach this
    /// instance, when several share it.
    pub desc: usize,
    pub func: FuncIdx,
    /// The key two shapes have to agree on to share a function.
    pub shape: String,
}

/// Why an instance was not generated. Recorded rather than diagnosed: a shape
/// this pass declines is one the run-time walker still handles, so declining is
/// a missing optimisation and not a broken program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Declined {
    /// `Desc::Opaque` or `Desc::Reserved` somewhere in the shape.
    NoStructure,
    /// A type the descriptor names that `Program::desc_index` does not.
    NoType,
    /// The operation's own result type never appeared in the program, so there
    /// is nothing to give the generated function as a return type.
    NoResultType,
}

/// What the pass did.
///
/// Returned rather than stored: the pass writes its answers into the program
/// itself, and this is the record of *which* answers it wrote. `middle::native`
/// drops it — nothing downstream reads a row — and what keeps every field here
/// is this file's own tests, which are where "one function per shape" and
/// "a declined shape is still handled by the walker" are checked at all.
#[derive(Default, Debug)]
pub struct Derives {
    /// Every generated function, in generation order.
    pub instances: Vec<Instance>,
    /// Which function serves `(operation, descriptor)`. More rows than
    /// [`Derives::instances`] exactly where two shapes share one function.
    pub routes: Vec<(Op, usize, FuncIdx)>,
    /// Shapes that were asked for and not generated.
    pub declined: Vec<(Op, usize, Declined)>,
    /// How many call sites became direct calls.
    pub rewritten: usize,
    /// Descriptors `json.decode` was handed, each of which gets a decoder.
    pub from_json: Vec<usize>,
}

/// Adds one derived function per shape per structural operation the program
/// reaches, and rewrites every derive call site to a direct call.
pub fn run(program: &mut Program) -> Derives {
    let mut g = Generator::new(program);
    let wanted = collect(program, &mut g.out);
    for (op, desc) in wanted {
        g.request(op, desc);
    }
    for desc in g.out.from_json.clone() {
        g.request(Op::FromJson, desc);
    }
    g.drain();
    let built = g.finish();
    program.funcs.extend(built.funcs);
    route_cells(program, &built.routed);
    let mut out = built.out;
    rewrite(program, &built.routed, &built.hash_ty, &mut out);
    for f in &mut program.funcs {
        prim_to_json_body(f);
    }
    out
}

/// Gives `ToJson::toJson` at a primitive — `str.toJson`, `number.U8.toJson` —
/// the body `derivePrimJson(self)`, which is the leaf a derived `ToJson`
/// reaches at a field of that type.
fn prim_to_json_body(f: &mut Func) {
    if f.intrinsic_key().and_then(intrinsic_keys::prim_to_json).is_none() {
        return;
    }
    let Some(this) = f.params.first().copied() else { return };
    let Some(ty) = f.locals.get(this.index()).map(|l| l.ty) else { return };
    let x = Expr::new(ExprKind::Local(this), ty, Span::NONE);
    let name = String::from("derivePrimJson");
    let body = ExprKind::Intrinsic { name, targs: vec![ty], args: vec![x] };
    let ret = f.ret;
    f.set_body(Expr::new(body, ret, Span::NONE));
}

/// Records, per reactive cell type, the generated `Equal` a backend hands the
/// graph.
///
/// The intrinsic carries the *descriptor* (`monomorphize::build_fn`) and the
/// backend has a *type*, so this is the one place the two are both in hand:
/// `desc_index` read backwards is what turns one into the other, and the route
/// is what generation produced for it. A cell whose type declined generation —
/// an opaque, or a shape with no structure to walk — simply has no row, and the
/// runtime falls back to comparing the bytes.
fn route_cells(program: &mut Program, routed: &HashMap<(Op, usize), FuncIdx>) {
    let wanted: Vec<usize> = program
        .funcs
        .iter()
        .filter(|f| {
            f.intrinsic_key().is_some_and(|k| monomorphize::CELL_VALUE_KEYS.contains(&k))
        })
        .filter_map(|f| f.desc)
        .collect();
    let mut rows: Vec<(Ty, FuncIdx)> = Vec::new();
    for desc in wanted {
        let Some(func) = routed.get(&(Op::Eq, desc)).copied() else { continue };
        if let Some((ty, _)) = program.desc_index.iter().find(|(_, i)| **i == desc) {
            rows.push((*ty, func));
        }
    }
    // A cell whose type reaches a hand-written `Equal` already has the
    // comparison `monomorphize` generated for it, which this one would miss.
    for (ty, func) in rows {
        program.cell_equal.entry(ty).or_insert(func);
    }
}

/// What generation produced, before it is spliced into the program.
struct Built {
    funcs: Vec<Func>,
    routed: HashMap<(Op, usize), FuncIdx>,
    /// The type `Hash` accumulates in, needed for the seed at a call site.
    hash_ty: Ty,
    out: Derives,
}

// ---------------------------------------------------------------------------
// Finding the call sites
// ---------------------------------------------------------------------------

/// Every `(operation, descriptor)` the program asks for, deduplicated and in a
/// deterministic order.
///
/// Also fills in the two places a descriptor reaches an *intrinsic function*
/// rather than an expression, which this pass reports rather than rewrites.
fn collect(program: &Program, out: &mut Derives) -> Vec<(Op, usize)> {
    let mut seen: Vec<(Op, usize)> = Vec::new();
    let push = |op: Op, d: usize, seen: &mut Vec<(Op, usize)>| {
        if !seen.contains(&(op, d)) {
            seen.push((op, d));
        }
    };
    for f in &program.funcs {
        if let (Some(key), Some(d)) = (f.intrinsic_key(), f.desc) {
            if key == JSON_DECODE {
                if !out.from_json.contains(&d) {
                    out.from_json.push(d);
                }
            } else if monomorphize::CELL_VALUE_KEYS.contains(&key) {
                // A reactive cell's value type: what the graph compares a write
                // against, so that writing a value equal to the one already
                // there re-runs nothing. `monomorphize::build_fn` put the
                // descriptor here for exactly this.
                push(Op::Eq, d, &mut seen);
            } else {
                push(Op::Show, d, &mut seen);
            }
        }
        let Some(body) = f.body() else { continue };
        typed::walk(body, &mut |e| match &e.kind {
            ExprKind::Intrinsic { name, args, .. } => {
                let Some(op) = Op::all().into_iter().find(|o| o.intrinsic() == name) else {
                    return;
                };
                if let Some(d) = descriptor_arg(args) {
                    push(op, d, &mut seen);
                }
            }
            ExprKind::StructuralEq { args, .. } => {
                if let Some(d) = args.first().and_then(|a| program.desc_index.get(&a.ty)) {
                    push(Op::Eq, *d, &mut seen);
                }
            }
            _ => {}
        });
    }
    seen
}

/// The descriptor `structural_call` appended to the argument list.
fn descriptor_arg(args: &[Expr]) -> Option<usize> {
    match args.last().map(|a| &a.kind) {
        Some(ExprKind::Int(v, false)) => usize::try_from(v.get()).ok(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The types a generated body needs
// ---------------------------------------------------------------------------

/// The type information this pass has, which is a `Program` and no `Tables`.
///
/// Everything below is *learned from the program itself*: the type a
/// descriptor describes is `desc_index` read backwards, and the result type of
/// each operation is the type of a call site that asked for it. That is enough
/// precisely because a generated function is only ever needed where a call site
/// exists.
///
/// The one thing a *call site* cannot answer is a primitive a generated body
/// needs and the program never mentions — a `Bool` in a program that derives
/// `Ordered` and no `Equal` — and `Program::shapes` closes it, because it is every
/// declared type rather than the reached ones. [`Environment::discover`] reads it last,
/// so it fills gaps and overrides nothing.
struct Env {
    /// Descriptor index to the type it describes.
    ty_of: Vec<Option<Ty>>,
    /// The type of a primitive, where the program mentions one.
    prim_of: HashMap<Prim, Ty>,
    /// The result type of each operation, from a call site.
    result: HashMap<Op, Ty>,
    /// `Json`'s variant order, if the program described the type; otherwise the
    /// order `core/json` declares.
    json_variants: Vec<String>,
    /// What a decoder spells, where the program calls `json.decode`.
    decoding: Option<Decoding>,
}

/// The types a generated decoder names, read off the `json.decode` it answers:
/// `decode(ctx, value: Json): Result<T, DecodeError>`.
#[derive(Clone, Copy)]
struct Decoding {
    json: Ty,
    result: TyConId,
    error: Ty,
    option: TyConId,
}

impl Decoding {
    fn of(program: &Program) -> Option<Decoding> {
        let f = program.funcs.iter().find(|f| f.intrinsic_key() == Some(JSON_DECODE))?;
        let json = f.locals.get(f.params.get(1)?.index())?.ty;
        let TyKind::Con(result, args) = f.ret.kind() else { return None };
        let error = *args.get(1)?;
        Some(Decoding { json, result: *result, error, option: program.shapes.option? })
    }
}

/// `core/json`'s declaration order, which `runtime.js` also hard-codes and for
/// the same reason: a walker that builds a library type without the library's
/// help has to know its tags.
const JSON_VARIANTS: [&str; 6] = ["Null", "Bool", "Num", "Str", "Array", "Object"];

impl Env {
    fn discover(program: &Program) -> Env {
        let mut ty_of: Vec<Option<Ty>> = vec![None; program.descriptors.len()];
        for (ty, i) in &program.desc_index {
            if let Some(slot) = ty_of.get_mut(*i) {
                *slot = Some(*ty);
            }
        }
        let mut prim_of: HashMap<Prim, Ty> = HashMap::default();
        for (i, d) in program.descriptors.iter().enumerate() {
            if let (Desc::Prim(p), Some(Some(ty))) = (d, ty_of.get(i)) {
                prim_of.entry(*p).or_insert_with(|| *ty);
            }
        }
        let mut result: HashMap<Op, Ty> = HashMap::default();
        let mut json_variants: Vec<String> = Vec::new();
        for f in &program.funcs {
            let Some(body) = f.body() else { continue };
            typed::walk(body, &mut |e| {
                match &e.kind {
                    // A literal is the cheapest place to learn a primitive's
                    // type, and the three that name their own primitive
                    // unambiguously are the three a generated body writes.
                    ExprKind::Str(_) => {
                        prim_of.entry(Prim::Str).or_insert_with(|| e.ty);
                    }
                    ExprKind::Bool(_) => {
                        prim_of.entry(Prim::Bool).or_insert_with(|| e.ty);
                    }
                    ExprKind::Char(_) => {
                        prim_of.entry(Prim::Char).or_insert_with(|| e.ty);
                    }
                    ExprKind::Prim { prim, args, .. } => {
                        if let Some(a) = args.first() {
                            prim_of.entry(*prim).or_insert_with(|| a.ty);
                        }
                    }
                    ExprKind::StructuralEq { .. } => {
                        result.entry(Op::Eq).or_insert_with(|| e.ty);
                    }
                    ExprKind::Intrinsic { name, .. } => {
                        if let Some(op) = Op::all().into_iter().find(|o| o.intrinsic() == name) {
                            result.entry(op).or_insert_with(|| e.ty);
                        }
                    }
                    _ => {}
                }
            });
        }
        // `Str` is needed for an object key even in a program that never shows
        // anything, and `Show`'s result is a `Str`.
        if let Some(s) = result.get(&Op::Show).cloned() {
            prim_of.entry(Prim::Str).or_insert(s);
        }
        // And the other direction, for the same identity. A test suite asks for
        // `Show` at every type it asserts on without ever calling `show`, so
        // there is no `structuralShow` call site to read the result type from —
        // rendering a failure is the runner's, not the program's. A program
        // that names a `Str` anywhere can answer one, and every assertion names
        // its kind as a literal.
        if let Some(s) = prim_of.get(&Prim::Str).cloned() {
            result.entry(Op::Show).or_insert(s);
        }
        // The tags of `Json`, from the program where it described the type.
        for (i, d) in program.descriptors.iter().enumerate() {
            let Desc::Enum { name, variants } = d else { continue };
            if name != "Json" {
                continue;
            }
            let is_json = result
                .get(&Op::ToJson)
                .and_then(|t| t.head())
                .zip(ty_of.get(i).and_then(|t| t.as_ref()).and_then(|t| t.head()))
                .is_some_and(|(a, b)| a == b);
            if is_json {
                json_variants = variants.iter().map(|v| v.name.clone()).collect();
            }
        }
        if json_variants.is_empty() {
            json_variants = JSON_VARIANTS.iter().map(|s| (*s).to_string()).collect();
        }
        // Whatever is still missing, off `Program::shapes` — which is *every*
        // declared type rather than the reached ones, so it answers where the
        // readings above cannot: a program that derives `Ordered` and never asks
        // for `==`, never writes a `Bool` literal and never spells a comparison
        // of its own has no `structuralEq` call site and no literal to read
        // one from, and its generated `compare` was then built with conditions
        // of type `Ty::Error` — reported by the verifier as "branches on a
        // value that is not a Bool" (buri-lang/buri#27). A primitive's type is
        // its constructor applied to nothing, and `shapes.cons` is indexed by
        // `TyConId`, so the index *is* the constructor.
        //
        // Last, not first, so that every type this pass used to learn from the
        // program is still the one it learns: `Str` and `Template` are two
        // constructors over the same primitive (`middle::lower`), and which of
        // them a body means is a question only the body answers.
        for (i, shape) in program.shapes.cons.iter().enumerate() {
            if let ConShape::Prim(p) = shape {
                prim_of
                    .entry(*p)
                    .or_insert_with(|| Ty::con(TyConId(i as u32), []));
            }
        }
        // `Equal` answers a `Bool`, and a program can hold a signal without
        // ever writing a comparison of its own — nothing in
        // `web.navigate(ctx, path)` is an `==`. The cutoff's comparison is
        // generated for such a program all the same, so where no call site
        // named the result type it comes off the primitive. Last, after the
        // loop above, because that loop is what answers `Bool` in a program
        // with no `Bool` literal in it.
        if let Some(b) = prim_of.get(&Prim::Bool).cloned() {
            result.entry(Op::Eq).or_insert(b);
        }
        Env { ty_of, prim_of, result, json_variants, decoding: Decoding::of(program) }
    }

    fn ty(&self, desc: usize) -> Option<&Ty> {
        self.ty_of.get(desc).and_then(|t| t.as_ref())
    }

    fn result(&self, op: Op) -> Option<&Ty> {
        self.result.get(&op)
    }

    fn json_variant(&self, name: &str) -> Option<usize> {
        self.json_variants.iter().position(|v| v == name)
    }
}

// ---------------------------------------------------------------------------
// Support: which shapes can be generated at all
// ---------------------------------------------------------------------------

/// Whether each descriptor has a structure a generated function can walk.
///
/// A least fixpoint over "unsupported": an `Opaque` or `Reserved` descriptor,
/// or one whose type the program does not name, poisons everything that
/// reaches it. A *cycle* is supported — a recursive type's generated function
/// calls itself, which is exactly what the reserved-slot-first construction
/// below is for.
fn support(program: &Program, env: &Env) -> Vec<bool> {
    let mut ok: Vec<bool> = program
        .descriptors
        .iter()
        .enumerate()
        .map(|(i, d)| !matches!(d, Desc::Opaque(_) | Desc::Reserved) && env.ty(i).is_some())
        .collect();
    let mut changed = true;
    while changed {
        changed = false;
        for (i, d) in program.descriptors.iter().enumerate() {
            if !ok.get(i).copied().unwrap_or(false) {
                continue;
            }
            let good = children(d).into_iter().all(|c| ok.get(c).copied().unwrap_or(false));
            if !good {
                if let Some(slot) = ok.get_mut(i) {
                    *slot = false;
                }
                changed = true;
            }
        }
    }
    ok
}

/// The descriptors one descriptor names directly.
fn children(d: &Desc) -> Vec<usize> {
    match d {
        Desc::Prim(_) | Desc::Unit | Desc::Opaque(_) | Desc::Reserved => Vec::new(),
        Desc::Struct { fields, .. } | Desc::Flags { fields, .. } => {
            fields.iter().map(|f| f.ty).collect()
        }
        Desc::Enum { variants, .. } => {
            variants.iter().flat_map(|v| v.fields.iter().map(|f| f.ty)).collect()
        }
        Desc::Array(e) | Desc::Option(e) => vec![*e],
        Desc::Tuple(es) => es.clone(),
    }
}

// ---------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------

/// Adjacent literal text as one part: a field name and the separator before it
/// are written separately and are one string.
fn merge(parts: Vec<TemplatePart>) -> Vec<TemplatePart> {
    let mut merged: Vec<TemplatePart> = Vec::new();
    for p in parts {
        match (merged.last_mut(), p) {
            (Some(TemplatePart::Text(prev)), TemplatePart::Text(next)) => prev.push_str(&next),
            (_, p) => merged.push(p),
        }
    }
    merged
}

/// One side's pattern bindings: each field's index, local and type.
type Binds = Vec<(usize, LocalId, Ty)>;

/// How a derived function reads variant `vi`'s payload out of each of its
/// sides: the arm's patterns, one per side, and the fields they bind.
struct Payload {
    ty: Ty,
    vi: usize,
    sides: Vec<Expr>,
    /// Each field's descriptor and type.
    fields: Vec<(usize, Ty)>,
    patterns: Vec<Pattern>,
    /// Each field's value on every side, or `None` where the patterns bind
    /// nothing and [`Generator::read`] reads the fields where they're used.
    bound: Option<Vec<Vec<Expr>>>,
}

impl Payload {
    /// The runs of fields the payload is read in: all of them where the arm
    /// binds them, and [`EAGER_FIELDS_MAX`] at a time where it doesn't.
    fn runs(&self) -> Vec<std::ops::Range<usize>> {
        let n = self.fields.len();
        let step = if self.bound.is_some() { n.max(1) } else { EAGER_FIELDS_MAX };
        (0..n).step_by(step).map(|start| start..n.min(start + step)).collect()
    }
}

/// One generated function under construction: its locals, and the parameters
/// among them.
struct Frame {
    locals: Vec<typed::Local>,
    params: Vec<LocalId>,
}

impl Frame {
    fn new() -> Frame {
        Frame { locals: Vec::new(), params: Vec::new() }
    }

    fn local(&mut self, name: &str, ty: &Ty) -> LocalId {
        let id = LocalId(u32::try_from(self.locals.len()).unwrap_or(u32::MAX));
        self.locals.push(typed::Local { name: Name::new(name), ty: *ty, span: Span::NONE });
        id
    }

    fn param(&mut self, name: &str, ty: &Ty) -> LocalId {
        let id = self.local(name, ty);
        self.params.push(id);
        id
    }
}

struct Generator {
    descs: std::rc::Rc<Vec<Desc>>,
    /// The module each descriptor's type is declared in, from
    /// [`Program::desc_modules`]. A generated function's debug name is
    /// qualified with it, which is what puts it in that module's codegen unit.
    modules: Vec<Option<String>>,
    env: Env,
    ok: Vec<bool>,
    /// Where the generated functions start in `Program::funcs`.
    base: usize,
    funcs: Vec<Func>,
    /// Shape key to the function that implements it.
    shared: HashMap<String, FuncIdx>,
    /// Symbol to the shape it was minted for, so that two shapes whose hashes
    /// collide cannot be given one symbol and one body.
    taken: HashMap<String, String>,
    /// `(op, descriptor)` to the function that serves it.
    routed: HashMap<(Op, usize), FuncIdx>,
    /// Instances whose body is still to be built.
    queue: Vec<(Op, usize, FuncIdx)>,
    /// Arity to the shared string joiner of that arity ([`Generator::joiner`]).
    joiners: HashMap<usize, FuncIdx>,
    /// The three helpers every decoder shares ([`Generator::helper`]).
    helpers: HashMap<Helper, FuncIdx>,
    /// The runtime operations decoders call, by key and parameter types
    /// ([`Generator::runtime`]).
    runtime: HashMap<(String, Vec<Ty>), FuncIdx>,
    out: Derives,
}

/// A function every generated decoder shares, minted on first use.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Helper {
    /// `found(j: Json): Str` — what the document held, for the message.
    Found,
    /// `wrong(p: Str, wanted: Str, j: Json): DecodeError`.
    Wrong,
    /// `member(es: [(Str, Json)], key: Str, p: Str, i: Int): Result<Json, DecodeError>`.
    Member,
}

impl Generator {
    fn new(program: &Program) -> Generator {
        let env = Env::discover(program);
        let ok = support(program, &env);
        Generator {
            descs: std::rc::Rc::new(program.descriptors.clone()),
            modules: program.desc_modules.clone(),
            env,
            ok,
            base: program.funcs.len(),
            funcs: Vec::new(),
            shared: HashMap::default(),
            taken: HashMap::default(),
            routed: HashMap::default(),
            queue: Vec::new(),
            joiners: HashMap::default(),
            helpers: HashMap::default(),
            runtime: HashMap::default(),
            out: Derives::default(),
        }
    }

    /// Drops any instance whose body could not be built, so that no call site
    /// is ever rewritten to a function that is still `Unbuilt`. `support`
    /// should have ruled all of these out already; this is what makes "should"
    /// unnecessary to trust.
    fn finish(mut self) -> Built {
        let base = self.base;
        let unbuilt: Vec<FuncIdx> = self
            .funcs
            .iter()
            .enumerate()
            .filter(|(_, f)| matches!(f.kind, FuncKind::Unbuilt))
            .filter_map(|(i, _)| u32::try_from(base + i).ok().map(FuncIdx))
            .collect();
        if !unbuilt.is_empty() {
            self.routed.retain(|_, f| !unbuilt.contains(f));
            for i in self.out.instances.iter().filter(|i| unbuilt.contains(&i.func)) {
                self.out.declined.push((i.op, i.desc, Declined::NoStructure));
            }
            self.out.instances.retain(|i| !unbuilt.contains(&i.func));
        }
        let mut routes: Vec<(Op, usize, FuncIdx)> =
            self.routed.iter().map(|((op, d), f)| (*op, *d, *f)).collect();
        routes.sort_by_key(|(op, d, f)| (*op, *d, f.0));
        self.out.routes = routes;
        let hash_ty = self.result_ty(Op::Hash);
        Built { funcs: self.funcs, routed: self.routed, hash_ty, out: self.out }
    }

    fn desc(&self, i: usize) -> Option<&Desc> {
        self.descs.get(i)
    }

    /// Asks for one instance, generating it — and everything it reaches — if it
    /// is not already there.
    fn request(&mut self, op: Op, desc: usize) -> Option<FuncIdx> {
        if let Some(f) = self.routed.get(&(op, desc)) {
            return Some(*f);
        }
        if !self.ok.get(desc).copied().unwrap_or(false) {
            let why = if self.env.ty(desc).is_none() { Declined::NoType } else { Declined::NoStructure };
            self.decline(op, desc, why);
            return None;
        }
        let Some(ret) = self.op_ret(op, desc) else {
            self.decline(op, desc, Declined::NoResultType);
            return None;
        };
        // `ToJson` writes object keys and `FromJson` reads them, so both need
        // `Str` as well.
        let keyed = matches!(op, Op::ToJson | Op::FromJson);
        if keyed && !self.env.prim_of.contains_key(&Prim::Str) {
            self.decline(op, desc, Declined::NoResultType);
            return None;
        }
        let shape = self.shape_key(op, desc);
        if let Some(f) = self.shared.get(&shape).copied() {
            self.routed.insert((op, desc), f);
            return Some(f);
        }
        // The slot is reserved *before* the body is built, so a recursive type
        // finds itself rather than recursing forever.
        let idx = FuncIdx(u32::try_from(self.base + self.funcs.len()).unwrap_or(u32::MAX));
        let name = self.symbol(op, desc, &shape);
        // Qualified with the module that declares the type, so that
        // `lower::unit_name` puts the function in that module's codegen unit.
        // Unqualified, every derived function in the program is in `root`,
        // which at 118k lines was 65x the median unit and was invalidated by
        // any new derive anywhere.
        let debug_name = match self.modules.get(desc).and_then(Option::as_ref) {
            Some(module) => format!("{module}:{name}"),
            None => name.clone(),
        };
        self.funcs.push(Func {
            symbol: name.clone(),
            debug_name,
            params: Vec::new(),
            locals: Vec::new(),
            kind: FuncKind::Unbuilt,
            ret,
            desc: None,
            span: Span::NONE,
        });
        self.shared.insert(shape.clone(), idx);
        self.routed.insert((op, desc), idx);
        self.out.instances.push(Instance { op, desc, func: idx, shape });
        self.queue.push((op, desc, idx));
        Some(idx)
    }

    /// What the instance at `desc` answers. Every operation but `FromJson`
    /// answers one type whatever it is at; a decoder answers
    /// `Result<T, DecodeError>` at its own `T`.
    fn op_ret(&self, op: Op, desc: usize) -> Option<Ty> {
        match op {
            Op::FromJson => self.decoded(&self.ty_of(desc)),
            _ => self.env.result(op).cloned(),
        }
    }

    fn decline(&mut self, op: Op, desc: usize, why: Declined) {
        if !self.out.declined.iter().any(|(o, d, _)| *o == op && *d == desc) {
            self.out.declined.push((op, desc, why));
        }
    }

    /// A stable, readable symbol. Two shapes that share a function share the
    /// name of whichever reached it first, which is the same rule
    /// `monomorphize` uses for an instantiation.
    ///
    /// The tail is a hash of the *shape*, never the descriptor index. A
    /// descriptor index is a program-global interning position, so a new type
    /// described anywhere renamed every derived function after it — and the
    /// symbol is what the `codegen` key renders a callee by, so that renaming
    /// invalidated every unit that calls one. The shape is what decides which
    /// functions are the same function, so it is the identity the name should
    /// carry.
    ///
    /// A symbol names exactly one body: on the hash collision that would give
    /// two shapes one name, the descriptor index disambiguates, and a symbol
    /// with a fifth `$` cannot equal one with four.
    fn symbol(&mut self, op: Op, desc: usize, shape: &str) -> String {
        let base = match self.desc(desc) {
            Some(Desc::Struct { name, .. })
            | Some(Desc::Enum { name, .. })
            | Some(Desc::Flags { name, .. }) => name.clone(),
            Some(Desc::Prim(p)) => p.name().to_string(),
            Some(Desc::Array(_)) => "list".to_string(),
            Some(Desc::Tuple(_)) => "tuple".to_string(),
            Some(Desc::Option(_)) => "option".to_string(),
            Some(Desc::Unit) => "unit".to_string(),
            _ => "value".to_string(),
        };
        let clean: String = base
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
            .collect();
        let stem = format!("$derive${}${clean}", op.tag());
        let hash = short_hash(shape);
        let mut out = format!("{stem}${hash}");
        if self.taken.get(&out).is_some_and(|s| s != shape) {
            out = format!("{stem}${hash}${desc}");
        }
        self.taken.insert(out.clone(), shape.to_string());
        out
    }

    /// Builds every queued body, including the ones building a body queues.
    fn drain(&mut self) {
        while let Some((op, desc, idx)) = self.queue.pop() {
            let built = self.body(op, desc);
            let Some((frame, expr)) = built else { continue };
            let Some(slot) = self.funcs.get_mut(idx.index().saturating_sub(self.base)) else {
                continue;
            };
            slot.params = frame.params;
            slot.locals = frame.locals;
            slot.kind = FuncKind::Body(expr);
        }
    }

    // -- the shape key ------------------------------------------------------

    /// The key two descriptors have to agree on to share a generated function.
    ///
    /// Structural, with a back-reference for a cycle, so a recursive type has a
    /// finite key. Names are in the key only for the operations that print
    /// them.
    fn shape_key(&self, op: Op, desc: usize) -> String {
        let mut out = String::from(op.tag());
        out.push(':');
        let mut path: Vec<usize> = Vec::new();
        self.key_into(op, desc, &mut path, &mut out);
        out
    }

    fn key_into(&self, op: Op, desc: usize, path: &mut Vec<usize>, out: &mut String) {
        if let Some(pos) = path.iter().position(|d| *d == desc) {
            out.push_str(&format!("^{}", path.len().saturating_sub(pos)));
            return;
        }
        path.push(desc);
        let named = op.reads_names();
        match self.desc(desc) {
            Some(Desc::Prim(p)) => out.push_str(&format!("p{}", p.name())),
            // Compared and hashed as its word, so only the operations that
            // print names tell two of these apart.
            Some(Desc::Flags { name, record, fields, prim }) => {
                out.push_str(&format!("f{}(", prim.name()));
                if named {
                    out.push_str(name);
                    out.push_str(if *record { "{" } else { "(" });
                    for f in fields {
                        out.push_str(&f.name);
                        out.push(',');
                    }
                }
                out.push(')');
            }
            Some(Desc::Unit) => out.push('u'),
            Some(Desc::Struct { name, record, fields }) => {
                out.push_str("s(");
                if named {
                    out.push_str(name);
                    out.push_str(if *record { "{" } else { "(" });
                }
                for f in fields {
                    if named {
                        out.push_str(&f.name);
                        out.push(':');
                    }
                    self.key_into(op, f.ty, path, out);
                    out.push(',');
                }
                out.push(')');
            }
            Some(Desc::Enum { name, variants }) => {
                out.push_str("e(");
                if named {
                    out.push_str(name);
                }
                for v in variants {
                    out.push('|');
                    if named {
                        out.push_str(&v.name);
                        out.push_str(if v.record { "{" } else { "(" });
                    }
                    for f in &v.fields {
                        if named {
                            out.push_str(&f.name);
                            out.push(':');
                        }
                        self.key_into(op, f.ty, path, out);
                        out.push(',');
                    }
                }
                out.push(')');
            }
            Some(Desc::Array(e)) => {
                out.push_str("a(");
                self.key_into(op, *e, path, out);
                out.push(')');
            }
            Some(Desc::Option(e)) => {
                out.push_str("o(");
                self.key_into(op, *e, path, out);
                out.push(')');
            }
            Some(Desc::Tuple(es)) => {
                out.push_str("t(");
                for e in es {
                    self.key_into(op, *e, path, out);
                    out.push(',');
                }
                out.push(')');
            }
            _ => out.push('?'),
        }
        path.pop();
    }

    // -- small builders -----------------------------------------------------

    fn ty_of(&self, desc: usize) -> Ty {
        self.env.ty(desc).cloned().unwrap_or(Ty::ERROR)
    }

    fn result_ty(&self, op: Op) -> Ty {
        self.env.result(op).cloned().unwrap_or(Ty::ERROR)
    }

    fn str_ty(&self) -> Ty {
        self.env.prim_of.get(&Prim::Str).cloned().unwrap_or(Ty::ERROR)
    }

    /// `Bool`, which a generated `compare` needs for its `if` even in a
    /// program that derives no `Equal`.
    ///
    /// It read `result(Op::Eq)` alone, which is the type of a `structuralEq`
    /// **call site** — so a program that derives `Ordered` and never asks for `==`
    /// had no `Bool` at all, and every `if (a < b)` in a generated `compare`
    /// was built with a condition of type `Ty::Error`. The verifier caught it
    /// as "branches on a value that is not a Bool" rather than as a missing
    /// type (buri-lang/buri#27). `prim_of[Bool]` is the same type, read off a
    /// `Bool` literal where the program wrote one and off `Program::shapes`
    /// where it did not — and the shape table is every *declared* type, so it
    /// answers whatever the program happens to contain.
    fn bool_ty(&self) -> Ty {
        self.env
            .result(Op::Eq)
            .or_else(|| self.env.prim_of.get(&Prim::Bool))
            .cloned()
            .unwrap_or(Ty::ERROR)
    }

    /// The word a `Flags` descriptor's value is stored in.
    fn word(&self, desc: usize, prim: Prim, fields: usize) -> flags::Word {
        flags::Word { prim, fields, ty: self.ty_of(desc), bool_ty: self.bool_ty(), span: Span::NONE }
    }

    fn local_expr(&self, id: LocalId, ty: &Ty) -> Expr {
        Expr::new(ExprKind::Local(id), *ty, Span::NONE)
    }

    fn str_lit(&self, s: &str) -> Expr {
        Expr::new(ExprKind::Str(s.to_string()), self.str_ty(), Span::NONE)
    }

    fn call(&self, f: FuncIdx, args: Vec<Expr>, ret: Ty) -> Expr {
        Expr::new(ExprKind::CallFn { func: Callee::Func(f), args }, ret, Span::NONE)
    }

    fn intrinsic(&self, name: &str, targs: Vec<Ty>, args: Vec<Expr>, ret: Ty) -> Expr {
        Expr::new(
            ExprKind::Intrinsic { name: name.to_string(), targs, args },
            ret,
            Span::NONE,
        )
    }

    /// A code pointer to a generated function, for the array helpers.
    fn fn_ref(&self, f: FuncIdx, params: Vec<Ty>, ret: Ty) -> Expr {
        Expr::new(
            ExprKind::FnRef(Callee::Func(f)),
            Ty::func(params, ret),
            Span::NONE,
        )
    }

    /// `x.i`, by index — a struct field or a tuple element, whichever the
    /// descriptor says.
    fn project(&self, base: Expr, index: usize, tuple: bool, ty: Ty) -> Expr {
        let kind = if tuple {
            ExprKind::TupleIndex { base: Box::new(base), index }
        } else {
            ExprKind::Field { base: Box::new(base), index }
        };
        Expr::new(kind, ty, Span::NONE)
    }

    fn variant_pattern(
        &self,
        ty: &Ty,
        variant: usize,
        binds: &[(usize, LocalId, Ty)],
    ) -> Option<Pattern> {
        let con = ty.head()?;
        let fields = binds
            .iter()
            .map(|(i, l, t)| FieldPat {
                index: *i,
                pattern: Pattern {
                    kind: PatKind::Bind { local: *l, sub: None },
                    ty: *t,
                    span: Span::NONE,
                },
            })
            .collect();
        Some(Pattern {
            kind: PatKind::Variant { con, variant, fields },
            ty: *ty,
            span: Span::NONE,
        })
    }

    fn wild(&self, ty: &Ty) -> Pattern {
        Pattern { kind: PatKind::Wild, ty: *ty, span: Span::NONE }
    }

    fn arm(&self, pattern: Pattern, body: Expr) -> Arm {
        Arm { pattern, guard: None, body, span: Span::NONE }
    }

    fn match_(&self, scrutinee: Expr, arms: Vec<Arm>, ty: Ty) -> Expr {
        Expr::new(
            ExprKind::Match { scrutinee: Box::new(scrutinee), arms },
            ty,
            Span::NONE,
        )
    }

    fn enum_lit(&self, ty: &Ty, variant: usize, args: Vec<Expr>) -> Option<Expr> {
        let con: TyConId = ty.head()?;
        let targs = match ty.kind() {
            TyKind::Con(_, a) => a.to_vec(),
            _ => Vec::new(),
        };
        Some(Expr::new(
            ExprKind::EnumLit { con, targs, variant, args },
            *ty,
            Span::NONE,
        ))
    }

    // -- the bodies ---------------------------------------------------------

    /// The parameters every generated function of one operation takes, and its
    /// body.
    fn body(&mut self, op: Op, desc: usize) -> Option<(Frame, Expr)> {
        let ty = self.ty_of(desc);
        let mut frame = Frame::new();
        let expr = match op {
            Op::Eq | Op::Compare => {
                let a = frame.param("a", &ty);
                let b = frame.param("b", &ty);
                let (ae, be) = (self.local_expr(a, &ty), self.local_expr(b, &ty));
                if op == Op::Eq {
                    self.eq(desc, ae, be, &mut frame)?
                } else {
                    self.compare(desc, ae, be, &mut frame)?
                }
            }
            Op::Show => {
                let x = frame.param("x", &ty);
                let xe = self.local_expr(x, &ty);
                self.show(desc, xe, &mut frame)?
            }
            Op::ToJson => {
                let x = frame.param("x", &ty);
                let xe = self.local_expr(x, &ty);
                self.json_of(desc, xe, &mut frame)?
            }
            Op::Hash => {
                let acc = self.result_ty(Op::Hash);
                let h = frame.param("h", &acc);
                let x = frame.param("x", &ty);
                let (he, xe) = (self.local_expr(h, &acc), self.local_expr(x, &ty));
                self.hash(desc, he, xe, &mut frame)?
            }
            Op::FromJson => {
                let json = self.env.decoding?.json;
                let j = frame.param("j", &json);
                let p = frame.param("p", &self.str_ty());
                self.decode(desc, j, p, &mut frame)?
            }
        };
        Some((frame, expr))
    }

    /// Whether an expression may be written twice without changing what the
    /// program does or what it costs. A projection of a local is; a call is
    /// not.
    fn duplicable(e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Local(_)
            | ExprKind::Int(..)
            | ExprKind::Float(_)
            | ExprKind::Str(_)
            | ExprKind::Char(_)
            | ExprKind::Bool(_)
            | ExprKind::Unit => true,
            ExprKind::Field { base, .. } | ExprKind::TupleIndex { base, .. } => {
                Generator::duplicable(base)
            }
            _ => false,
        }
    }

    /// The operation at one descriptor, either inlined or as a call to the
    /// generated function for it.
    fn eq(&mut self, desc: usize, a: Expr, b: Expr, frame: &mut Frame) -> Option<Expr> {
        let bool_ty = self.bool_ty();
        let descs = std::rc::Rc::clone(&self.descs);
        match descs.get(desc)? {
            Desc::Prim(p) => Some(Expr::new(
                ExprKind::Prim { op: PrimOp::Eq, prim: *p, args: vec![a, b] },
                bool_ty,
                Span::NONE,
            )),
            Desc::Flags { prim, .. } => Some(self.prim_test(PrimOp::Eq, *prim, a, b)),
            Desc::Unit => Some(Expr::new(ExprKind::Bool(true), bool_ty, Span::NONE)),
            Desc::Struct { fields, .. } => {
                let parts: Vec<(usize, usize)> =
                    fields.iter().enumerate().map(|(i, f)| (i, f.ty)).collect();
                self.eq_fields(&parts, a, b, false)
            }
            Desc::Tuple(es) => {
                let parts: Vec<(usize, usize)> =
                    es.iter().enumerate().map(|(i, d)| (i, *d)).collect();
                self.eq_fields(&parts, a, b, true)
            }
            Desc::Array(elem) => {
                let elem_ty = self.ty_of(*elem);
                let f = self.request(Op::Eq, *elem)?;
                let ptr = self.fn_ref(f, vec![elem_ty, elem_ty], bool_ty);
                Some(self.intrinsic("deriveArrayEq", vec![elem_ty], vec![a, b, ptr], bool_ty))
            }
            Desc::Option(inner) => self.eq_option(desc, *inner, a, b, frame),
            Desc::Enum { variants, .. } => self.eq_enum(desc, variants, a, b, frame),
            Desc::Opaque(_) | Desc::Reserved => None,
        }
    }

    /// `a.0 == b.0 && a.1 == b.1`, right-nested so the first difference stops
    /// the walk.
    fn eq_fields(
        &mut self,
        fields: &[(usize, usize)],
        a: Expr,
        b: Expr,
        tuple: bool,
    ) -> Option<Expr> {
        let bool_ty = self.bool_ty();
        let mut acc: Option<Expr> = None;
        for (i, d) in fields.iter().rev() {
            let fty = self.ty_of(*d);
            let ae = self.project(a.clone(), *i, tuple, fty);
            let be = self.project(b.clone(), *i, tuple, fty);
            let one = self.at(Op::Eq, *d, vec![ae, be])?;
            acc = Some(self.then_eq(one, acc));
        }
        Some(acc.unwrap_or_else(|| Expr::new(ExprKind::Bool(true), bool_ty, Span::NONE)))
    }

    fn eq_option(
        &mut self,
        desc: usize,
        inner: usize,
        a: Expr,
        b: Expr,
        frame: &mut Frame,
    ) -> Option<Expr> {
        let ty = self.ty_of(desc);
        let inner_ty = self.ty_of(inner);
        let bool_ty = self.bool_ty();
        let (x, y) = (frame.local("x", &inner_ty), frame.local("y", &inner_ty));
        let some_x = self.variant_pattern(&ty, OPTION_SOME, &[(0, x, inner_ty)])?;
        let some_y = self.variant_pattern(&ty, OPTION_SOME, &[(0, y, inner_ty)])?;
        let none = self.variant_pattern(&ty, OPTION_NONE, &[])?;
        let inner_eq = self.at(
            Op::Eq,
            inner,
            vec![self.local_expr(x, &inner_ty), self.local_expr(y, &inner_ty)],
        )?;
        let false_ = Expr::new(ExprKind::Bool(false), bool_ty, Span::NONE);
        let true_ = Expr::new(ExprKind::Bool(true), bool_ty, Span::NONE);
        let some_arm = self.match_(
            b.clone(),
            vec![self.arm(some_y, inner_eq), self.arm(self.wild(&ty), false_.clone())],
            bool_ty,
        );
        let none_arm = self.match_(
            b,
            vec![self.arm(none, true_), self.arm(self.wild(&ty), false_)],
            bool_ty,
        );
        Some(self.match_(
            a,
            vec![self.arm(some_x, some_arm), self.arm(self.wild(&ty), none_arm)],
            bool_ty,
        ))
    }

    fn eq_enum(
        &mut self,
        desc: usize,
        variants: &[DescVariant],
        a: Expr,
        b: Expr,
        frame: &mut Frame,
    ) -> Option<Expr> {
        let ty = self.ty_of(desc);
        let bool_ty = self.bool_ty();
        let mut arms: Vec<Arm> = Vec::new();
        let false_ = Expr::new(ExprKind::Bool(false), bool_ty, Span::NONE);
        for (vi, v) in variants.iter().enumerate() {
            let payload = self.payload(&ty, vi, v, &[a.clone(), b.clone()], frame)?;
            let mut acc: Option<Expr> = None;
            for run in payload.runs().into_iter().rev() {
                let descs = payload.fields.get(run.clone())?.to_vec();
                acc = Some(self.read(&payload, run, frame, false_.clone(), |g, _, values| {
                    let mut acc = acc;
                    for ((d, _), xs) in descs.iter().zip(values).rev() {
                        let one = g.at(Op::Eq, *d, xs)?;
                        acc = Some(g.then_eq(one, acc));
                    }
                    acc
                })?);
            }
            let same = acc.unwrap_or_else(|| {
                Expr::new(ExprKind::Bool(true), bool_ty, Span::NONE)
            });
            let mut patterns = payload.patterns.into_iter();
            let (px, py) = (patterns.next()?, patterns.next()?);
            let inner = self.match_(
                b.clone(),
                vec![self.arm(py, same), self.arm(self.wild(&ty), false_.clone())],
                bool_ty,
            );
            arms.push(self.arm(px, inner));
        }
        Some(self.match_(a, arms, bool_ty))
    }

    // -- ordering -----------------------------------------------------------

    fn order_lit(&self, which: usize) -> Option<Expr> {
        let ty = self.result_ty(Op::Compare);
        self.enum_lit(&ty, which, Vec::new())
    }

    /// `a < b` as `Less`, `a > b` as `Greater`, and `otherwise` for the rest.
    fn compare_prim(&self, p: Prim, a: Expr, b: Expr, otherwise: Expr) -> Option<Expr> {
        let order = self.result_ty(Op::Compare);
        let gt = self.prim_test(PrimOp::Gt, p, a.clone(), b.clone());
        let inner = self.choose(gt, self.order_lit(ORDER_GREATER)?, otherwise, order);
        let lt = self.prim_test(PrimOp::Lt, p, a, b);
        Some(self.choose(lt, self.order_lit(ORDER_LESS)?, inner, order))
    }

    /// A float's `compare`: `-inf < … < -0.0 < 0.0 < … < inf < NaN`, every NaN
    /// equal to every other. `==` at a float is SPEC 7.2's, where every NaN
    /// equals every other, so `x == NaN` is how a NaN is found, and `1 / x` is
    /// how a zero's sign is read. `stencil/emit.rs`'s `compare_float` is the
    /// same steps.
    fn compare_float(&self, desc: usize, p: Prim, a: Expr, b: Expr) -> Option<Expr> {
        let order = self.result_ty(Op::Compare);
        let ty = self.ty_of(desc);
        let float = |v: f64| Expr::new(ExprKind::Float(v), ty, Span::NONE);
        let reciprocal = |x: Expr| {
            let args = vec![float(1.0), x];
            Expr::new(ExprKind::Prim { op: PrimOp::Div, prim: p, args }, ty, Span::NONE)
        };
        let equal = self.order_lit(ORDER_EQUAL)?;
        let zeros = self.compare_prim(p, reciprocal(a.clone()), reciprocal(b.clone()), equal)?;
        let one_nan = self.choose(
            self.prim_test(PrimOp::Eq, p, a.clone(), float(f64::NAN)),
            self.order_lit(ORDER_GREATER)?,
            self.order_lit(ORDER_LESS)?,
            order,
        );
        let same = self.prim_test(PrimOp::Eq, p, a.clone(), b.clone());
        let rest = self.choose(same, zeros, one_nan, order);
        self.compare_prim(p, a, b, rest)
    }

    fn prim_test(&self, op: PrimOp, p: Prim, a: Expr, b: Expr) -> Expr {
        Expr::new(ExprKind::Prim { op, prim: p, args: vec![a, b] }, self.bool_ty(), Span::NONE)
    }

    fn choose(&self, cond: Expr, then: Expr, else_: Expr, ty: Ty) -> Expr {
        Expr::new(
            ExprKind::If { cond: Box::new(cond), then: Box::new(then), else_: Box::new(else_) },
            ty,
            Span::NONE,
        )
    }

    fn compare(&mut self, desc: usize, a: Expr, b: Expr, frame: &mut Frame) -> Option<Expr> {
        let order = self.result_ty(Op::Compare);
        let descs = std::rc::Rc::clone(&self.descs);
        match descs.get(desc)? {
            Desc::Prim(p) if p.is_float() => self.compare_float(desc, *p, a, b),
            Desc::Prim(p) | Desc::Flags { prim: p, .. } => {
                let otherwise = self.order_lit(ORDER_EQUAL)?;
                self.compare_prim(*p, a, b, otherwise)
            }
            Desc::Unit => self.order_lit(ORDER_EQUAL),
            Desc::Struct { fields, .. } => {
                let parts: Vec<(usize, usize)> =
                    fields.iter().enumerate().map(|(i, f)| (i, f.ty)).collect();
                self.compare_fields(&parts, a, b, false, frame)
            }
            Desc::Tuple(es) => {
                let parts: Vec<(usize, usize)> =
                    es.iter().enumerate().map(|(i, d)| (i, *d)).collect();
                self.compare_fields(&parts, a, b, true, frame)
            }
            Desc::Array(elem) => {
                let elem_ty = self.ty_of(*elem);
                let f = self.request(Op::Compare, *elem)?;
                let ptr = self.fn_ref(f, vec![elem_ty, elem_ty], order);
                Some(self.intrinsic(
                    "deriveArrayCompare",
                    vec![elem_ty],
                    vec![a, b, ptr],
                    order,
                ))
            }
            Desc::Option(inner) => self.compare_option(desc, *inner, a, b, frame),
            Desc::Enum { variants, .. } => self.compare_enum(desc, variants, a, b, frame),
            Desc::Opaque(_) | Desc::Reserved => None,
        }
    }

    /// `match cmp(a.0, b.0) { .Equal => <the rest>, c => c }`, which is the
    /// lexicographic order VALUE-MODEL.md and `$cmp` both give.
    fn compare_fields(
        &mut self,
        fields: &[(usize, usize)],
        a: Expr,
        b: Expr,
        tuple: bool,
        frame: &mut Frame,
    ) -> Option<Expr> {
        let mut acc: Option<Expr> = None;
        for (i, d) in fields.iter().rev() {
            let fty = self.ty_of(*d);
            let ae = self.project(a.clone(), *i, tuple, fty);
            let be = self.project(b.clone(), *i, tuple, fty);
            let one = self.at(Op::Compare, *d, vec![ae, be])?;
            acc = Some(self.then_compare(one, acc, frame)?);
        }
        match acc {
            Some(e) => Some(e),
            None => self.order_lit(ORDER_EQUAL),
        }
    }

    fn compare_option(
        &mut self,
        desc: usize,
        inner: usize,
        a: Expr,
        b: Expr,
        frame: &mut Frame,
    ) -> Option<Expr> {
        // `.None` sorts before every `.Some`, which is what `$cmp` does with
        // the `undefined` it represents `None` as.
        let ty = self.ty_of(desc);
        let inner_ty = self.ty_of(inner);
        let order = self.result_ty(Op::Compare);
        let (x, y) = (frame.local("x", &inner_ty), frame.local("y", &inner_ty));
        let some_x = self.variant_pattern(&ty, OPTION_SOME, &[(0, x, inner_ty)])?;
        let some_y = self.variant_pattern(&ty, OPTION_SOME, &[(0, y, inner_ty)])?;
        let none = self.variant_pattern(&ty, OPTION_NONE, &[])?;
        let inner_cmp = self.at(
            Op::Compare,
            inner,
            vec![self.local_expr(x, &inner_ty), self.local_expr(y, &inner_ty)],
        )?;
        let some_arm = self.match_(
            b.clone(),
            vec![
                self.arm(some_y, inner_cmp),
                self.arm(self.wild(&ty), self.order_lit(ORDER_GREATER)?),
            ],
            order,
        );
        let none_arm = self.match_(
            b,
            vec![
                self.arm(none, self.order_lit(ORDER_EQUAL)?),
                self.arm(self.wild(&ty), self.order_lit(ORDER_LESS)?),
            ],
            order,
        );
        Some(self.match_(
            a,
            vec![self.arm(some_x, some_arm), self.arm(self.wild(&ty), none_arm)],
            order,
        ))
    }

    /// Tag first, then payload — declaration order is the order, which is what
    /// makes `derive Ordered` on an enum mean what a reader of the declaration
    /// expects.
    ///
    /// Up to [`RANKED_COMPARE_MIN`] variants, each arm for `a` matches `b`
    /// against every variant, which is a direct jump per pair. Past it the
    /// variants are ranked once each and the ranks compared, because `n²` arms
    /// for `n` variants was 90,000 arms at 300 (PERFORMANCE.md §6.32).
    fn compare_enum(
        &mut self,
        desc: usize,
        variants: &[DescVariant],
        a: Expr,
        b: Expr,
        frame: &mut Frame,
    ) -> Option<Expr> {
        if variants.len() > RANKED_COMPARE_MIN {
            if let Some(int) = self.env.prim_of.get(&Prim::I64).cloned() {
                return self.compare_enum_ranked(desc, variants, a, b, int, frame);
            }
        }
        let ty = self.ty_of(desc);
        let order = self.result_ty(Op::Compare);
        let mut arms: Vec<Arm> = Vec::new();
        for (vi, v) in variants.iter().enumerate() {
            let sides = [a.clone(), b.clone()];
            let (px, py, same) = self.same_variant(&ty, vi, v, &sides, frame)?;
            // A lower-numbered variant on the right means this one is greater.
            let mut inner: Vec<Arm> = Vec::new();
            for wi in 0..variants.len() {
                if wi == vi {
                    inner.push(self.arm(py.clone(), same.clone()));
                    continue;
                }
                let pat = self.tag_pattern(&ty, wi)?;
                let which =
                    if wi < vi { self.order_lit(ORDER_GREATER)? } else { self.order_lit(ORDER_LESS)? };
                inner.push(self.arm(pat, which));
            }
            arms.push(self.arm(px, self.match_(b.clone(), inner, order)));
        }
        Some(self.match_(a, arms, order))
    }

    /// The arm patterns for variant `vi` on each of `sides`, and the
    /// lexicographic comparison of the two payloads.
    fn same_variant(
        &mut self,
        ty: &Ty,
        vi: usize,
        v: &DescVariant,
        sides: &[Expr],
        frame: &mut Frame,
    ) -> Option<(Pattern, Pattern, Expr)> {
        let payload = self.payload(ty, vi, v, sides, frame)?;
        let equal_lit = self.order_lit(ORDER_EQUAL)?;
        let mut acc: Option<Expr> = None;
        for run in payload.runs().into_iter().rev() {
            let descs = payload.fields.get(run.clone())?.to_vec();
            acc = Some(self.read(&payload, run, frame, equal_lit.clone(), |g, frame, values| {
                let mut acc = acc;
                for ((d, _), xs) in descs.iter().zip(values).rev() {
                    let one = g.at(Op::Compare, *d, xs)?;
                    acc = Some(g.then_compare(one, acc, frame)?);
                }
                acc
            })?);
        }
        let same = acc.unwrap_or(equal_lit);
        let mut patterns = payload.patterns.into_iter();
        Some((patterns.next()?, patterns.next()?, same))
    }

    /// `compare` for a wide enum, in code linear in its variants:
    ///
    /// ```text
    /// let ra = match a { .V0 => 0, .V1 => 1, … };
    /// let rb = match b { .V0 => 0, .V1 => 1, … };
    /// if ra < rb { .Less } else if ra > rb { .Greater } else {
    ///     match a { .V0(x) => match b { .V0(y) => <payloads>, _ => .Equal }, …, _ => .Equal }
    /// }
    /// ```
    fn compare_enum_ranked(
        &mut self,
        desc: usize,
        variants: &[DescVariant],
        a: Expr,
        b: Expr,
        int: Ty,
        frame: &mut Frame,
    ) -> Option<Expr> {
        let ty = self.ty_of(desc);
        let order = self.result_ty(Op::Compare);
        let bool_ty = self.bool_ty();
        let rank = |this: &Self, x: Expr| -> Option<Expr> {
            let mut arms = Vec::with_capacity(variants.len());
            for vi in 0..variants.len() {
                let lit = Expr::new(
                    ExprKind::Int(typed::Magnitude::new(vi as u128), false),
                    int,
                    Span::NONE,
                );
                arms.push(this.arm(this.tag_pattern(&ty, vi)?, lit));
            }
            Some(this.match_(x, arms, int))
        };
        let (ra, rb) = (frame.local("ra", &int), frame.local("rb", &int));
        let let_rank = |this: &Self, local: LocalId, x: Expr| -> Option<typed::Stmt> {
            Some(typed::Stmt::Let {
                pattern: Pattern {
                    kind: PatKind::Bind { local, sub: None },
                    ty: int,
                    span: Span::NONE,
                },
                value: rank(this, x)?,
                span: Span::NONE,
            })
        };
        let stmts = vec![let_rank(self, ra, a.clone())?, let_rank(self, rb, b.clone())?];

        let mut arms: Vec<Arm> = Vec::new();
        for (vi, v) in variants.iter().enumerate() {
            if v.fields.is_empty() {
                continue;
            }
            let sides = [a.clone(), b.clone()];
            let (px, py, same) = self.same_variant(&ty, vi, v, &sides, frame)?;
            let inner = self.match_(
                b.clone(),
                vec![self.arm(py, same), self.arm(self.wild(&ty), self.order_lit(ORDER_EQUAL)?)],
                order,
            );
            arms.push(self.arm(px, inner));
        }
        let payloads = if arms.is_empty() {
            self.order_lit(ORDER_EQUAL)?
        } else {
            if arms.len() < variants.len() {
                arms.push(self.arm(self.wild(&ty), self.order_lit(ORDER_EQUAL)?));
            }
            self.match_(a, arms, order)
        };
        let ranks = |op: PrimOp| {
            Expr::new(
                ExprKind::Prim {
                    op,
                    prim: Prim::I64,
                    args: vec![self.local_expr(ra, &int), self.local_expr(rb, &int)],
                },
                bool_ty,
                Span::NONE,
            )
        };
        let greater = Expr::new(
            ExprKind::If {
                cond: Box::new(ranks(PrimOp::Gt)),
                then: Box::new(self.order_lit(ORDER_GREATER)?),
                else_: Box::new(payloads),
            },
            order,
            Span::NONE,
        );
        let body = Expr::new(
            ExprKind::If {
                cond: Box::new(ranks(PrimOp::Lt)),
                then: Box::new(self.order_lit(ORDER_LESS)?),
                else_: Box::new(greater),
            },
            order,
            Span::NONE,
        );
        Some(Expr::new(ExprKind::Block { stmts, tail: Some(Box::new(body)) }, order, Span::NONE))
    }

    /// A variant pattern that binds nothing, for a test that only reads the
    /// tag. Fields are matched with `_`, which is what makes it legal at a
    /// variant that carries a payload.
    fn tag_pattern(&self, ty: &Ty, variant: usize) -> Option<Pattern> {
        let con = ty.head()?;
        Some(Pattern {
            kind: PatKind::Variant { con, variant, fields: Vec::new() },
            ty: *ty,
            span: Span::NONE,
        })
    }

    /// The arm patterns for variant `vi` on each of `sides`. Up to
    /// [`EAGER_FIELDS_MAX`] fields they bind the whole payload. Past it they
    /// bind nothing, because every field bound at the arm's entry is a load
    /// there, and 401 of them in one region made `llc` quadratic
    /// (PERFORMANCE.md §6.34).
    fn payload(
        &mut self,
        ty: &Ty,
        vi: usize,
        v: &DescVariant,
        sides: &[Expr],
        frame: &mut Frame,
    ) -> Option<Payload> {
        let fields: Vec<(usize, Ty)> = v.fields.iter().map(|f| (f.ty, self.ty_of(f.ty))).collect();
        let lazy = fields.len() > EAGER_FIELDS_MAX && sides.iter().all(Generator::duplicable);
        let mut payload = Payload {
            ty: *ty,
            vi,
            sides: sides.to_vec(),
            fields,
            patterns: Vec::new(),
            bound: None,
        };
        if lazy {
            for _ in sides {
                payload.patterns.push(self.tag_pattern(ty, vi)?);
            }
            return Some(payload);
        }
        let (binds, bound) = self.bind(&payload, 0..payload.fields.len(), frame)?;
        for b in &binds {
            payload.patterns.push(self.variant_pattern(ty, vi, b)?);
        }
        payload.bound = Some(bound);
        Some(payload)
    }

    /// A local for each field in `run` on each side: the bindings for each
    /// side's pattern, and each field's value per side.
    fn bind(
        &self,
        payload: &Payload,
        run: std::ops::Range<usize>,
        frame: &mut Frame,
    ) -> Option<(Vec<Binds>, Vec<Vec<Expr>>)> {
        let mut binds: Vec<Binds> = vec![Vec::new(); payload.sides.len()];
        let mut values: Vec<Vec<Expr>> = Vec::new();
        for fi in run {
            let (_, fty) = payload.fields.get(fi)?;
            let mut each = Vec::new();
            for (side, b) in binds.iter_mut().enumerate() {
                let l = frame.local(side_name(side), fty);
                b.push((fi, l, *fty));
                each.push(self.local_expr(l, fty));
            }
            values.push(each);
        }
        Some((binds, values))
    }

    /// `use_` over the fields in `run`, each a value per side. Fields the arm
    /// didn't bind are read here, by matching each side again and binding just
    /// these: `match a { .V(x0, x1, ..) => match b { .V(y0, y1, ..) => use_(..),
    /// _ => other }, _ => other }`. Both sides are already known to be that
    /// variant, so `other` never runs.
    fn read(
        &mut self,
        payload: &Payload,
        run: std::ops::Range<usize>,
        frame: &mut Frame,
        other: Expr,
        use_: impl FnOnce(&mut Self, &mut Frame, Vec<Vec<Expr>>) -> Option<Expr>,
    ) -> Option<Expr> {
        if let Some(bound) = &payload.bound {
            return use_(self, frame, bound.get(run)?.to_vec());
        }
        let (binds, values) = self.bind(payload, run, frame)?;
        let mut body = use_(self, frame, values)?;
        for (side, b) in payload.sides.iter().zip(&binds).rev() {
            let pat = self.variant_pattern(&payload.ty, payload.vi, b)?;
            let ty = body.ty;
            body = self.match_(
                side.clone(),
                vec![self.arm(pat, body), self.arm(self.wild(&payload.ty), other.clone())],
                ty,
            );
        }
        Some(body)
    }

    /// `match one { .Equal => rest, c => c }`, or `one` alone at the end.
    fn then_compare(&self, one: Expr, rest: Option<Expr>, frame: &mut Frame) -> Option<Expr> {
        let Some(rest) = rest else { return Some(one) };
        let order = self.result_ty(Op::Compare);
        let c = frame.local("c", &order);
        let equal = self.variant_pattern(&order, ORDER_EQUAL, &[])?;
        let bind = Pattern { kind: PatKind::Bind { local: c, sub: None }, ty: order, span: Span::NONE };
        Some(self.match_(
            one,
            vec![self.arm(equal, rest), self.arm(bind, self.local_expr(c, &order))],
            order,
        ))
    }

    /// `one && rest`, or `one` alone at the end.
    fn then_eq(&self, one: Expr, rest: Option<Expr>) -> Expr {
        match rest {
            None => one,
            Some(rest) => Expr::new(
                ExprKind::And { lhs: Box::new(one), rhs: Box::new(rest) },
                self.bool_ty(),
                Span::NONE,
            ),
        }
    }

    // -- rendering ----------------------------------------------------------

    fn show(&mut self, desc: usize, x: Expr, frame: &mut Frame) -> Option<Expr> {
        let str_ty = self.str_ty();
        let descs = std::rc::Rc::clone(&self.descs);
        match descs.get(desc)? {
            Desc::Prim(_) => {
                let ty = self.ty_of(desc);
                Some(self.intrinsic("derivePrimShow", vec![ty], vec![x], str_ty))
            }
            Desc::Unit => Some(self.str_lit("()")),
            Desc::Flags { name, record, fields, prim } => {
                let word = self.word(desc, *prim, fields.len());
                let open = if *record { format!("{name} {{ ") } else { format!("{name}(") };
                let mut parts: Vec<TemplatePart> = vec![TemplatePart::Text(open)];
                for (i, f) in fields.iter().enumerate() {
                    if i > 0 {
                        parts.push(TemplatePart::Text(", ".into()));
                    }
                    if *record {
                        parts.push(TemplatePart::Text(format!("{}: ", f.name)));
                    }
                    let bit = word.bit(x.clone(), i);
                    let shown =
                        self.intrinsic("derivePrimShow", vec![word.bool_ty], vec![bit], str_ty);
                    parts.push(TemplatePart::Hole(shown));
                }
                parts.push(TemplatePart::Text(if *record { " }" } else { ")" }.to_string()));
                Some(self.joined(parts))
            }
            Desc::Struct { name, record, fields } => {
                // A struct with no fields is still written with its delimiters:
                // `Hollow {}` is a value and `Hollow` is a type. `$show` renders
                // the same shape, because a failure report and a hand-called
                // `show` may not disagree about one value.
                if fields.is_empty() {
                    let shown = if *record { format!("{name} {{}}") } else { format!("{name}()") };
                    return Some(self.str_lit(&shown));
                }
                let parts: Vec<(String, usize, usize)> = fields
                    .iter()
                    .enumerate()
                    .map(|(i, f)| (f.name.clone(), i, f.ty))
                    .collect();
                self.show_fields(name, *record, &parts, x, false)
            }
            Desc::Tuple(es) => {
                let parts: Vec<(String, usize, usize)> =
                    es.iter().enumerate().map(|(i, d)| (String::new(), i, *d)).collect();
                self.show_fields("", false, &parts, x, true)
            }
            Desc::Array(elem) => {
                let elem_ty = self.ty_of(*elem);
                let f = self.request(Op::Show, *elem)?;
                let ptr = self.fn_ref(f, vec![elem_ty], str_ty);
                Some(self.intrinsic("deriveArrayShow", vec![elem_ty], vec![x, ptr], str_ty))
            }
            Desc::Option(inner) => {
                let ty = self.ty_of(desc);
                let inner_ty = self.ty_of(*inner);
                let v = frame.local("v", &inner_ty);
                let some = self.variant_pattern(&ty, OPTION_SOME, &[(0, v, inner_ty)])?;
                let shown = self.at(Op::Show, *inner, vec![self.local_expr(v, &inner_ty)])?;
                let body = self.joined(vec![
                    TemplatePart::Text(".Some(".into()),
                    TemplatePart::Hole(shown),
                    TemplatePart::Text(")".into()),
                ]);
                Some(self.match_(
                    x,
                    vec![
                        self.arm(some, body),
                        self.arm(self.wild(&ty), self.str_lit(".None")),
                    ],
                    str_ty,
                ))
            }
            Desc::Enum { variants, .. } => {
                let ty = self.ty_of(desc);
                let mut arms: Vec<Arm> = Vec::new();
                for (vi, v) in variants.iter().enumerate() {
                    let mut binds: Vec<(usize, LocalId, Ty)> = Vec::new();
                    for (fi, f) in v.fields.iter().enumerate() {
                        let fty = self.ty_of(f.ty);
                        binds.push((fi, frame.local("v", &fty), fty));
                    }
                    let pat = self.variant_pattern(&ty, vi, &binds)?;
                    let body = if v.fields.is_empty() {
                        self.str_lit(&format!(".{}", v.name))
                    } else {
                        let mut parts: Vec<TemplatePart> = Vec::new();
                        let open = if v.record {
                            format!(".{} {{ ", v.name)
                        } else {
                            format!(".{}(", v.name)
                        };
                        parts.push(TemplatePart::Text(open));
                        for (k, f) in v.fields.iter().enumerate() {
                            if k > 0 {
                                parts.push(TemplatePart::Text(", ".into()));
                            }
                            if v.record {
                                parts.push(TemplatePart::Text(format!("{}: ", f.name)));
                            }
                            let (_, l, lt) = binds.get(k)?;
                            let shown = self.at(Op::Show, f.ty, vec![self.local_expr(*l, lt)])?;
                            parts.push(TemplatePart::Hole(shown));
                        }
                        parts.push(TemplatePart::Text(
                            if v.record { " }".to_string() } else { ")".to_string() },
                        ));
                        self.joined(parts)
                    };
                    arms.push(self.arm(pat, body));
                }
                Some(self.match_(x, arms, str_ty))
            }
            Desc::Opaque(_) | Desc::Reserved => None,
        }
    }

    /// The rendered pieces of one value, joined by a **single shared call**
    /// rather than by a chain of concatenations written out at the site.
    ///
    /// `middle::lower` turns a `Template` of *k* parts into *k*−1 `str.concat`
    /// calls and, because every intermediate is a fresh block this generator
    /// owns, *k*−1 more drops — and a drop of a `Str` is four CLIF blocks
    /// inline. A derived `show` writes one such template **per variant**, so an
    /// enum of twenty three-field variants pays for sixty joins and ninety
    /// drops in one function body, which is where `enum-heavy`'s native row
    /// went: turning `Show` on took it from 97.9 k lines/s to 20.8 k, with
    /// 1,221 ms of the 1,251 ms difference inside `emit`.
    ///
    /// The chain itself is not wrong, and this does not replace it — it moves
    /// it. [`Generator::joiner`] mints one function per *arity* for the whole
    /// program, whose body is that same template, so the chain is emitted once
    /// per arity instead of once per variant. What is left at the site is one
    /// call with *k* string arguments, and no intermediate for anyone to drop.
    fn joined(&mut self, parts: Vec<TemplatePart>) -> Expr {
        let merged = merge(parts);
        // Two parts are already one join; a call for them would be one more
        // instruction, not one fewer.
        if merged.len() < 3 {
            return self.template_of(merged);
        }
        // Past the chain, `middle::lower` joins a template with one runtime
        // call over a list it builds in place, so at the site it costs what a
        // joiner call would. A joiner of that arity would cost far more: every
        // part a parameter, each given a count to go into the list, and each
        // dropped again by the caller once the call returns.
        if merged.len() > lower::CONCAT_CHAIN_MAX {
            return self.template_of(merged);
        }
        let Some(f) = self.joiner(merged.len()) else { return self.template_of(merged) };
        let args: Vec<Expr> = merged
            .into_iter()
            .map(|p| match p {
                TemplatePart::Text(t) => self.str_lit(&t),
                TemplatePart::Hole(e) => e,
            })
            .collect();
        let ret = self.str_ty();
        self.call(f, args, ret)
    }

    /// The function that concatenates `n` strings, minted on first use and
    /// shared by every generated body in the program.
    ///
    /// It is deliberately unqualified, so it lands in the root codegen unit:
    /// there are as many of these as there are distinct arities in the whole
    /// program — a dozen or so — and giving them a module would mean minting
    /// one per module instead.
    fn joiner(&mut self, n: usize) -> Option<FuncIdx> {
        if let Some(f) = self.joiners.get(&n) {
            return Some(*f);
        }
        let str_ty = self.str_ty();
        if str_ty.is_error() {
            return None;
        }
        let idx = FuncIdx(u32::try_from(self.base + self.funcs.len()).ok()?);
        let mut frame = Frame::new();
        let mut parts = Vec::with_capacity(n);
        for i in 0..n {
            let p = frame.param(&format!("p{i}"), &str_ty);
            parts.push(TemplatePart::Hole(self.local_expr(p, &str_ty)));
        }
        let body = Expr::new(ExprKind::Template { parts }, str_ty, Span::NONE);
        self.funcs.push(Func {
            symbol: format!("derive$join${n}"),
            debug_name: format!("derive$join${n}"),
            params: frame.params,
            locals: frame.locals,
            kind: FuncKind::Body(body),
            ret: str_ty,
            desc: None,
            span: Span::NONE,
        });
        self.joiners.insert(n, idx);
        Some(idx)
    }

    /// A template over parts already merged.
    ///
    /// Every hole in a generated template is already a `Str`, so a backend
    /// needs no per-type rendering to lower one: it is concatenation.
    fn template_of(&self, merged: Vec<TemplatePart>) -> Expr {
        Expr::new(ExprKind::Template { parts: merged }, self.str_ty(), Span::NONE)
    }


    fn show_fields(
        &mut self,
        name: &str,
        record: bool,
        fields: &[(String, usize, usize)],
        x: Expr,
        tuple: bool,
    ) -> Option<Expr> {
        let open = if tuple {
            "(".to_string()
        } else if record {
            format!("{name} {{ ")
        } else {
            format!("{name}(")
        };
        let close = if !tuple && record { " }" } else { ")" };
        let mut parts: Vec<TemplatePart> = vec![TemplatePart::Text(open)];
        for (k, (fname, index, d)) in fields.iter().enumerate() {
            if k > 0 {
                parts.push(TemplatePart::Text(", ".into()));
            }
            if record && !tuple {
                parts.push(TemplatePart::Text(format!("{fname}: ")));
            }
            let fty = self.ty_of(*d);
            let proj = self.project(x.clone(), *index, tuple, fty);
            let shown = self.at(Op::Show, *d, vec![proj])?;
            parts.push(TemplatePart::Hole(shown));
        }
        parts.push(TemplatePart::Text(close.to_string()));
        Some(self.joined(parts))
    }

    // -- JSON ---------------------------------------------------------------

    fn json_lit(&self, name: &str, args: Vec<Expr>) -> Option<Expr> {
        let ty = self.result_ty(Op::ToJson);
        let v = self.env.json_variant(name)?;
        self.enum_lit(&ty, v, args)
    }

    fn json_array_ty(&self) -> Ty {
        Ty::array(self.result_ty(Op::ToJson))
    }

    fn json_array(&self, items: Vec<Expr>) -> Expr {
        Expr::new(ExprKind::Array(items), self.json_array_ty(), Span::NONE)
    }

    /// `[(Str, Json)]`, which is what `.Object` carries.
    fn json_object_ty(&self) -> Ty {
        Ty::array(Ty::tuple([self.str_ty(), self.result_ty(Op::ToJson)]))
    }

    fn json_members(&self, members: Vec<(String, Expr)>) -> Expr {
        let pair_ty = Ty::tuple([self.str_ty(), self.result_ty(Op::ToJson)]);
        let items: Vec<Expr> = members
            .into_iter()
            .map(|(k, v)| {
                Expr::new(
                    ExprKind::Tuple(vec![self.str_lit(&k), v]),
                    pair_ty,
                    Span::NONE,
                )
            })
            .collect();
        Expr::new(ExprKind::Array(items), self.json_object_ty(), Span::NONE)
    }

    fn json_of(&mut self, desc: usize, x: Expr, frame: &mut Frame) -> Option<Expr> {
        let json = self.result_ty(Op::ToJson);
        let descs = std::rc::Rc::clone(&self.descs);
        match descs.get(desc)? {
            Desc::Prim(_) => {
                let ty = self.ty_of(desc);
                Some(self.intrinsic("derivePrimJson", vec![ty], vec![x], json))
            }
            Desc::Unit => self.json_lit("Null", Vec::new()),
            Desc::Flags { record, fields, prim, .. } => {
                let word = self.word(desc, *prim, fields.len());
                let mut items: Vec<(String, Expr)> = Vec::new();
                for (i, f) in fields.iter().enumerate() {
                    let bit = word.bit(x.clone(), i);
                    let one = self.intrinsic("derivePrimJson", vec![word.bool_ty], vec![bit], json);
                    items.push((f.name.clone(), one));
                }
                if *record {
                    let obj = self.json_members(items);
                    self.json_lit("Object", vec![obj])
                } else {
                    let arr = self.json_array(items.into_iter().map(|(_, v)| v).collect());
                    self.json_lit("Array", vec![arr])
                }
            }
            Desc::Struct { record, fields, .. } => {
                let mut members: Vec<(String, Expr)> = Vec::new();
                let mut items: Vec<Expr> = Vec::new();
                for (i, f) in fields.iter().enumerate() {
                    let fty = self.ty_of(f.ty);
                    let proj = self.project(x.clone(), i, false, fty);
                    let v = self.at(Op::ToJson, f.ty, vec![proj])?;
                    if *record {
                        members.push((f.name.clone(), v));
                    } else {
                        items.push(v);
                    }
                }
                if *record {
                    let obj = self.json_members(members);
                    self.json_lit("Object", vec![obj])
                } else {
                    let arr = self.json_array(items);
                    self.json_lit("Array", vec![arr])
                }
            }
            Desc::Tuple(es) => {
                let mut items: Vec<Expr> = Vec::new();
                for (i, d) in es.iter().enumerate() {
                    let fty = self.ty_of(*d);
                    let proj = self.project(x.clone(), i, true, fty);
                    items.push(self.at(Op::ToJson, *d, vec![proj])?);
                }
                let arr = self.json_array(items);
                self.json_lit("Array", vec![arr])
            }
            Desc::Array(elem) => {
                let elem_ty = self.ty_of(*elem);
                let f = self.request(Op::ToJson, *elem)?;
                let ptr = self.fn_ref(f, vec![elem_ty], json);
                let mapped = self.intrinsic(
                    "deriveArrayJson",
                    vec![elem_ty],
                    vec![x, ptr],
                    self.json_array_ty(),
                );
                self.json_lit("Array", vec![mapped])
            }
            Desc::Option(inner) => {
                let ty = self.ty_of(desc);
                let inner_ty = self.ty_of(*inner);
                let v = frame.local("v", &inner_ty);
                let some = self.variant_pattern(&ty, OPTION_SOME, &[(0, v, inner_ty)])?;
                let body = self.at(Op::ToJson, *inner, vec![self.local_expr(v, &inner_ty)])?;
                let null = self.json_lit("Null", Vec::new())?;
                Some(self.match_(
                    x,
                    vec![self.arm(some, body), self.arm(self.wild(&ty), null)],
                    json,
                ))
            }
            Desc::Enum { variants, .. } => {
                let ty = self.ty_of(desc);
                let mut arms: Vec<Arm> = Vec::new();
                for (vi, v) in variants.iter().enumerate() {
                    let mut binds: Vec<(usize, LocalId, Ty)> = Vec::new();
                    for (fi, f) in v.fields.iter().enumerate() {
                        let fty = self.ty_of(f.ty);
                        binds.push((fi, frame.local("v", &fty), fty));
                    }
                    let pat = self.variant_pattern(&ty, vi, &binds)?;
                    // Externally tagged: a variant with no payload is its own
                    // name, and one with a payload is a one-member object.
                    let body = if v.fields.is_empty() {
                        let name = self.str_lit(&v.name);
                        self.json_lit("Str", vec![name])?
                    } else {
                        let mut members: Vec<(String, Expr)> = Vec::new();
                        let mut items: Vec<Expr> = Vec::new();
                        for (k, f) in v.fields.iter().enumerate() {
                            let (_, l, lt) = binds.get(k)?;
                            let one =
                                self.at(Op::ToJson, f.ty, vec![self.local_expr(*l, lt)])?;
                            if v.record {
                                members.push((f.name.clone(), one));
                            } else {
                                items.push(one);
                            }
                        }
                        let payload = if v.record {
                            let obj = self.json_members(members);
                            self.json_lit("Object", vec![obj])?
                        } else {
                            let arr = self.json_array(items);
                            self.json_lit("Array", vec![arr])?
                        };
                        let wrapper = self.json_members(vec![(v.name.clone(), payload)]);
                        self.json_lit("Object", vec![wrapper])?
                    };
                    arms.push(self.arm(pat, body));
                }
                Some(self.match_(x, arms, json))
            }
            Desc::Opaque(_) | Desc::Reserved => None,
        }
    }

    // -- decoding -----------------------------------------------------------
    //
    // `runtime.js`'s `$json_into`, one function per shape. Every decoder is
    // `(j: Json, p: Str): Result<T, DecodeError>`, where `p` is the path `j`
    // sits at, and a failure is the `DecodeError` the walk there throws: the
    // same path and the same words.

    /// `Result<T, DecodeError>`.
    fn decoded(&self, ty: &Ty) -> Option<Ty> {
        let d = self.env.decoding?;
        Some(Ty::con(d.result, [*ty, d.error]))
    }

    fn json_ty(&self) -> Ty {
        self.env.decoding.map(|d| d.json).unwrap_or(Ty::ERROR)
    }

    fn error_ty(&self) -> Ty {
        self.env.decoding.map(|d| d.error).unwrap_or(Ty::ERROR)
    }

    fn option_of(&self, ty: Ty) -> Option<Ty> {
        Some(Ty::con(self.env.decoding?.option, [ty]))
    }

    fn prim_ty(&self, p: Prim) -> Ty {
        self.env.prim_of.get(&p).cloned().unwrap_or(Ty::ERROR)
    }

    fn int_lit(&self, n: usize) -> Expr {
        let v = typed::Magnitude::new(n as u128);
        Expr::new(ExprKind::Int(v, false), self.prim_ty(Prim::I64), Span::NONE)
    }

    fn plus_one(&self, i: Expr) -> Expr {
        let args = vec![i, self.int_lit(1)];
        let int = self.prim_ty(Prim::I64);
        Expr::new(ExprKind::Prim { op: PrimOp::Add, prim: Prim::I64, args }, int, Span::NONE)
    }

    fn ok(&self, ty: &Ty, v: Expr) -> Option<Expr> {
        self.enum_lit(&self.decoded(ty)?, RESULT_OK, vec![v])
    }

    fn fail(&self, ty: &Ty, e: Expr) -> Option<Expr> {
        self.enum_lit(&self.decoded(ty)?, RESULT_ERR, vec![e])
    }

    /// `.Err(.WrongType { path: p, wanted, found: found(j) })` at `ty`.
    fn wrong(&mut self, ty: &Ty, p: Expr, wanted: &str, j: Expr) -> Option<Expr> {
        let f = self.helper(Helper::Wrong)?;
        let e = self.call(f, vec![p, self.str_lit(wanted), j], self.error_ty());
        self.fail(ty, e)
    }

    /// `.Err(.UnknownVariant { path: p, tag })` at `ty`.
    fn unknown(&self, ty: &Ty, p: Expr, tag: Expr) -> Option<Expr> {
        let e = self.enum_lit(&self.error_ty(), DECODE_UNKNOWN_VARIANT, vec![p, tag])?;
        self.fail(ty, e)
    }

    /// `p` with a literal suffix: `$.name`, `$[0]`.
    fn path(&self, p: Expr, suffix: &str) -> Expr {
        self.template_of(vec![TemplatePart::Hole(p), TemplatePart::Text(suffix.to_string())])
    }

    /// `x?` on a `Result`, which leaves the decoder with the same error.
    fn tried(&self, x: Expr, payload: Ty) -> Expr {
        let kind = ExprKind::Try { base: Box::new(x), kind: typed::OptionOrResult::Result };
        Expr::new(kind, payload, Span::NONE)
    }

    fn binding(&self, local: LocalId, ty: Ty) -> Pattern {
        Pattern { kind: PatKind::Bind { local, sub: None }, ty, span: Span::NONE }
    }

    fn let_(&self, local: LocalId, ty: Ty, value: Expr) -> typed::Stmt {
        typed::Stmt::Let { pattern: self.binding(local, ty), value, span: Span::NONE }
    }

    fn block(&self, stmts: Vec<typed::Stmt>, tail: Expr) -> Expr {
        let ty = tail.ty;
        Expr::new(ExprKind::Block { stmts, tail: Some(Box::new(tail)) }, ty, Span::NONE)
    }

    /// `xs[i]`, which is an `Option`.
    fn index(&self, xs: Expr, i: Expr, elem: Ty) -> Option<Expr> {
        let kind = ExprKind::Index { base: Box::new(xs), index: Box::new(i), elem };
        Some(Expr::new(kind, self.option_of(elem)?, Span::NONE))
    }

    /// `match j { .V(x) => then, _ => otherwise }` over a `Json`, binding the
    /// payload to `x` where there is one.
    fn on_json(
        &self,
        j: Expr,
        variant: &str,
        x: Option<(LocalId, Ty)>,
        then: Expr,
        otherwise: Expr,
    ) -> Option<Expr> {
        let json = self.json_ty();
        let vi = self.env.json_variant(variant)?;
        let pat = match x {
            Some((l, t)) => self.variant_pattern(&json, vi, &[(0, l, t)])?,
            None => self.tag_pattern(&json, vi)?,
        };
        let ty = then.ty;
        Some(self.match_(j, vec![self.arm(pat, then), self.arm(self.wild(&json), otherwise)], ty))
    }

    /// `let v = decode(j, p)?;`, and `v`.
    fn decode_into(
        &mut self,
        desc: usize,
        j: Expr,
        p: Expr,
        stmts: &mut Vec<typed::Stmt>,
        frame: &mut Frame,
    ) -> Option<Expr> {
        let ty = self.ty_of(desc);
        let f = self.request(Op::FromJson, desc)?;
        let call = self.call(f, vec![j, p], self.decoded(&ty)?);
        let v = frame.local("v", &ty);
        stmts.push(self.let_(v, ty, self.tried(call, ty)));
        Some(self.local_expr(v, &ty))
    }

    /// The body of one decoder.
    fn decode(&mut self, desc: usize, j: LocalId, p: LocalId, frame: &mut Frame) -> Option<Expr> {
        let ty = self.ty_of(desc);
        let je = self.local_expr(j, &self.json_ty());
        let pe = self.local_expr(p, &self.str_ty());
        let descs = std::rc::Rc::clone(&self.descs);
        match descs.get(desc)? {
            Desc::Prim(prim) => self.decode_prim(*prim, &ty, je, pe, frame),
            Desc::Unit => {
                let unit = Expr::new(ExprKind::Unit, ty, Span::NONE);
                let then = self.ok(&ty, unit)?;
                let otherwise = self.wrong(&ty, pe, "null", je.clone())?;
                self.on_json(je, "Null", None, then, otherwise)
            }
            Desc::Option(inner) => {
                // `null` is absent, and anything else is the payload.
                let none = self.enum_lit(&ty, OPTION_NONE, Vec::new())?;
                let absent = self.ok(&ty, none)?;
                let mut stmts = Vec::new();
                let v = self.decode_into(*inner, je.clone(), pe, &mut stmts, frame)?;
                let some = self.enum_lit(&ty, OPTION_SOME, vec![v])?;
                let present = self.block(stmts, self.ok(&ty, some)?);
                self.on_json(je, "Null", None, absent, present)
            }
            Desc::Struct { record: true, fields, .. } => {
                let members: Vec<(String, usize)> =
                    fields.iter().map(|f| (f.name.clone(), f.ty)).collect();
                self.decode_object(&ty, je, pe, &members, Build::Struct(ty), frame)
            }
            Desc::Struct { record: false, fields, .. } => {
                let items: Vec<usize> = fields.iter().map(|f| f.ty).collect();
                self.decode_array(&ty, je, pe, &items, Build::Struct(ty), frame)
            }
            Desc::Flags { record, fields, prim, .. } => {
                let build = Build::Flags(self.word(desc, *prim, fields.len()));
                if *record {
                    let members: Vec<(String, usize)> =
                        fields.iter().map(|f| (f.name.clone(), f.ty)).collect();
                    self.decode_object(&ty, je, pe, &members, build, frame)
                } else {
                    let items: Vec<usize> = fields.iter().map(|f| f.ty).collect();
                    self.decode_array(&ty, je, pe, &items, build, frame)
                }
            }
            Desc::Tuple(es) => self.decode_array(&ty, je, pe, es, Build::Tuple(ty), frame),
            Desc::Array(elem) => self.decode_list(&ty, *elem, je, pe, frame),
            Desc::Enum { variants, .. } => self.decode_enum(&ty, variants, je, pe, frame),
            Desc::Opaque(_) | Desc::Reserved => None,
        }
    }

    /// A primitive: the one `Json` variant it is written as and, for a number,
    /// whether the type holds it.
    fn decode_prim(
        &mut self,
        prim: Prim,
        ty: &Ty,
        je: Expr,
        pe: Expr,
        frame: &mut Frame,
    ) -> Option<Expr> {
        let (variant, payload, wanted) = match prim {
            Prim::Bool => ("Bool", Prim::Bool, "a boolean"),
            Prim::Str | Prim::Template => ("Str", Prim::Str, "a string"),
            Prim::Char => ("Str", Prim::Str, "a one-character string"),
            p if p.is_float() => ("Num", Prim::F64, "a number"),
            _ => ("Num", Prim::F64, "an integer"),
        };
        let payload_ty = self.prim_ty(payload);
        let x = frame.local("x", &payload_ty);
        let xe = self.local_expr(x, &payload_ty);
        let otherwise = self.wrong(ty, pe, wanted, je.clone())?;
        let ret = otherwise.ty;
        let then = match prim {
            Prim::Bool | Prim::Str | Prim::Template | Prim::F64 => self.ok(ty, xe)?,
            Prim::F32 => {
                let narrowed = self.rt("number.F64.wrapToF32", vec![xe], *ty);
                self.ok(ty, narrowed)?
            }
            // One Unicode scalar value, which is one step of `str.length`.
            Prim::Char => {
                let int = self.prim_ty(Prim::I64);
                let len = self.rt("str.length", vec![xe.clone()], int);
                let one = self.prim_test(PrimOp::Eq, Prim::I64, len, self.int_lit(1));
                let opt = self.option_of(*ty)?;
                let args = vec![xe, self.int_lit(0)];
                let first = self.rt("str.charAt", args, opt);
                let c = frame.local("c", ty);
                let some = self.variant_pattern(&opt, OPTION_SOME, &[(0, c, *ty)])?;
                let got = self.ok(ty, self.local_expr(c, ty))?;
                let arms = vec![self.arm(some, got), self.arm(self.wild(&opt), otherwise.clone())];
                let read = self.match_(first, arms, ret);
                self.choose(one, read, otherwise.clone(), ret)
            }
            // A whole number the type holds: inside its bounds, and rounded
            // into the type and back, still the number the document wrote.
            // Each bound is zero or a power of two, so it is exact as a double.
            p => {
                let n = frame.local("n", ty);
                let into = format!("number.F64.wrapTo{}", p.name());
                let rounded = self.rt(&into, vec![xe.clone()], *ty);
                let float = |v: f64| Expr::new(ExprKind::Float(v), payload_ty, Span::NONE);
                let out = format!("number.{}.toF64", p.name());
                let back = self.rt(&out, vec![self.local_expr(n, ty)], payload_ty);
                let bits = i32::try_from(p.bits()).unwrap_or(64);
                let (low, past) = if p.is_signed() {
                    (-(2f64.powi(bits - 1)), 2f64.powi(bits - 1))
                } else {
                    (0.0, 2f64.powi(bits))
                };
                let above = self.prim_test(PrimOp::Ge, Prim::F64, xe.clone(), float(low));
                let below = self.prim_test(PrimOp::Lt, Prim::F64, xe.clone(), float(past));
                let whole = self.prim_test(PrimOp::Eq, Prim::F64, back, xe);
                let both = |a: Expr, b: Expr| {
                    let kind = ExprKind::And { lhs: Box::new(a), rhs: Box::new(b) };
                    Expr::new(kind, self.bool_ty(), Span::NONE)
                };
                let fits = both(both(above, below), whole);
                let got = self.ok(ty, self.local_expr(n, ty))?;
                let checked = self.choose(fits, got, otherwise.clone(), ret);
                self.block(vec![self.let_(n, *ty, rounded)], checked)
            }
        };
        self.on_json(je, variant, Some((x, payload_ty)), then, otherwise)
    }

    /// A value built from its decoded parts.
    fn build(&self, build: Build, vals: Vec<Expr>) -> Option<Expr> {
        match build {
            Build::Struct(ty) => {
                let con = ty.head()?;
                let targs = match ty.kind() {
                    TyKind::Con(_, a) => a.to_vec(),
                    _ => Vec::new(),
                };
                Some(Expr::new(ExprKind::StructLit { con, targs, fields: vals }, ty, Span::NONE))
            }
            Build::Tuple(ty) => Some(Expr::new(ExprKind::Tuple(vals), ty, Span::NONE)),
            Build::Flags(word) => Some(word.pack(vals)),
            Build::Variant(ty, vi) => self.enum_lit(&ty, vi, vals),
        }
    }

    /// An object with these members, in any order, and any others ignored.
    fn decode_object(
        &mut self,
        ty: &Ty,
        je: Expr,
        pe: Expr,
        members: &[(String, usize)],
        build: Build,
        frame: &mut Frame,
    ) -> Option<Expr> {
        let json = self.json_ty();
        let entries_ty = Ty::array(Ty::tuple([self.str_ty(), json]));
        let es = frame.local("es", &entries_ty);
        let member = self.helper(Helper::Member)?;
        let found_ty = self.decoded(&json)?;
        let mut stmts = Vec::new();
        let mut vals = Vec::new();
        for (name, d) in members {
            let args = vec![
                self.local_expr(es, &entries_ty),
                self.str_lit(name),
                pe.clone(),
                self.int_lit(0),
            ];
            let found = self.call(member, args, found_ty);
            let m = frame.local("m", &json);
            stmts.push(self.let_(m, json, self.tried(found, json)));
            let at = self.path(pe.clone(), &format!(".{name}"));
            vals.push(self.decode_into(*d, self.local_expr(m, &json), at, &mut stmts, frame)?);
        }
        let then = self.block(stmts, self.ok(ty, self.build(build, vals)?)?);
        let otherwise = self.wrong(ty, pe, "an object", je.clone())?;
        self.on_json(je, "Object", Some((es, entries_ty)), then, otherwise)
    }

    /// An array of exactly one element per item, each its own type.
    fn decode_array(
        &mut self,
        ty: &Ty,
        je: Expr,
        pe: Expr,
        items: &[usize],
        build: Build,
        frame: &mut Frame,
    ) -> Option<Expr> {
        let json = self.json_ty();
        let xs_ty = Ty::array(json);
        let xs = frame.local("xs", &xs_ty);
        let mut elems = Vec::new();
        let mut stmts = Vec::new();
        let mut vals = Vec::new();
        for (i, d) in items.iter().enumerate() {
            let e = frame.local("e", &json);
            elems.push(self.binding(e, json));
            let at = self.path(pe.clone(), &format!("[{i}]"));
            vals.push(self.decode_into(*d, self.local_expr(e, &json), at, &mut stmts, frame)?);
        }
        let exact = Pattern {
            kind: PatKind::Array { elems, rest: typed::ArrayRest::None },
            ty: xs_ty,
            span: Span::NONE,
        };
        let got = self.block(stmts, self.ok(ty, self.build(build, vals)?)?);
        let wanted = format!("an array of length {}", items.len());
        let short = self.wrong(ty, pe.clone(), &wanted, je.clone())?;
        let then = self.match_(
            self.local_expr(xs, &xs_ty),
            vec![self.arm(exact, got), self.arm(self.wild(&xs_ty), short)],
            self.decoded(ty)?,
        );
        let otherwise = self.wrong(ty, pe, "an array", je.clone())?;
        self.on_json(je, "Array", Some((xs, xs_ty)), then, otherwise)
    }

    /// `[T]`: an array, each element at `p[i]`.
    fn decode_list(
        &mut self,
        ty: &Ty,
        elem: usize,
        je: Expr,
        pe: Expr,
        frame: &mut Frame,
    ) -> Option<Expr> {
        let xs_ty = Ty::array(self.json_ty());
        let xs = frame.local("xs", &xs_ty);
        let each = self.decode_each(ty, elem)?;
        let empty = Expr::new(ExprKind::Array(Vec::new()), *ty, Span::NONE);
        let args = vec![self.local_expr(xs, &xs_ty), pe.clone(), self.int_lit(0), empty];
        let then = self.call(each, args, self.decoded(ty)?);
        let otherwise = self.wrong(ty, pe, "an array", je.clone())?;
        self.on_json(je, "Array", Some((xs, xs_ty)), then, otherwise)
    }

    /// `each(xs: [Json], p: Str, i: Int, acc: [T]): Result<[T], DecodeError>`,
    /// which decodes from element `i` on. It is the loop `middle::tail_calls`
    /// makes of a function that tail-calls itself, written out because that
    /// pass has already run.
    fn decode_each(&mut self, ty: &Ty, elem: usize) -> Option<FuncIdx> {
        let json = self.json_ty();
        let xs_ty = Ty::array(json);
        let int = self.prim_ty(Prim::I64);
        let str_ty = self.str_ty();
        let ret = self.decoded(ty)?;
        let mut frame = Frame::new();
        let xs = frame.param("xs", &xs_ty);
        let p = frame.param("p", &str_ty);
        let i = frame.param("i", &int);
        let acc = frame.param("acc", ty);
        let xse = self.local_expr(xs, &xs_ty);
        let pe = self.local_expr(p, &str_ty);
        let ie = self.local_expr(i, &int);
        let acce = self.local_expr(acc, ty);
        let at = self.index(xse.clone(), ie.clone(), json)?;
        let opt = at.ty;
        let e = frame.local("e", &json);
        let some = self.variant_pattern(&opt, OPTION_SOME, &[(0, e, json)])?;
        let shown = self.intrinsic("derivePrimShow", vec![int], vec![ie.clone()], str_ty);
        let path = self.template_of(vec![
            TemplatePart::Hole(pe.clone()),
            TemplatePart::Text("[".into()),
            TemplatePart::Hole(shown),
            TemplatePart::Text("]".into()),
        ]);
        let mut stmts = Vec::new();
        let v = self.decode_into(elem, self.local_expr(e, &json), path, &mut stmts, &mut frame)?;
        // `push` is declared with a context, which is dropped by position before
        // the runtime sees it (`runtime_table::Arg::Dropped`), so `()` stands in.
        let unit = Expr::new(ExprKind::Unit, Ty::UNIT, Span::NONE);
        let pushed = self.rt("list.push", vec![acce.clone(), unit, v], *ty);
        let args = vec![xse, pe, self.plus_one(ie), pushed];
        let again = Expr::new(ExprKind::Continue { func: None, entry: 0, args }, ret, Span::NONE);
        let step = self.block(stmts, again);
        let done = self.ok(ty, acce)?;
        let arms = vec![self.arm(some, step), self.arm(self.wild(&opt), done)];
        let body = self.match_(at, arms, ret);
        let body = Expr::new(ExprKind::Loop { entries: vec![body] }, ret, Span::NONE);
        let name = format!("$derive$decs${}", short_hash(&self.shape_key(Op::FromJson, elem)));
        Some(self.mint(&name, frame, body, ret))
    }

    /// An enum, externally tagged: a variant with no fields is its name, and
    /// one with fields is `{"Name": payload}`.
    fn decode_enum(
        &mut self,
        ty: &Ty,
        variants: &[DescVariant],
        je: Expr,
        pe: Expr,
        frame: &mut Frame,
    ) -> Option<Expr> {
        let str_ty = self.str_ty();
        let json = self.json_ty();
        let ret = self.decoded(ty)?;
        let named_pat = |name: &str| Pattern {
            kind: PatKind::Str(name.to_string()),
            ty: str_ty,
            span: Span::NONE,
        };

        // A bare name.
        let s = frame.local("s", &str_ty);
        let se = self.local_expr(s, &str_ty);
        let mut arms = Vec::new();
        for (vi, v) in variants.iter().enumerate() {
            let body = if v.fields.is_empty() {
                self.ok(ty, self.enum_lit(ty, vi, Vec::new())?)?
            } else {
                let wanted = format!("an object naming {}'s fields", v.name);
                self.wrong(ty, pe.clone(), &wanted, je.clone())?
            };
            arms.push(self.arm(named_pat(&v.name), body));
        }
        arms.push(self.arm(self.wild(&str_ty), self.unknown(ty, pe.clone(), se.clone())?));
        let bare = self.match_(se, arms, ret);

        // `{"Name": payload}`.
        let pair_ty = Ty::tuple([str_ty, json]);
        let entries_ty = Ty::array(pair_ty);
        let es = frame.local("es", &entries_ty);
        let pair = frame.local("pair", &pair_ty);
        let name = frame.local("name", &str_ty);
        let inner = frame.local("inner", &json);
        let pair_e = self.local_expr(pair, &pair_ty);
        let name_e = self.local_expr(name, &str_ty);
        let inner_e = self.local_expr(inner, &json);
        let mut arms = Vec::new();
        for (vi, v) in variants.iter().enumerate() {
            let body = if v.fields.is_empty() {
                self.wrong(ty, pe.clone(), &format!("the string {}", v.name), je.clone())?
            } else {
                let q = frame.local("q", &str_ty);
                let qe = self.local_expr(q, &str_ty);
                let build = Build::Variant(*ty, vi);
                let payload = if v.record {
                    let members: Vec<(String, usize)> =
                        v.fields.iter().map(|f| (f.name.clone(), f.ty)).collect();
                    self.decode_object(ty, inner_e.clone(), qe, &members, build, frame)?
                } else {
                    let items: Vec<usize> = v.fields.iter().map(|f| f.ty).collect();
                    self.decode_array(ty, inner_e.clone(), qe, &items, build, frame)?
                };
                let at = self.path(pe.clone(), &format!(".{}", v.name));
                self.block(vec![self.let_(q, str_ty, at)], payload)
            };
            arms.push(self.arm(named_pat(&v.name), body));
        }
        arms.push(self.arm(self.wild(&str_ty), self.unknown(ty, pe.clone(), name_e.clone())?));
        let by_name = self.match_(name_e, arms, ret);
        let one = self.block(
            vec![
                self.let_(name, str_ty, self.project(pair_e.clone(), 0, true, str_ty)),
                self.let_(inner, json, self.project(pair_e, 1, true, json)),
            ],
            by_name,
        );
        let single = Pattern {
            kind: PatKind::Array {
                elems: vec![self.binding(pair, pair_ty)],
                rest: typed::ArrayRest::None,
            },
            ty: entries_ty,
            span: Span::NONE,
        };
        let wanted = "an object with one member, naming the variant";
        let many = self.wrong(ty, pe.clone(), wanted, je.clone())?;
        let tagged = self.match_(
            self.local_expr(es, &entries_ty),
            vec![self.arm(single, one), self.arm(self.wild(&entries_ty), many)],
            ret,
        );

        let neither = self.wrong(ty, pe, "a string or an object", je.clone())?;
        let object = self.on_json(je.clone(), "Object", Some((es, entries_ty)), tagged, neither)?;
        self.on_json(je, "Str", Some((s, str_ty)), bare, object)
    }

    /// A function every decoder shares, minted on first use.
    fn helper(&mut self, which: Helper) -> Option<FuncIdx> {
        if let Some(f) = self.helpers.get(&which) {
            return Some(*f);
        }
        let json = self.json_ty();
        let str_ty = self.str_ty();
        let error = self.error_ty();
        let mut frame = Frame::new();
        let (name, body) = match which {
            Helper::Found => {
                let j = frame.param("j", &json);
                let mut arms = Vec::new();
                for (variant, said) in [
                    ("Null", "null"),
                    ("Bool", "a boolean"),
                    ("Num", "a number"),
                    ("Str", "a string"),
                    ("Array", "an array"),
                ] {
                    let pat = self.tag_pattern(&json, self.env.json_variant(variant)?)?;
                    arms.push(self.arm(pat, self.str_lit(said)));
                }
                arms.push(self.arm(self.wild(&json), self.str_lit("an object")));
                ("$derive$found", self.match_(self.local_expr(j, &json), arms, str_ty))
            }
            Helper::Wrong => {
                let found = self.helper(Helper::Found)?;
                let p = frame.param("p", &str_ty);
                let wanted = frame.param("wanted", &str_ty);
                let j = frame.param("j", &json);
                let said = self.call(found, vec![self.local_expr(j, &json)], str_ty);
                let args =
                    vec![self.local_expr(p, &str_ty), self.local_expr(wanted, &str_ty), said];
                ("$derive$wrong", self.enum_lit(&error, DECODE_WRONG_TYPE, args)?)
            }
            Helper::Member => {
                let ret = self.decoded(&json)?;
                let int = self.prim_ty(Prim::I64);
                let pair_ty = Ty::tuple([str_ty, json]);
                let entries_ty = Ty::array(pair_ty);
                let es = frame.param("es", &entries_ty);
                let key = frame.param("key", &str_ty);
                let p = frame.param("p", &str_ty);
                let i = frame.param("i", &int);
                let ese = self.local_expr(es, &entries_ty);
                let keye = self.local_expr(key, &str_ty);
                let pe = self.local_expr(p, &str_ty);
                let ie = self.local_expr(i, &int);
                let at = self.index(ese.clone(), ie.clone(), pair_ty)?;
                let opt = at.ty;
                let pair = frame.local("pair", &pair_ty);
                let pair_e = self.local_expr(pair, &pair_ty);
                let some = self.variant_pattern(&opt, OPTION_SOME, &[(0, pair, pair_ty)])?;
                let k = self.project(pair_e.clone(), 0, true, str_ty);
                let same = self.prim_test(PrimOp::Eq, Prim::Str, k, keye.clone());
                let here = self.ok(&json, self.project(pair_e, 1, true, json))?;
                let args = vec![ese, keye.clone(), pe.clone(), self.plus_one(ie)];
                let again =
                    Expr::new(ExprKind::Continue { func: None, entry: 0, args }, ret, Span::NONE);
                let step = self.choose(same, here, again, ret);
                let at_path = self.template_of(vec![
                    TemplatePart::Hole(pe),
                    TemplatePart::Text(".".into()),
                    TemplatePart::Hole(keye),
                ]);
                let missing = self.enum_lit(&error, DECODE_MISSING, vec![at_path])?;
                let none = self.fail(&json, missing)?;
                let arms = vec![self.arm(some, step), self.arm(self.wild(&opt), none)];
                let body = self.match_(at, arms, ret);
                let looped = Expr::new(ExprKind::Loop { entries: vec![body] }, ret, Span::NONE);
                ("$derive$member", looped)
            }
        };
        let ret = body.ty;
        let f = self.mint(name, frame, body, ret);
        self.helpers.insert(which, f);
        Some(f)
    }

    /// A call to the runtime operation `key`, through a function of its own.
    ///
    /// A function rather than an `ExprKind::Intrinsic`, because both backends
    /// build a `number.*` conversion as a function's whole body and nowhere
    /// else, and because `middle::rc` reads how `list.push` treats its
    /// receiver off the function.
    fn rt(&mut self, key: &str, args: Vec<Expr>, ret: Ty) -> Expr {
        let params: Vec<Ty> = args.iter().map(|a| a.ty).collect();
        let f = self.runtime(key, params, ret);
        self.call(f, args, ret)
    }

    fn runtime(&mut self, key: &str, params: Vec<Ty>, ret: Ty) -> FuncIdx {
        let slot = (key.to_string(), params);
        if let Some(f) = self.runtime.get(&slot) {
            return *f;
        }
        let mut frame = Frame::new();
        for (i, t) in slot.1.iter().enumerate() {
            frame.param(&format!("p{i}"), t);
        }
        let idx = FuncIdx(u32::try_from(self.base + self.funcs.len()).unwrap_or(u32::MAX));
        let symbol = format!("$derive$rt${}${}", key.replace('.', "$"), self.runtime.len());
        self.funcs.push(Func {
            symbol: symbol.clone(),
            debug_name: symbol,
            params: frame.params,
            locals: frame.locals,
            kind: FuncKind::Intrinsic(key.to_string()),
            ret,
            desc: None,
            span: Span::NONE,
        });
        self.runtime.insert(slot, idx);
        idx
    }

    /// Adds a finished function. Unqualified, so it lands in the root codegen
    /// unit with the joiners.
    fn mint(&mut self, name: &str, frame: Frame, body: Expr, ret: Ty) -> FuncIdx {
        let idx = FuncIdx(u32::try_from(self.base + self.funcs.len()).unwrap_or(u32::MAX));
        self.funcs.push(Func {
            symbol: name.to_string(),
            debug_name: name.to_string(),
            params: frame.params,
            locals: frame.locals,
            kind: FuncKind::Body(body),
            ret,
            desc: None,
            span: Span::NONE,
        });
        idx
    }

    // -- hashing ------------------------------------------------------------

    fn hash_ty(&self) -> Ty {
        self.result_ty(Op::Hash)
    }

    fn hash_int(&self, v: u128) -> Expr {
        Expr::new(ExprKind::Int(typed::Magnitude::new(v), false), self.hash_ty(), Span::NONE)
    }

    /// `$mix(h, n)` on a number the shape itself supplies — a field count or a
    /// tag. Goes through the same primitive intrinsic as a hashed value, at the
    /// accumulator's own type.
    fn mix(&self, h: Expr, n: u128) -> Expr {
        let ty = self.hash_ty();
        self.intrinsic("derivePrimHash", vec![ty], vec![h, self.hash_int(n)], ty)
    }

    fn hash(&mut self, desc: usize, h: Expr, x: Expr, frame: &mut Frame) -> Option<Expr> {
        let acc = self.hash_ty();
        let descs = std::rc::Rc::clone(&self.descs);
        match descs.get(desc)? {
            Desc::Prim(_) => {
                let ty = self.ty_of(desc);
                Some(self.intrinsic("derivePrimHash", vec![ty], vec![h, x], acc))
            }
            // The word, hashed as a primitive of its width hashes.
            Desc::Flags { prim, .. } => {
                let ty = self.env.prim_of.get(prim).cloned().unwrap_or(Ty::ERROR);
                Some(self.intrinsic("derivePrimHash", vec![ty], vec![h, x], acc))
            }
            // `$hashInto` sees `()` as the number zero, and this is what keeps
            // the two backends' `hash()` the same number.
            Desc::Unit => Some(self.mix(h, 0)),
            Desc::Struct { fields, .. } => {
                let parts: Vec<(usize, usize)> =
                    fields.iter().enumerate().map(|(i, f)| (i, f.ty)).collect();
                self.hash_fields(&parts, h, x, false)
            }
            Desc::Tuple(es) => {
                let parts: Vec<(usize, usize)> =
                    es.iter().enumerate().map(|(i, d)| (i, *d)).collect();
                self.hash_fields(&parts, h, x, true)
            }
            Desc::Array(elem) => {
                let elem_ty = self.ty_of(*elem);
                let f = self.request(Op::Hash, *elem)?;
                let ptr = self.fn_ref(f, vec![acc, elem_ty], acc);
                Some(self.intrinsic("deriveArrayHash", vec![elem_ty], vec![h, x, ptr], acc))
            }
            Desc::Option(inner) => {
                let ty = self.ty_of(desc);
                let inner_ty = self.ty_of(*inner);
                let v = frame.local("v", &inner_ty);
                let some = self.variant_pattern(&ty, OPTION_SOME, &[(0, v, inner_ty)])?;
                let body =
                    self.at_hash(*inner, h.clone(), self.local_expr(v, &inner_ty))?;
                Some(self.match_(
                    x,
                    vec![self.arm(some, body), self.arm(self.wild(&ty), self.mix(h, 0))],
                    acc,
                ))
            }
            Desc::Enum { variants, .. } => {
                let ty = self.ty_of(desc);
                let payloads = variants.iter().any(|v| !v.fields.is_empty());
                let mut arms: Vec<Arm> = Vec::new();
                for (vi, v) in variants.iter().enumerate() {
                    let payload = self.payload(&ty, vi, v, std::slice::from_ref(&x), frame)?;
                    // A payload-carrying enum is an array of tag and payload in
                    // JavaScript, so its length is mixed first and its tag
                    // second. A payloadless one is the tag itself.
                    let mut cur = if payloads {
                        let len = 1 + v.fields.len();
                        let with_len = self.mix(h.clone(), len as u128);
                        self.mix(with_len, vi as u128)
                    } else {
                        self.mix(h.clone(), vi as u128)
                    };
                    // Fields read where they're hashed are hashed into the
                    // accumulator a `let` holds, which `other` can then be.
                    let mut stmts = Vec::new();
                    for run in payload.runs() {
                        if payload.bound.is_none() {
                            let prev = frame.local("h", &acc);
                            stmts.push(typed::Stmt::Let {
                                pattern: Pattern {
                                    kind: PatKind::Bind { local: prev, sub: None },
                                    ty: acc,
                                    span: Span::NONE,
                                },
                                value: cur,
                                span: Span::NONE,
                            });
                            cur = self.local_expr(prev, &acc);
                        }
                        let descs = payload.fields.get(run.clone())?.to_vec();
                        let from = cur.clone();
                        cur = self.read(&payload, run, frame, cur, |g, _, values| {
                            let mut cur = from;
                            for ((d, _), xs) in descs.iter().zip(values) {
                                cur = g.at_hash(*d, cur, one(&xs)?)?;
                            }
                            Some(cur)
                        })?;
                    }
                    if !stmts.is_empty() {
                        cur = Expr::new(
                            ExprKind::Block { stmts, tail: Some(Box::new(cur)) },
                            acc,
                            Span::NONE,
                        );
                    }
                    let pat = payload.patterns.into_iter().next()?;
                    arms.push(self.arm(pat, cur));
                }
                Some(self.match_(x, arms, acc))
            }
            Desc::Opaque(_) | Desc::Reserved => None,
        }
    }

    fn hash_fields(
        &mut self,
        fields: &[(usize, usize)],
        h: Expr,
        x: Expr,
        tuple: bool,
    ) -> Option<Expr> {
        // `$hashInto` mixes an array's length before its elements, and a struct
        // is an array there.
        let mut cur = self.mix(h, fields.len() as u128);
        for (i, d) in fields {
            let fty = self.ty_of(*d);
            let proj = self.project(x.clone(), *i, tuple, fty);
            cur = self.at_hash(*d, cur, proj)?;
        }
        Some(cur)
    }

    fn at_hash(&mut self, desc: usize, h: Expr, x: Expr) -> Option<Expr> {
        self.at(Op::Hash, desc, vec![h, x])
    }

    // -- inlining or calling ------------------------------------------------

    /// The operation at one descriptor over the given arguments: inlined where
    /// that costs nothing, and a call to the generated function otherwise.
    ///
    /// Inlining is only allowed where every argument is used at most once, or
    /// is cheap to write twice — a primitive comparison writes both of its
    /// operands twice, so at a call site whose operand is a call it becomes a
    /// function instead.
    fn at(&mut self, op: Op, desc: usize, args: Vec<Expr>) -> Option<Expr> {
        // A `Flags` word compares and hashes as a primitive does.
        let prim = match self.desc(desc) {
            Some(Desc::Prim(_)) => true,
            Some(Desc::Flags { .. }) => !op.reads_names(),
            _ => false,
        };
        let unit = matches!(self.desc(desc), Some(Desc::Unit));
        // A leaf expansion may write an operand twice (`a < b`, then `a > b`)
        // or not at all (`()` is equal to `()`). A primitive's writes each
        // exactly once and can't fail, so it takes the operands themselves.
        // Copying them copied a hash's accumulator at every field, which made
        // a wide struct's hash `n²` (PERFORMANCE.md §6.36).
        if prim && op != Op::Compare {
            return self.leaf(op, desc, args);
        }
        // Otherwise it's inlined only over operands that may be written any
        // number of times, which are cheap to copy.
        if (prim || unit) && args.iter().all(Generator::duplicable) {
            if let Some(e) = self.leaf(op, desc, args.clone()) {
                return Some(e);
            }
        }
        let f = self.request(op, desc)?;
        let ret = self.result_ty(op);
        Some(self.call(f, args, ret))
    }

    /// The operation at a primitive or `()`, written out.
    fn leaf(&mut self, op: Op, desc: usize, args: Vec<Expr>) -> Option<Expr> {
        let mut frame = Frame::new();
        let mut args = args.into_iter();
        let built = match op {
            Op::Eq => {
                let (a, b) = (args.next()?, args.next()?);
                self.eq(desc, a, b, &mut frame)
            }
            Op::Compare => {
                let (a, b) = (args.next()?, args.next()?);
                self.compare(desc, a, b, &mut frame)
            }
            Op::Show => self.show(desc, args.next()?, &mut frame),
            Op::ToJson => self.json_of(desc, args.next()?, &mut frame),
            Op::Hash => {
                let (h, x) = (args.next()?, args.next()?);
                self.hash(desc, h, x, &mut frame)
            }
            // A decoder binds the payload it reads, so it is never a leaf.
            Op::FromJson => None,
        };
        // A leaf never allocates a local; if one appeared, the expression
        // would be referring to a frame nobody kept.
        if frame.locals.is_empty() { built } else { None }
    }
}

/// The name of a payload local on one side of a derived function.
fn side_name(side: usize) -> &'static str {
    if side == 0 { "x" } else { "y" }
}

fn one(args: &[Expr]) -> Option<Expr> {
    args.first().cloned()
}

/// `Option`'s variants, in declaration order (`core/option`).
const OPTION_SOME: usize = 0;
const OPTION_NONE: usize = 1;

/// `Result`'s variants, in declaration order (`core/result`).
const RESULT_OK: usize = 0;
const RESULT_ERR: usize = 1;

/// `DecodeError`'s variants, in `core/json`'s declaration order, which
/// `runtime.js` hard-codes too.
const DECODE_MISSING: usize = 0;
const DECODE_WRONG_TYPE: usize = 1;
const DECODE_UNKNOWN_VARIANT: usize = 2;

/// What a decoder builds from the parts it decoded.
#[derive(Clone, Copy)]
enum Build {
    Struct(Ty),
    Tuple(Ty),
    Variant(Ty, usize),
    /// A `derive Flags` word, packed from its decoded `Bool`s.
    Flags(flags::Word),
}

/// `Order`'s variants, in declaration order (`core/order`).
const ORDER_LESS: usize = 0;
const ORDER_EQUAL: usize = 1;
const ORDER_GREATER: usize = 2;

// ---------------------------------------------------------------------------
// Rewriting the call sites
// ---------------------------------------------------------------------------

/// Replaces every structural intrinsic that has a generated function with a
/// direct call to it.
fn rewrite(
    program: &mut Program,
    routed: &HashMap<(Op, usize), FuncIdx>,
    hash_ty: &Ty,
    out: &mut Derives,
) {
    let index = program.desc_index.clone();
    let mut rewritten = 0usize;
    for f in &mut program.funcs {
        let Some(body) = f.body_mut() else { continue };
        rewrite_expr(body, routed, &index, hash_ty, &mut rewritten);
    }
    out.rewritten = rewritten;
    // The descriptors a rewritten program still needs: exactly the ones an
    // intrinsic *function* was handed, since no expression reads one any more.
    for i in 0..program.funcs.len() {
        let Some(f) = program.funcs.get(i) else { continue };
        let (Some(key), Some(d)) = (f.intrinsic_key().map(str::to_owned), f.desc) else { continue };
        if key == JSON_DECODE {
            let Some(decoder) = routed.get(&(Op::FromJson, d)).copied() else { continue };
            // The decoder's second parameter is the path, which is a `Str`.
            let path_ty = program
                .funcs
                .get(decoder.index())
                .and_then(|g| g.locals.get(g.params.get(1)?.index()))
                .map(|l| l.ty);
            if let (Some(path_ty), Some(f)) = (path_ty, program.funcs.get_mut(i)) {
                decoder_body(f, decoder, path_ty);
            }
            continue;
        }
        let Some(show) = routed.get(&(Op::Show, d)).copied() else { continue };
        if let Some(f) = program.funcs.get_mut(i) {
            reporter_body(f, &key, show);
        }
    }
}

/// Gives `json.decode(ctx, value)` the body `decoder(value, "$")`: the path
/// starts at the document, as `$json_decode`'s does.
fn decoder_body(f: &mut Func, decoder: FuncIdx, path_ty: Ty) {
    let Some(value) = f.params.get(1).copied() else { return };
    let Some(json) = f.locals.get(value.index()).map(|l| l.ty) else { return };
    let args = vec![
        Expr::new(ExprKind::Local(value), json, Span::NONE),
        Expr::new(ExprKind::Str("$".to_string()), path_ty, Span::NONE),
    ];
    let call = ExprKind::CallFn { func: Callee::Func(decoder), args };
    let ret = f.ret;
    f.set_body(Expr::new(call, ret, Span::NONE));
}

/// Gives the test runner's two reporting intrinsics a body that renders their
/// values, where there is a generated `Show` at the type to render them with.
///
/// This is the substitution the module docs name: a descriptor reaches no
/// native artifact (VALUE-MODEL.md §9), so the descriptor `report` was handed
/// becomes a *call* at the type, and what reaches the runtime is two `Str`s
/// rather than a value and a walk. The bytes are then the program's own —
/// `$show`'s, by way of the `Show` this pass generates — which is what lets one
/// `commands/test.rs::report_failure` state the failure format for both
/// backends instead of each runtime having its own.
///
/// `report` renders **only on the branch that fails**. An assertion that passes
/// is a comparison and a branch, here as on JavaScript, because rendering both
/// sides of every passing assertion in a suite is the cost this shape exists to
/// avoid.
fn reporter_body(f: &mut Func, key: &str, show: FuncIdx) {
    let ty_of = |f: &Func, p: usize| {
        f.params.get(p).and_then(|l| f.locals.get(l.0 as usize)).map(|l| l.ty)
    };
    let local = |f: &Func, p: usize| {
        Some(Expr::new(ExprKind::Local(*f.params.get(p)?), ty_of(f, p)?, Span::NONE))
    };
    // A rendering answers a `Str`, and the one `Str` this function is sure to
    // name is the `kind` it was handed.
    let shown = |f: &Func, p: usize, str_ty: &Ty| {
        Some(Expr::new(
            ExprKind::CallFn { func: Callee::Func(show), args: vec![local(f, p)?] },
            *str_ty,
            Span::NONE,
        ))
    };
    let built = match key {
        // `report(passed, kind, actual, expected)`.
        "testing_assert.report" => (|| {
            let str_ty = ty_of(f, 1)?;
            let fail = Expr::new(
                ExprKind::Intrinsic {
                    name: String::from(REPORT_SHOWN),
                    targs: Vec::new(),
                    args: vec![local(f, 1)?, shown(f, 2, &str_ty)?, shown(f, 3, &str_ty)?],
                },
                f.ret,
                Span::NONE,
            );
            Some(Expr::new(
                ExprKind::If {
                    cond: Box::new(local(f, 0)?),
                    then: Box::new(Expr::new(ExprKind::Unit, f.ret, Span::NONE)),
                    else_: Box::new(fail),
                },
                f.ret,
                Span::NONE,
            ))
        })(),
        // `failExpected(kind, got)`, which answers the bottom type: it is
        // reached only where the test has already failed, so there is no branch
        // and the rendering is unconditional.
        "testing_assert.failExpected" => (|| {
            let str_ty = ty_of(f, 0)?;
            Some(Expr::new(
                ExprKind::Intrinsic {
                    name: String::from(EXPECTED_SHOWN),
                    targs: Vec::new(),
                    args: vec![local(f, 0)?, shown(f, 1, &str_ty)?],
                },
                f.ret,
                Span::NONE,
            ))
        })(),
        _ => None,
    };
    if let Some(body) = built {
        f.set_body(body);
    }
}

/// The key `core/json`'s bodyless `decode` monomorphizes to. Natively this
/// pass gives it a body (`decoder_body`), so no backend emits it.
pub const JSON_DECODE: &str = "json.decode";

/// `report`, with both values rendered: `(kind, actual, expected) -> ()`, and
/// it does not return. Named here because both native backends lower it and
/// neither may spell it differently.
pub const REPORT_SHOWN: &str = "testing_assert.reportShown";

/// `failExpected`, with its one value rendered: `(kind, got) -> R`, and it does
/// not return.
pub const EXPECTED_SHOWN: &str = "testing_assert.failExpectedShown";

fn rewrite_expr(
    e: &mut Expr,
    routed: &HashMap<(Op, usize), FuncIdx>,
    index: &HashMap<Ty, usize>,
    hash_ty: &Ty,
    n: &mut usize,
) {
    // Children first: an argument may itself be a structural call.
    typed::children_mut(e, &mut |c| rewrite_expr(c, routed, index, hash_ty, n));
    let replacement = match &e.kind {
        ExprKind::Intrinsic { name, args, .. } => {
            let op = Op::all().into_iter().find(|o| o.intrinsic() == name);
            match (op, descriptor_arg(args)) {
                (Some(op), Some(d)) => routed.get(&(op, d)).map(|f| {
                    let mut values: Vec<Expr> =
                        args.iter().take(op.values()).cloned().collect();
                    if op == Op::Hash {
                        values.insert(
                            0,
                            Expr::new(ExprKind::Int(typed::Magnitude::new(HASH_SEED), false), *hash_ty, Span::NONE),
                        );
                    }
                    ExprKind::CallFn { func: Callee::Func(*f), args: values }
                }),
                _ => None,
            }
        }
        ExprKind::StructuralEq { negate, args } => {
            let d = args.first().and_then(|a| index.get(&a.ty)).copied();
            match d.and_then(|d| routed.get(&(Op::Eq, d))) {
                Some(f) => {
                    let call = ExprKind::CallFn { func: Callee::Func(*f), args: args.clone() };
                    if *negate {
                        // `!=` is the same call under a negation, which is one
                        // instruction rather than a second generated function.
                        Some(ExprKind::Prim {
                            op: PrimOp::Not,
                            prim: Prim::Bool,
                            args: vec![Expr::new(call, e.ty, e.span)],
                        })
                    } else {
                        Some(call)
                    }
                }
                None => None,
            }
        }
        _ => None,
    };
    if let Some(kind) = replacement {
        e.kind = kind;
        *n += 1;
    }
}
