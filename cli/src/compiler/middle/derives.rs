//! A generated `Show`, `Eq`, `Ord`, `Hash` and `ToJson` per type.
//! **Wave 1e.**
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
//! and reports the third, which is a wave-2 seam rather than a rewrite (§ *What
//! is scaffolded*).
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
//! conformance needs. That is the contract wave 2a and 2b implement; it is
//! stated here because this pass is the only thing that emits them.
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
//! divergence is named in `design/native/OPEN-QUESTIONS.md`'s terms rather than
//! silently reproduced.
//!
//! # Sharing
//!
//! One function per **shape**, not per type: `struct Meters(I64)` and
//! `struct Seconds(I64)` share `eq`, `cmp` and `hash`, because a derived
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
//! # What is scaffolded
//!
//! * **`FromJson`.** `json.decode` stays a `FuncKind::Intrinsic` carrying
//!   `Func::desc`, and this pass records which descriptors it needs in
//!   [`Derives::from_json`]. A generated decoder has to *construct*
//!   `Result<T, DecodeError>` values, which means naming `core/json`'s type
//!   constructors — and this pass is handed a [`Program`], which has no type
//!   table (`middle::native` takes no `Tables`). Encoding needs no such thing
//!   because `Json`'s own type constructor arrives on the `structuralToJson`
//!   call site; decoding has no call site to read one from. Closing it is a
//!   change to `middle::native`'s signature, which is wave 0's file.
//! * **The test reporter.** `testing_assert.report` is handed a descriptor the
//!   same way; the `Show` instances it needs are generated and listed in
//!   [`Derives::reporter_show`], so wave 2 can hand it a code pointer instead
//!   of a descriptor.
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

use crate::compiler::middle::monomorphize::{Desc, DescVariant, Func, FuncKind, Program};
use crate::compiler::semantics::typed::{
    self, Arm, Callee, Expr, ExprKind, FieldPat, PatKind, Pattern, PrimOp, TemplatePart,
};
use crate::compiler::semantics::types::{FuncIdx, LocalId, Prim, Ty, TyConId};
use crate::diagnostics::Span;
use crate::hash::Map as HashMap;

/// FNV-1a's offset basis, which is where `$hash` starts. Written here as well
/// as in `runtime.js` because the two have to agree and neither can import the
/// other.
const HASH_SEED: u128 = 0x811c_9dc5;

/// The five operations a `derive` can stand for, once monomorphization has
/// resolved it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Op {
    Eq,
    Compare,
    Show,
    ToJson,
    Hash,
}

impl Op {
    /// The intrinsic name monomorphization left at a call site.
    fn intrinsic(self) -> &'static str {
        match self {
            Op::Eq => "structuralEq",
            Op::Compare => "structuralCompare",
            Op::Show => "structuralShow",
            Op::ToJson => "structuralToJson",
            Op::Hash => "structuralHash",
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
        }
    }

    /// Whether the operation *prints* names, which is what decides whether two
    /// layout-identical types may share one generated function.
    fn reads_names(self) -> bool {
        matches!(self, Op::Show | Op::ToJson)
    }

    /// How many values of the described type the generated function takes.
    /// `Hash` takes one, behind an accumulator.
    fn values(self) -> usize {
        match self {
            Op::Eq | Op::Compare => 2,
            Op::Show | Op::ToJson | Op::Hash => 1,
        }
    }

    fn all() -> [Op; 5] {
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

/// What the pass did, for the backends and for the tests.
///
/// Returned rather than stored: `Program` has no field for it and
/// `monomorphize.rs` is not this wave's file. `middle::native` discards it
/// today; wave 2 calls [`run`] itself and keeps it.
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
    /// Descriptors `json.decode` was handed. `FromJson` is not generated — see
    /// the module docs.
    pub from_json: Vec<usize>,
    /// `(descriptor, generated Show)` for each intrinsic that carries a
    /// descriptor for rendering — the test runner's `report`.
    pub reporter_show: Vec<(usize, FuncIdx)>,
}

impl Derives {
    /// The generated function for one operation at one descriptor, if there is
    /// one.
    pub fn instance(&self, op: Op, desc: usize) -> Option<FuncIdx> {
        self.routes.iter().find(|(o, d, _)| *o == op && *d == desc).map(|(_, _, f)| *f)
    }
}

/// Adds one derived function per shape per structural operation the program
/// reaches, and rewrites every derive call site to a direct call.
pub fn run(program: &mut Program) -> Derives {
    let mut g = Generator::new(program);
    let wanted = collect(program, &mut g.out);
    for (op, desc) in wanted {
        g.request(op, desc);
    }
    g.drain();
    let built = g.finish();
    program.funcs.extend(built.funcs);
    let mut out = built.out;
    rewrite(program, &built.routed, &built.hash_ty, &mut out);
    out
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
            if key == "json.decode" {
                if !out.from_json.contains(&d) {
                    out.from_json.push(d);
                }
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
        Some(ExprKind::Int(v, false)) => usize::try_from(*v).ok(),
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
                *slot = Some(ty.clone());
            }
        }
        let mut prim_of: HashMap<Prim, Ty> = HashMap::default();
        for (i, d) in program.descriptors.iter().enumerate() {
            if let (Desc::Prim(p), Some(Some(ty))) = (d, ty_of.get(i)) {
                prim_of.entry(*p).or_insert_with(|| ty.clone());
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
                        prim_of.entry(Prim::Str).or_insert_with(|| e.ty.clone());
                    }
                    ExprKind::Bool(_) => {
                        prim_of.entry(Prim::Bool).or_insert_with(|| e.ty.clone());
                    }
                    ExprKind::Char(_) => {
                        prim_of.entry(Prim::Char).or_insert_with(|| e.ty.clone());
                    }
                    ExprKind::Prim { prim, args, .. } => {
                        if let Some(a) = args.first() {
                            prim_of.entry(*prim).or_insert_with(|| a.ty.clone());
                        }
                    }
                    ExprKind::StructuralEq { .. } => {
                        result.entry(Op::Eq).or_insert_with(|| e.ty.clone());
                    }
                    ExprKind::Intrinsic { name, .. } => {
                        if let Some(op) = Op::all().into_iter().find(|o| o.intrinsic() == name) {
                            result.entry(op).or_insert_with(|| e.ty.clone());
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
        // The tags of `Json`, from the program where it described the type.
        for (i, d) in program.descriptors.iter().enumerate() {
            let Desc::Enum { name, variants } = d else { continue };
            if name != "Json" {
                continue;
            }
            let is_json = result
                .get(&Op::ToJson)
                .and_then(Ty::head)
                .zip(ty_of.get(i).and_then(|t| t.as_ref()).and_then(Ty::head))
                .is_some_and(|(a, b)| a == b);
            if is_json {
                json_variants = variants.iter().map(|v| v.name.clone()).collect();
            }
        }
        if json_variants.is_empty() {
            json_variants = JSON_VARIANTS.iter().map(|s| (*s).to_string()).collect();
        }
        Env { ty_of, prim_of, result, json_variants }
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
        Desc::Struct { fields, .. } => fields.iter().map(|f| f.ty).collect(),
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
        self.locals.push(typed::Local { name: name.to_string(), ty: ty.clone(), span: Span::NONE });
        id
    }

    fn param(&mut self, name: &str, ty: &Ty) -> LocalId {
        let id = self.local(name, ty);
        self.params.push(id);
        id
    }
}

struct Generator {
    descs: Vec<Desc>,
    env: Env,
    ok: Vec<bool>,
    /// Where the generated functions start in `Program::funcs`.
    base: usize,
    funcs: Vec<Func>,
    /// Shape key to the function that implements it.
    shared: HashMap<String, FuncIdx>,
    /// `(op, descriptor)` to the function that serves it.
    routed: HashMap<(Op, usize), FuncIdx>,
    /// Instances whose body is still to be built.
    queue: Vec<(Op, usize, FuncIdx)>,
    out: Derives,
}

impl Generator {
    fn new(program: &Program) -> Generator {
        let env = Env::discover(program);
        let ok = support(program, &env);
        Generator {
            descs: program.descriptors.clone(),
            env,
            ok,
            base: program.funcs.len(),
            funcs: Vec::new(),
            shared: HashMap::default(),
            routed: HashMap::default(),
            queue: Vec::new(),
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
        if self.env.result(op).is_none() {
            self.decline(op, desc, Declined::NoResultType);
            return None;
        }
        // `Hash` threads an accumulator whose type is its own result type, and
        // `ToJson` writes object keys, so both need `Str` as well.
        if op == Op::ToJson && !self.env.prim_of.contains_key(&Prim::Str) {
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
        let ret = self.env.result(op).cloned().unwrap_or(Ty::Error);
        let name = self.symbol(op, desc);
        self.funcs.push(Func {
            symbol: name.clone(),
            debug_name: name,
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

    fn decline(&mut self, op: Op, desc: usize, why: Declined) {
        if !self.out.declined.iter().any(|(o, d, _)| *o == op && *d == desc) {
            self.out.declined.push((op, desc, why));
        }
    }

    /// A stable, readable symbol. Two shapes that share a function share the
    /// name of whichever reached it first, which is the same rule
    /// `monomorphize` uses for an instantiation.
    fn symbol(&self, op: Op, desc: usize) -> String {
        let base = match self.desc(desc) {
            Some(Desc::Struct { name, .. }) | Some(Desc::Enum { name, .. }) => name.clone(),
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
        format!("$derive${}${clean}${desc}", op.tag())
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
        self.env.ty(desc).cloned().unwrap_or(Ty::Error)
    }

    fn result_ty(&self, op: Op) -> Ty {
        self.env.result(op).cloned().unwrap_or(Ty::Error)
    }

    fn str_ty(&self) -> Ty {
        self.env.prim_of.get(&Prim::Str).cloned().unwrap_or(Ty::Error)
    }

    fn bool_ty(&self) -> Ty {
        self.env.result(Op::Eq).cloned().unwrap_or(Ty::Error)
    }

    fn local_expr(&self, id: LocalId, ty: &Ty) -> Expr {
        Expr::new(ExprKind::Local(id), ty.clone(), Span::NONE)
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
            Ty::Fn(params, Box::new(ret)),
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
                    ty: t.clone(),
                    span: Span::NONE,
                },
            })
            .collect();
        Some(Pattern {
            kind: PatKind::Variant { con, variant, fields },
            ty: ty.clone(),
            span: Span::NONE,
        })
    }

    fn wild(&self, ty: &Ty) -> Pattern {
        Pattern { kind: PatKind::Wild, ty: ty.clone(), span: Span::NONE }
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
        let targs = match ty {
            Ty::Con(_, a) => a.clone(),
            _ => Vec::new(),
        };
        Some(Expr::new(
            ExprKind::EnumLit { con, targs, variant, args },
            ty.clone(),
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
        match self.desc(desc).cloned()? {
            Desc::Prim(p) => Some(Expr::new(
                ExprKind::Prim { op: PrimOp::Eq, prim: p, args: vec![a, b] },
                bool_ty,
                Span::NONE,
            )),
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
                let elem_ty = self.ty_of(elem);
                let f = self.request(Op::Eq, elem)?;
                let ptr = self.fn_ref(f, vec![elem_ty.clone(), elem_ty.clone()], bool_ty.clone());
                Some(self.intrinsic("deriveArrayEq", vec![elem_ty], vec![a, b, ptr], bool_ty))
            }
            Desc::Option(inner) => self.eq_option(desc, inner, a, b, frame),
            Desc::Enum { variants, .. } => self.eq_enum(desc, &variants, a, b, frame),
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
            let ae = self.project(a.clone(), *i, tuple, fty.clone());
            let be = self.project(b.clone(), *i, tuple, fty);
            let one = self.at(Op::Eq, *d, vec![ae, be])?;
            acc = Some(match acc {
                None => one,
                Some(rest) => Expr::new(
                    ExprKind::And { lhs: Box::new(one), rhs: Box::new(rest) },
                    bool_ty.clone(),
                    Span::NONE,
                ),
            });
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
        let some_x = self.variant_pattern(&ty, OPTION_SOME, &[(0, x, inner_ty.clone())])?;
        let some_y = self.variant_pattern(&ty, OPTION_SOME, &[(0, y, inner_ty.clone())])?;
        let none = self.variant_pattern(&ty, OPTION_NONE, &[])?;
        let inner_eq = self.at(
            Op::Eq,
            inner,
            vec![self.local_expr(x, &inner_ty), self.local_expr(y, &inner_ty)],
        )?;
        let false_ = Expr::new(ExprKind::Bool(false), bool_ty.clone(), Span::NONE);
        let true_ = Expr::new(ExprKind::Bool(true), bool_ty.clone(), Span::NONE);
        let some_arm = self.match_(
            b.clone(),
            vec![self.arm(some_y, inner_eq), self.arm(self.wild(&ty), false_.clone())],
            bool_ty.clone(),
        );
        let none_arm = self.match_(
            b,
            vec![self.arm(none, true_), self.arm(self.wild(&ty), false_)],
            bool_ty.clone(),
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
        for (vi, v) in variants.iter().enumerate() {
            let mut xs: Vec<(usize, LocalId, Ty)> = Vec::new();
            let mut ys: Vec<(usize, LocalId, Ty)> = Vec::new();
            for (fi, f) in v.fields.iter().enumerate() {
                let fty = self.ty_of(f.ty);
                xs.push((fi, frame.local("x", &fty), fty.clone()));
                ys.push((fi, frame.local("y", &fty), fty));
            }
            let px = self.variant_pattern(&ty, vi, &xs)?;
            let py = self.variant_pattern(&ty, vi, &ys)?;
            let mut acc: Option<Expr> = None;
            for (k, f) in v.fields.iter().enumerate().rev() {
                let (_, xl, xt) = xs.get(k)?;
                let (_, yl, _) = ys.get(k)?;
                let one = self.at(
                    Op::Eq,
                    f.ty,
                    vec![self.local_expr(*xl, xt), self.local_expr(*yl, xt)],
                )?;
                acc = Some(match acc {
                    None => one,
                    Some(rest) => Expr::new(
                        ExprKind::And { lhs: Box::new(one), rhs: Box::new(rest) },
                        bool_ty.clone(),
                        Span::NONE,
                    ),
                });
            }
            let same = acc.unwrap_or_else(|| {
                Expr::new(ExprKind::Bool(true), bool_ty.clone(), Span::NONE)
            });
            let inner = self.match_(
                b.clone(),
                vec![
                    self.arm(py, same),
                    self.arm(
                        self.wild(&ty),
                        Expr::new(ExprKind::Bool(false), bool_ty.clone(), Span::NONE),
                    ),
                ],
                bool_ty.clone(),
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

    fn compare(&mut self, desc: usize, a: Expr, b: Expr, frame: &mut Frame) -> Option<Expr> {
        let order = self.result_ty(Op::Compare);
        match self.desc(desc).cloned()? {
            Desc::Prim(p) => {
                let bool_ty = self.bool_ty();
                let lt = Expr::new(
                    ExprKind::Prim { op: PrimOp::Lt, prim: p, args: vec![a.clone(), b.clone()] },
                    bool_ty.clone(),
                    Span::NONE,
                );
                let gt = Expr::new(
                    ExprKind::Prim { op: PrimOp::Gt, prim: p, args: vec![a, b] },
                    bool_ty,
                    Span::NONE,
                );
                let inner = Expr::new(
                    ExprKind::If {
                        cond: Box::new(gt),
                        then: Box::new(self.order_lit(ORDER_GREATER)?),
                        else_: Box::new(self.order_lit(ORDER_EQUAL)?),
                    },
                    order.clone(),
                    Span::NONE,
                );
                Some(Expr::new(
                    ExprKind::If {
                        cond: Box::new(lt),
                        then: Box::new(self.order_lit(ORDER_LESS)?),
                        else_: Box::new(inner),
                    },
                    order,
                    Span::NONE,
                ))
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
                let elem_ty = self.ty_of(elem);
                let f = self.request(Op::Compare, elem)?;
                let ptr = self.fn_ref(f, vec![elem_ty.clone(), elem_ty.clone()], order.clone());
                Some(self.intrinsic(
                    "deriveArrayCompare",
                    vec![elem_ty],
                    vec![a, b, ptr],
                    order,
                ))
            }
            Desc::Option(inner) => self.compare_option(desc, inner, a, b, frame),
            Desc::Enum { variants, .. } => self.compare_enum(desc, &variants, a, b, frame),
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
        let order = self.result_ty(Op::Compare);
        let mut acc: Option<Expr> = None;
        for (i, d) in fields.iter().rev() {
            let fty = self.ty_of(*d);
            let ae = self.project(a.clone(), *i, tuple, fty.clone());
            let be = self.project(b.clone(), *i, tuple, fty);
            let one = self.at(Op::Compare, *d, vec![ae, be])?;
            acc = Some(match acc {
                None => one,
                Some(rest) => {
                    let c = frame.local("c", &order);
                    let equal = self.variant_pattern(&order, ORDER_EQUAL, &[])?;
                    let bind = Pattern {
                        kind: PatKind::Bind { local: c, sub: None },
                        ty: order.clone(),
                        span: Span::NONE,
                    };
                    self.match_(
                        one,
                        vec![
                            self.arm(equal, rest),
                            self.arm(bind, self.local_expr(c, &order)),
                        ],
                        order.clone(),
                    )
                }
            });
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
        let some_x = self.variant_pattern(&ty, OPTION_SOME, &[(0, x, inner_ty.clone())])?;
        let some_y = self.variant_pattern(&ty, OPTION_SOME, &[(0, y, inner_ty.clone())])?;
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
            order.clone(),
        );
        let none_arm = self.match_(
            b,
            vec![
                self.arm(none, self.order_lit(ORDER_EQUAL)?),
                self.arm(self.wild(&ty), self.order_lit(ORDER_LESS)?),
            ],
            order.clone(),
        );
        Some(self.match_(
            a,
            vec![self.arm(some_x, some_arm), self.arm(self.wild(&ty), none_arm)],
            order,
        ))
    }

    /// Tag first, then payload — declaration order is the order, which is what
    /// makes `derive Ord` on an enum mean what a reader of the declaration
    /// expects.
    fn compare_enum(
        &mut self,
        desc: usize,
        variants: &[DescVariant],
        a: Expr,
        b: Expr,
        frame: &mut Frame,
    ) -> Option<Expr> {
        let ty = self.ty_of(desc);
        let order = self.result_ty(Op::Compare);
        let mut arms: Vec<Arm> = Vec::new();
        for (vi, v) in variants.iter().enumerate() {
            let mut xs: Vec<(usize, LocalId, Ty)> = Vec::new();
            let mut ys: Vec<(usize, LocalId, Ty)> = Vec::new();
            for (fi, f) in v.fields.iter().enumerate() {
                let fty = self.ty_of(f.ty);
                xs.push((fi, frame.local("x", &fty), fty.clone()));
                ys.push((fi, frame.local("y", &fty), fty));
            }
            let px = self.variant_pattern(&ty, vi, &xs)?;
            let py = self.variant_pattern(&ty, vi, &ys)?;
            // Same variant: compare the payloads lexicographically.
            let mut acc: Option<Expr> = None;
            for (k, f) in v.fields.iter().enumerate().rev() {
                let (_, xl, xt) = xs.get(k)?;
                let (_, yl, _) = ys.get(k)?;
                let one = self.at(
                    Op::Compare,
                    f.ty,
                    vec![self.local_expr(*xl, xt), self.local_expr(*yl, xt)],
                )?;
                acc = Some(match acc {
                    None => one,
                    Some(rest) => {
                        let c = frame.local("c", &order);
                        let equal = self.variant_pattern(&order, ORDER_EQUAL, &[])?;
                        let bind = Pattern {
                            kind: PatKind::Bind { local: c, sub: None },
                            ty: order.clone(),
                            span: Span::NONE,
                        };
                        self.match_(
                            one,
                            vec![
                                self.arm(equal, rest),
                                self.arm(bind, self.local_expr(c, &order)),
                            ],
                            order.clone(),
                        )
                    }
                });
            }
            let same = match acc {
                Some(e) => e,
                None => self.order_lit(ORDER_EQUAL)?,
            };
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
            arms.push(self.arm(px, self.match_(b.clone(), inner, order.clone())));
        }
        Some(self.match_(a, arms, order))
    }

    /// A variant pattern that binds nothing, for a test that only reads the
    /// tag. Fields are matched with `_`, which is what makes it legal at a
    /// variant that carries a payload.
    fn tag_pattern(&self, ty: &Ty, variant: usize) -> Option<Pattern> {
        let con = ty.head()?;
        Some(Pattern {
            kind: PatKind::Variant { con, variant, fields: Vec::new() },
            ty: ty.clone(),
            span: Span::NONE,
        })
    }

    // -- rendering ----------------------------------------------------------

    fn show(&mut self, desc: usize, x: Expr, frame: &mut Frame) -> Option<Expr> {
        let str_ty = self.str_ty();
        match self.desc(desc).cloned()? {
            Desc::Prim(_) => {
                let ty = self.ty_of(desc);
                Some(self.intrinsic("derivePrimShow", vec![ty], vec![x], str_ty))
            }
            Desc::Unit => Some(self.str_lit("()")),
            Desc::Struct { name, record, fields } => {
                if fields.is_empty() {
                    return Some(self.str_lit(&name));
                }
                let parts: Vec<(String, usize, usize)> = fields
                    .iter()
                    .enumerate()
                    .map(|(i, f)| (f.name.clone(), i, f.ty))
                    .collect();
                self.show_fields(&name, record, &parts, x, false)
            }
            Desc::Tuple(es) => {
                let parts: Vec<(String, usize, usize)> =
                    es.iter().enumerate().map(|(i, d)| (String::new(), i, *d)).collect();
                self.show_fields("", false, &parts, x, true)
            }
            Desc::Array(elem) => {
                let elem_ty = self.ty_of(elem);
                let f = self.request(Op::Show, elem)?;
                let ptr = self.fn_ref(f, vec![elem_ty.clone()], str_ty.clone());
                Some(self.intrinsic("deriveArrayShow", vec![elem_ty], vec![x, ptr], str_ty))
            }
            Desc::Option(inner) => {
                let ty = self.ty_of(desc);
                let inner_ty = self.ty_of(inner);
                let v = frame.local("v", &inner_ty);
                let some = self.variant_pattern(&ty, OPTION_SOME, &[(0, v, inner_ty.clone())])?;
                let shown = self.at(Op::Show, inner, vec![self.local_expr(v, &inner_ty)])?;
                let body = self.template(vec![
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
                        self.template(parts)
                    };
                    arms.push(self.arm(pat, body));
                }
                Some(self.match_(x, arms, str_ty))
            }
            Desc::Opaque(_) | Desc::Reserved => None,
        }
    }

    /// Every hole in a generated template is already a `Str`, so a backend
    /// needs no per-type rendering to lower one: it is concatenation.
    ///
    /// Adjacent literal text is merged, because a field name and the separator
    /// before it are written separately and are one string.
    fn template(&self, parts: Vec<TemplatePart>) -> Expr {
        let mut merged: Vec<TemplatePart> = Vec::new();
        for p in parts {
            match (merged.last_mut(), p) {
                (Some(TemplatePart::Text(prev)), TemplatePart::Text(next)) => prev.push_str(&next),
                (_, p) => merged.push(p),
            }
        }
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
        Some(self.template(parts))
    }

    // -- JSON ---------------------------------------------------------------

    fn json_lit(&self, name: &str, args: Vec<Expr>) -> Option<Expr> {
        let ty = self.result_ty(Op::ToJson);
        let v = self.env.json_variant(name)?;
        self.enum_lit(&ty, v, args)
    }

    fn json_array_ty(&self) -> Ty {
        Ty::Array(Box::new(self.result_ty(Op::ToJson)))
    }

    fn json_array(&self, items: Vec<Expr>) -> Expr {
        Expr::new(ExprKind::Array(items), self.json_array_ty(), Span::NONE)
    }

    /// `[(Str, Json)]`, which is what `.Object` carries.
    fn json_object_ty(&self) -> Ty {
        Ty::Array(Box::new(Ty::Tuple(vec![self.str_ty(), self.result_ty(Op::ToJson)])))
    }

    fn json_members(&self, members: Vec<(String, Expr)>) -> Expr {
        let pair_ty = Ty::Tuple(vec![self.str_ty(), self.result_ty(Op::ToJson)]);
        let items: Vec<Expr> = members
            .into_iter()
            .map(|(k, v)| {
                Expr::new(
                    ExprKind::Tuple(vec![self.str_lit(&k), v]),
                    pair_ty.clone(),
                    Span::NONE,
                )
            })
            .collect();
        Expr::new(ExprKind::Array(items), self.json_object_ty(), Span::NONE)
    }

    fn json_of(&mut self, desc: usize, x: Expr, frame: &mut Frame) -> Option<Expr> {
        let json = self.result_ty(Op::ToJson);
        match self.desc(desc).cloned()? {
            Desc::Prim(_) => {
                let ty = self.ty_of(desc);
                Some(self.intrinsic("derivePrimJson", vec![ty], vec![x], json))
            }
            Desc::Unit => self.json_lit("Null", Vec::new()),
            Desc::Struct { record, fields, .. } => {
                let mut members: Vec<(String, Expr)> = Vec::new();
                let mut items: Vec<Expr> = Vec::new();
                for (i, f) in fields.iter().enumerate() {
                    let fty = self.ty_of(f.ty);
                    let proj = self.project(x.clone(), i, false, fty);
                    let v = self.at(Op::ToJson, f.ty, vec![proj])?;
                    if record {
                        members.push((f.name.clone(), v));
                    } else {
                        items.push(v);
                    }
                }
                if record {
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
                let elem_ty = self.ty_of(elem);
                let f = self.request(Op::ToJson, elem)?;
                let ptr = self.fn_ref(f, vec![elem_ty.clone()], json);
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
                let inner_ty = self.ty_of(inner);
                let v = frame.local("v", &inner_ty);
                let some = self.variant_pattern(&ty, OPTION_SOME, &[(0, v, inner_ty.clone())])?;
                let body = self.at(Op::ToJson, inner, vec![self.local_expr(v, &inner_ty)])?;
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

    // -- hashing ------------------------------------------------------------

    fn hash_ty(&self) -> Ty {
        self.result_ty(Op::Hash)
    }

    fn hash_int(&self, v: u128) -> Expr {
        Expr::new(ExprKind::Int(v, false), self.hash_ty(), Span::NONE)
    }

    /// `$mix(h, n)` on a number the shape itself supplies — a field count or a
    /// tag. Goes through the same primitive intrinsic as a hashed value, at the
    /// accumulator's own type.
    fn mix(&self, h: Expr, n: u128) -> Expr {
        let ty = self.hash_ty();
        self.intrinsic("derivePrimHash", vec![ty.clone()], vec![h, self.hash_int(n)], ty)
    }

    fn hash(&mut self, desc: usize, h: Expr, x: Expr, frame: &mut Frame) -> Option<Expr> {
        let acc = self.hash_ty();
        match self.desc(desc).cloned()? {
            Desc::Prim(_) => {
                let ty = self.ty_of(desc);
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
                let elem_ty = self.ty_of(elem);
                let f = self.request(Op::Hash, elem)?;
                let ptr = self.fn_ref(f, vec![acc.clone(), elem_ty.clone()], acc.clone());
                Some(self.intrinsic("deriveArrayHash", vec![elem_ty], vec![h, x, ptr], acc))
            }
            Desc::Option(inner) => {
                let ty = self.ty_of(desc);
                let inner_ty = self.ty_of(inner);
                let v = frame.local("v", &inner_ty);
                let some = self.variant_pattern(&ty, OPTION_SOME, &[(0, v, inner_ty.clone())])?;
                let body =
                    self.at_hash(inner, h.clone(), self.local_expr(v, &inner_ty))?;
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
                    let mut binds: Vec<(usize, LocalId, Ty)> = Vec::new();
                    for (fi, f) in v.fields.iter().enumerate() {
                        let fty = self.ty_of(f.ty);
                        binds.push((fi, frame.local("v", &fty), fty));
                    }
                    let pat = self.variant_pattern(&ty, vi, &binds)?;
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
                    for (k, f) in v.fields.iter().enumerate() {
                        let (_, l, lt) = binds.get(k)?;
                        cur = self.at_hash(f.ty, cur, self.local_expr(*l, lt))?;
                    }
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
        let leaf = matches!(self.desc(desc), Some(Desc::Prim(_) | Desc::Unit));
        if leaf {
            // A leaf expansion may write an operand twice (`a < b`, then
            // `a > b`) or not at all (`()` is equal to `()`). Where it writes
            // each exactly once it is inlined over anything; otherwise only
            // over operands that may be written any number of times.
            let linear = op != Op::Compare && matches!(self.desc(desc), Some(Desc::Prim(_)));
            if linear || args.iter().all(Generator::duplicable) {
                let mut frame = Frame::new();
                let built = match op {
                    Op::Eq => {
                        let (a, b) = two(&args)?;
                        self.eq(desc, a, b, &mut frame)
                    }
                    Op::Compare => {
                        let (a, b) = two(&args)?;
                        self.compare(desc, a, b, &mut frame)
                    }
                    Op::Show => self.show(desc, one(&args)?, &mut frame),
                    Op::ToJson => self.json_of(desc, one(&args)?, &mut frame),
                    Op::Hash => {
                        let (h, x) = two(&args)?;
                        self.hash(desc, h, x, &mut frame)
                    }
                };
                // A leaf never allocates a local; if one appeared, the
                // expression would be referring to a frame nobody kept.
                if frame.locals.is_empty() {
                    if let Some(e) = built {
                        return Some(e);
                    }
                }
            }
        }
        let f = self.request(op, desc)?;
        let ret = self.result_ty(op);
        Some(self.call(f, args, ret))
    }
}

fn one(args: &[Expr]) -> Option<Expr> {
    args.first().cloned()
}

fn two(args: &[Expr]) -> Option<(Expr, Expr)> {
    Some((args.first()?.clone(), args.get(1)?.clone()))
}

/// `Option`'s variants, in declaration order (`core/option`).
const OPTION_SOME: usize = 0;
const OPTION_NONE: usize = 1;

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
    let mut reporter: Vec<(usize, FuncIdx)> = Vec::new();
    for f in &program.funcs {
        if let (Some(key), Some(d)) = (f.intrinsic_key(), f.desc) {
            if key != "json.decode" {
                if let Some(idx) = routed.get(&(Op::Show, d)) {
                    reporter.push((d, *idx));
                }
            }
        }
    }
    out.reporter_show = reporter;
}

fn rewrite_expr(
    e: &mut Expr,
    routed: &HashMap<(Op, usize), FuncIdx>,
    index: &HashMap<Ty, usize>,
    hash_ty: &Ty,
    n: &mut usize,
) {
    // Children first: an argument may itself be a structural call.
    each_child(e, &mut |c| rewrite_expr(c, routed, index, hash_ty, n));
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
                            Expr::new(ExprKind::Int(HASH_SEED, false), hash_ty.clone(), Span::NONE),
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
                            args: vec![Expr::new(call, e.ty.clone(), e.span)],
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

/// Every direct sub-expression, mutably. `typed::walk` is the shared read-only
/// walk; there is no mutable one, and a pass that rewrites in place needs the
/// same coverage.
fn each_child(e: &mut Expr, f: &mut impl FnMut(&mut Expr)) {
    match &mut e.kind {
        ExprKind::CallValue { callee, args } => {
            f(callee);
            args.iter_mut().for_each(f);
        }
        ExprKind::CallFn { args, .. }
        | ExprKind::CallTrait { args, .. }
        | ExprKind::StructLit { fields: args, .. }
        | ExprKind::EnumLit { args, .. }
        | ExprKind::Tuple(args)
        | ExprKind::Array(args)
        | ExprKind::Prim { args, .. }
        | ExprKind::StructuralEq { args, .. }
        | ExprKind::StructuralCmp { args, .. }
        | ExprKind::Closure { env: args, .. }
        // A tail-recursive body is a `Loop` by the time this runs, so a derive
        // call site inside one is inside these two and nowhere else. Missing
        // them left a `structuralEq` in the tree for `lower` to meet as an
        // `Inst::Structural` — the same traversal gap `rc::kids` had.
        | ExprKind::Continue { args, .. }
        | ExprKind::Loop { entries: args }
        | ExprKind::Intrinsic { args, .. } => args.iter_mut().for_each(f),
        ExprKind::StructUpdate { base, updates, .. } => {
            f(base);
            updates.iter_mut().for_each(|(_, e)| f(e));
        }
        ExprKind::Field { base, .. }
        | ExprKind::TupleIndex { base, .. }
        | ExprKind::CtxGet { base, .. }
        | ExprKind::Try { base, .. } => f(base),
        ExprKind::Index { base, index, .. } => {
            f(base);
            f(index);
        }
        ExprKind::Block { stmts, tail } => {
            for s in stmts {
                match s {
                    typed::Stmt::Let { value, .. } => f(value),
                    typed::Stmt::Expr(e) => f(e),
                }
            }
            if let Some(t) = tail {
                f(t);
            }
        }
        ExprKind::If { cond, then, else_ } => {
            f(cond);
            f(then);
            f(else_);
        }
        ExprKind::Match { scrutinee, arms } => {
            f(scrutinee);
            for a in arms {
                if let Some(g) = &mut a.guard {
                    f(g);
                }
                f(&mut a.body);
            }
        }
        ExprKind::Lambda { body, .. } => f(body),
        ExprKind::And { lhs, rhs }
        | ExprKind::Or { lhs, rhs }
        | ExprKind::Coalesce { lhs, rhs, .. } => {
            f(lhs);
            f(rhs);
        }
        ExprKind::Template { parts } => {
            for p in parts {
                if let TemplatePart::Hole(h) = p {
                    f(h);
                }
            }
        }
        ExprKind::CtxLit { bindings } => bindings.iter_mut().for_each(|(_, e)| f(e)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::middle::monomorphize;
    use crate::compiler::semantics::typed::{Pattern, Stmt};
    use crate::diagnostics::{Diagnostics, SourceMap};

    /// One snippet, through the real front end and the shared half of the
    /// middle end — the same path `driver::run_snippet` takes, stopping where
    /// a backend would be chosen.
    fn compile(src: &str) -> (Program, crate::compiler::semantics::types::Tables) {
        let mut map = SourceMap::new();
        let analysis = crate::compiler::driver::analyze_snippet(
            &mut map,
            "derives_test.buri",
            src,
            crate::compiler::modules::Role::Entry,
        );
        let errors: Vec<String> = analysis
            .diags
            .items
            .iter()
            .filter(|d| d.is_error())
            .map(|d| d.message.clone())
            .collect();
        assert!(errors.is_empty(), "the snippet did not compile: {errors:?}");
        let entry = analysis.checked.entry.expect("the snippet exports `main`");
        let mut diags = Diagnostics::new();
        let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
        let mut program = monomorphize::run(
            &analysis.checked,
            paths,
            &mut diags,
            monomorphize::Roots::Main(entry),
        );
        crate::compiler::middle::run(&mut program, &crate::compiler::middle::Options::default());
        (program, analysis.checked.tables)
    }

    const POINT: &str = r#"
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

struct P { x: Int, y: Str }
derive Eq, Ord, Show, Hash for P;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let a = P { x: 1, y: "a" };
  let b = P { x: 2, y: "b" };
  let _ = ctx.println("${a == b}");
  let _ = ctx.println(a.show(ctx));
  let _ = ctx.println("${a.hash()}");
  let o = a.compare(b);
  let _ = ctx.println("${o == .Less}");
  .Ok(())
}
"#;

    /// A compact rendering of a generated body, for the goldens. Generated
    /// functions print by symbol so that a golden does not move when an
    /// unrelated function is added to the program.
    fn print(program: &Program, f: FuncIdx) -> String {
        let Some(func) = program.funcs.get(f.index()) else { return "<missing>".into() };
        let Some(body) = func.body() else { return format!("{} = <unbuilt>", name_of(program, f)) };
        let params: Vec<String> = func.params.iter().map(|p| format!("l{}", p.0)).collect();
        format!("{}({}) = {}", name_of(program, f), params.join(", "), sexp(program, body))
    }

    fn name_of(program: &Program, f: FuncIdx) -> String {
        match program.funcs.get(f.index()) {
            // The trailing `$<descriptor>` is an interning detail, and a golden
            // that carried it would move whenever an unrelated type was
            // described first.
            Some(func) if func.symbol.starts_with("$derive$") => {
                match func.symbol.rfind('$') {
                    Some(cut) => func.symbol.get(..cut).unwrap_or(&func.symbol).to_string(),
                    None => func.symbol.clone(),
                }
            }
            _ => format!("f{}", f.0),
        }
    }

    fn sexp(p: &Program, e: &Expr) -> String {
        let list = |xs: &[Expr]| xs.iter().map(|x| sexp(p, x)).collect::<Vec<_>>().join(", ");
        match &e.kind {
            ExprKind::Local(l) => format!("l{}", l.0),
            ExprKind::Int(v, _) => format!("{v}"),
            ExprKind::Str(s) => format!("{s:?}"),
            ExprKind::Bool(b) => format!("{b}"),
            ExprKind::Unit => "()".into(),
            ExprKind::Field { base, index } | ExprKind::TupleIndex { base, index } => {
                format!("{}.{index}", sexp(p, base))
            }
            ExprKind::CallFn { func, args } => match func.func() {
                Some(f) => format!("{}({})", name_of(p, f), list(args)),
                None => format!("?({})", list(args)),
            },
            ExprKind::FnRef(c) => match c.func() {
                Some(f) => format!("&{}", name_of(p, f)),
                None => "&?".into(),
            },
            ExprKind::Intrinsic { name, args, .. } => format!("{name}({})", list(args)),
            ExprKind::Prim { op, prim, args } => {
                format!("{op:?}<{}>({})", prim.name(), list(args))
            }
            ExprKind::And { lhs, rhs } => format!("({} && {})", sexp(p, lhs), sexp(p, rhs)),
            ExprKind::Or { lhs, rhs } => format!("({} || {})", sexp(p, lhs), sexp(p, rhs)),
            ExprKind::If { cond, then, else_ } => format!(
                "if {} {{ {} }} else {{ {} }}",
                sexp(p, cond),
                sexp(p, then),
                sexp(p, else_)
            ),
            ExprKind::Match { scrutinee, arms } => {
                let arms: Vec<String> = arms
                    .iter()
                    .map(|a| format!("{} => {}", pat(&a.pattern), sexp(p, &a.body)))
                    .collect();
                format!("match {} {{ {} }}", sexp(p, scrutinee), arms.join(", "))
            }
            ExprKind::EnumLit { variant, args, .. } => {
                if args.is_empty() {
                    format!(".v{variant}")
                } else {
                    format!(".v{variant}({})", list(args))
                }
            }
            ExprKind::StructLit { fields, .. } => format!("{{{}}}", list(fields)),
            ExprKind::Tuple(xs) => format!("({})", list(xs)),
            ExprKind::Array(xs) => format!("[{}]", list(xs)),
            ExprKind::Template { parts } => {
                let ps: Vec<String> = parts
                    .iter()
                    .map(|part| match part {
                        TemplatePart::Text(t) => format!("{t:?}"),
                        TemplatePart::Hole(h) => sexp(p, h),
                    })
                    .collect();
                format!("cat[{}]", ps.join(" "))
            }
            ExprKind::Block { stmts, tail } => {
                let mut out = String::from("{ ");
                for s in stmts {
                    match s {
                        Stmt::Let { value, .. } => {
                            out.push_str(&format!("let {}; ", sexp(p, value)));
                        }
                        Stmt::Expr(x) => out.push_str(&format!("{}; ", sexp(p, x))),
                    }
                }
                if let Some(t) = tail {
                    out.push_str(&sexp(p, t));
                }
                out.push_str(" }");
                out
            }
            other => format!("<{}>", kind_name(other)),
        }
    }

    fn kind_name(k: &ExprKind) -> &'static str {
        match k {
            ExprKind::CallValue { .. } => "callvalue",
            ExprKind::StructuralEq { .. } => "structuraleq",
            ExprKind::CtxGet { .. } => "ctxget",
            ExprKind::CtxLit { .. } => "ctxlit",
            ExprKind::Lambda { .. } => "lambda",
            _ => "other",
        }
    }

    fn pat(p: &Pattern) -> String {
        match &p.kind {
            PatKind::Wild => "_".into(),
            PatKind::Bind { local, .. } => format!("l{}", local.0),
            PatKind::Variant { variant, fields, .. } => {
                let fs: Vec<String> = fields.iter().map(|f| pat(&f.pattern)).collect();
                if fs.is_empty() {
                    format!(".v{variant}")
                } else {
                    format!(".v{variant}({})", fs.join(", "))
                }
            }
            _ => "?".into(),
        }
    }

    /// Every `structural*` intrinsic still in the program.
    fn remaining(program: &Program) -> Vec<String> {
        let mut out = Vec::new();
        for f in &program.funcs {
            let Some(body) = f.body() else { continue };
            typed::walk(body, &mut |e| {
                if let ExprKind::Intrinsic { name, .. } = &e.kind {
                    if Op::all().into_iter().any(|o| o.intrinsic() == name) {
                        out.push(name.clone());
                    }
                }
                if matches!(e.kind, ExprKind::StructuralEq { .. }) {
                    out.push("StructuralEq".into());
                }
            });
        }
        out
    }

    fn printed(program: &Program, out: &Derives, op: Op) -> Vec<String> {
        out.instances
            .iter()
            .filter(|i| i.op == op)
            .map(|i| print(program, i.func))
            .collect()
    }

    /// The call sites are gone and the functions are there instead.
    #[test]
    fn every_derive_call_site_becomes_a_direct_call() {
        let (mut program, _) = compile(POINT);
        assert!(!remaining(&program).is_empty(), "the JS-shaped program has the intrinsics");
        let out = run(&mut program);
        assert_eq!(remaining(&program), Vec::<String>::new());
        assert!(out.rewritten >= 4, "four operations at least: {}", out.rewritten);
        for i in &out.instances {
            let f = program.funcs.get(i.func.index()).expect("a generated function");
            assert!(f.body().is_some(), "{} has no body", f.symbol);
        }
    }

    /// The generated equality reads offsets, not names, and stops at the first
    /// difference.
    #[test]
    fn equality_is_a_fold_over_the_fields() {
        let (mut program, _) = compile(POINT);
        let out = run(&mut program);
        assert_eq!(
            printed(&program, &out, Op::Eq),
            vec![
                "$derive$eq$P(l0, l1) = (Eq<I64>(l0.0, l1.0) && Eq<Str>(l0.1, l1.1))",
                // `o == .Less` asks for one at `Order` too, and a payloadless
                // enum is a match on both tags.
                "$derive$eq$Order(l0, l1) = match l0 { .v0 => match l1 { .v0 => true, _ => false }, \
                 .v1 => match l1 { .v1 => true, _ => false }, \
                 .v2 => match l1 { .v2 => true, _ => false } }",
            ]
        );
    }

    /// Ordering is lexicographic, and the `.Equal` test is a match on the
    /// answer rather than a second comparison.
    #[test]
    fn ordering_is_lexicographic() {
        let (mut program, _) = compile(POINT);
        let out = run(&mut program);
        // A primitive comparison writes both operands twice, so it is inlined
        // only where writing them twice is free — which a projection of a
        // parameter is.
        assert_eq!(
            printed(&program, &out, Op::Compare),
            vec![
                "$derive$cmp$P(l0, l1) = match if Lt<I64>(l0.0, l1.0) { .v0 } \
                 else { if Gt<I64>(l0.0, l1.0) { .v2 } else { .v1 } } \
                 { .v1 => if Lt<Str>(l0.1, l1.1) { .v0 } \
                 else { if Gt<Str>(l0.1, l1.1) { .v2 } else { .v1 } }, l2 => l2 }",
            ]
        );
    }

    /// Rendering is a concatenation of literal text and rendered fields: no
    /// descriptor, and no name read at run time.
    #[test]
    fn rendering_is_a_concatenation() {
        let (mut program, _) = compile(POINT);
        let out = run(&mut program);
        assert_eq!(
            printed(&program, &out, Op::Show),
            vec![
                "$derive$show$P(l0) = cat[\"P { x: \" derivePrimShow(l0.0) \", y: \" \
                 derivePrimShow(l0.1) \" }\"]"
            ]
        );
    }

    /// Hashing threads an accumulator and mirrors `$hashInto`: the field count
    /// first, then the fields.
    #[test]
    fn hashing_threads_an_accumulator() {
        let (mut program, _) = compile(POINT);
        let out = run(&mut program);
        assert_eq!(
            printed(&program, &out, Op::Hash),
            vec![
                "$derive$hash$P(l0, l1) = derivePrimHash(derivePrimHash(derivePrimHash(l0, 2), \
                 l1.0), l1.1)",
            ]
        );
    }

    /// An enum matches on both sides for equality, and prints its variant name
    /// with the same spelling `$show` uses.
    #[test]
    fn an_enum_is_a_match_on_the_tag() {
        let src = r#"
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

enum Shape { Dot, Line(Int, Int) }
derive Eq, Show for Shape;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let a = Shape.Line(1, 2);
  let _ = ctx.println("${a == .Dot}");
  let _ = ctx.println(a.show(ctx));
  .Ok(())
}
"#;
        let (mut program, _) = compile(src);
        let out = run(&mut program);
        assert_eq!(
            printed(&program, &out, Op::Eq),
            vec![
                "$derive$eq$Shape(l0, l1) = match l0 { .v0 => match l1 { .v0 => true, _ => false }, \
                 .v1(l2, l4) => match l1 { .v1(l3, l5) => \
                 (Eq<I64>(l2, l3) && Eq<I64>(l4, l5)), _ => false } }"
            ]
        );
        assert_eq!(
            printed(&program, &out, Op::Show),
            vec![
                "$derive$show$Shape(l0) = match l0 { .v0 => \".Dot\", .v1(l1, l2) => \
                 cat[\".Line(\" derivePrimShow(l1) \", \" derivePrimShow(l2) \")\"] }"
            ]
        );
    }

    /// A list is one call to the runtime helper, handed a code pointer to the
    /// element's own generated function.
    #[test]
    fn a_list_is_the_element_function_and_a_helper() {
        let src = r#"
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

struct P { x: Int }
derive Eq, Show for P;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let xs = [P { x: 1 }];
  let _ = ctx.println("${xs == [P { x: 2 }]}");
  .Ok(())
}
"#;
        let (mut program, _) = compile(src);
        let out = run(&mut program);
        let eqs = printed(&program, &out, Op::Eq);
        assert!(
            eqs.iter().any(|s| s.contains("deriveArrayEq(l0, l1, &$derive$eq$P)")),
            "{eqs:?}"
        );
    }

    /// Two types with the same layout share the operations that do not print a
    /// name, and do not share the ones that do.
    #[test]
    fn layout_identical_types_share_the_operations_that_read_no_names() {
        let src = r#"
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

struct Meters { v: Int }
struct Seconds { v: Int }
derive Eq, Show for Meters;
derive Eq, Show for Seconds;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let a = Meters { v: 1 };
  let b = Seconds { v: 1 };
  let _ = ctx.println("${a == Meters { v: 2 }}");
  let _ = ctx.println("${b == Seconds { v: 2 }}");
  let _ = ctx.println(a.show(ctx));
  let _ = ctx.println(b.show(ctx));
  .Ok(())
}
"#;
        let (mut program, _) = compile(src);
        let out = run(&mut program);
        let eq: Vec<FuncIdx> =
            out.routes.iter().filter(|(o, _, _)| *o == Op::Eq).map(|(_, _, f)| *f).collect();
        assert_eq!(eq.len(), 2, "two descriptors ask for equality");
        assert_eq!(eq.first(), eq.get(1), "and one function answers both");
        let show: Vec<FuncIdx> =
            out.routes.iter().filter(|(o, _, _)| *o == Op::Show).map(|(_, _, f)| *f).collect();
        assert_eq!(show.len(), 2);
        assert_ne!(show.first(), show.get(1), "rendering prints the name, so it does not share");
    }

    /// A type that contains itself terminates, because the function is claimed
    /// before its body is built.
    #[test]
    fn a_recursive_type_generates_a_recursive_function() {
        let src = r#"
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

enum Rose { Leaf(Int), Node([Rose]) }
derive Eq for Rose;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let a = Rose.Node([Rose.Leaf(1)]);
  let _ = ctx.println("${a == Rose.Leaf(2)}");
  .Ok(())
}
"#;
        let (mut program, _) = compile(src);
        let out = run(&mut program);
        assert!(
            out.instances.iter().any(|i| i.op == Op::Eq && i.shape.contains('^')),
            "the shape key closes its own cycle"
        );
        let all: Vec<String> = out.instances.iter().map(|i| print(&program, i.func)).collect();
        // `Rose` calls the list's function, and the list's function calls back
        // into `Rose` — the cycle the reserved slot exists for.
        assert!(
            all.iter().any(|s| s.starts_with("$derive$eq$Rose") && s.contains("$derive$eq$list")),
            "{all:?}"
        );
        assert!(
            all.iter()
                .any(|s| s.starts_with("$derive$eq$list") && s.contains("&$derive$eq$Rose")),
            "{all:?}"
        );
    }

    /// `FromJson` is reported rather than generated, and the descriptor it
    /// needs stays where the intrinsic can find it.
    #[test]
    fn from_json_is_recorded_as_a_seam() {
        let src = r#"
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/json" import { DecodeError, ToJson, FromJson };
from "core/json" import * as json;

struct P { x: Int }
derive Eq, ToJson, FromJson for P;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let p = P { x: 1 };
  let back: Result<P, DecodeError> = json.decode(ctx, json.encode(ctx, p));
  let _ = ctx.println("${back == .Ok(p)}");
  .Ok(())
}
"#;
        let (mut program, _) = compile(src);
        let out = run(&mut program);
        assert_eq!(out.from_json.len(), 1, "one type is decoded");
        let json_fns = printed(&program, &out, Op::ToJson);
        assert!(
            json_fns.iter().any(|s| s.contains("derivePrimJson")),
            "encoding is generated: {json_fns:?}"
        );
        // The record becomes an object of one member, keyed by the field name.
        assert!(json_fns.iter().any(|s| s.contains("\"x\"")), "{json_fns:?}");
    }

    /// A derive inside a tail-recursive body is inside an `ExprKind::Loop` by
    /// the time this pass runs, and the rewrite has to reach it: one left
    /// behind arrives at `lower` as an `Inst::Structural`, which is the
    /// placeholder for the thing this pass was supposed to have replaced.
    #[test]
    fn a_call_site_inside_a_loop_is_rewritten_too() {
        let src = r#"
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

struct P { x: Int }
derive Eq, Show for P;

export fn seek(n: Int, needle: P): Int {
  if (n <= 0) {
    0
  } else if (P { x: n } == needle) {
    n
  } else {
    seek(n - 1, needle)
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = ctx.println("${seek(3, P { x: 2 })}");
  .Ok(())
}
"#;
        let (mut program, _) = compile(src);
        crate::compiler::middle::tail_calls::rewrite(&mut program);
        let looped = program
            .funcs
            .iter()
            .filter_map(|f| f.body())
            .any(|b| matches!(b.kind, ExprKind::Loop { .. }));
        assert!(looped, "`tail_calls` made a loop of it");
        assert!(!remaining(&program).is_empty(), "and the derive is inside it");
        let out = run(&mut program);
        assert_eq!(remaining(&program), Vec::<String>::new());
        assert!(out.rewritten > 0);
    }

    /// The JavaScript path is the program *before* this pass, and it still has
    /// the descriptor walk in it. If this fails, the branch in `middle::mod`
    /// has been crossed and the JavaScript goldens are about to move.
    #[test]
    fn the_js_path_still_carries_descriptor_walks() {
        let (mut program, tables) = compile(POINT);
        let js = crate::compiler::backend::js::generate::generate(
            &program,
            &tables,
            crate::compiler::backend::Profile::Debug,
        );
        let code = crate::compiler::backend::js::javascript::print(&js.stmts, true);
        assert!(code.contains("$D0"), "the descriptor table is still emitted");
        assert!(!code.contains("$derive$"), "and nothing generated is in it");
        // Running the native pass afterwards must not be what a JavaScript
        // build did: it is a different branch of the pipeline entirely.
        let out = run(&mut program);
        assert!(!out.instances.is_empty());
    }
}
