//! **The match generator.** A seeded writer of whole programs made of `match`
//! expressions, each with the output it must print worked out here rather than
//! by running it.
//!
//! One call to [`batch`] writes one program: a handful of type declarations, a
//! few helpers, [`CASES`] functions that each hold one `match`, and a `main`
//! that calls every function with two or three values and prints what it
//! answers. The generator keeps a model of every value and every pattern, so
//! it knows which arm each call takes and what that arm prints. A backend that
//! agrees with the others on the wrong arm still fails.
//!
//! # What a program is made of
//!
//! * **Types.** Enums whose variants carry nothing, a tuple payload or a record
//!   payload, and structs, each built from `Int`, `Str`, `Bool`, lists, tuples,
//!   `Option`, `Result` and the types declared before it.
//! * **Patterns**, drawn from the values the function is called with: literals
//!   (`Int`, `Str`, `Bool`) beside bound fields, wildcards, `..` in records,
//!   payloads and lists, `..name` list tails, `@` bindings and or-patterns. Many
//!   arms share a variant, because they are drawn from values that share one.
//! * **Guards** reading one binding or several, the function's `limit`
//!   parameter, and method calls (`length`, `isEmpty`, `isSome`, `startsWith`).
//! * **Bodies** that print the arm's label and the leaf values it bound, a
//!   static string where it bound none, or a nested `match` on a compound
//!   binding inside a block.
//! * **Scrutinees** passed in directly, through a generic helper, or built by a
//!   lambda whose result type is only inferred.
//! * **Uses**: a match as a function's result, bound by `let` and read after,
//!   or used as a statement whose arms print.
//! * **Heap values.** Strings are built by `str.format` and lists by `push` as
//!   often as they are written as literals, so the native heap check sees
//!   values the match owns.
//!
//! The model also decides which arms are reachable and whether the arms are
//! exhaustive, by the usefulness algorithm in Maranget's *Warnings for pattern
//! matching*. An arm the model finds unreachable is dropped and a match it
//! finds open gets a last arm, so every program is one the front end should
//! accept.

use std::fmt::Write as _;

/// The seed the checked-in corpus was written from.
pub const SEED: u64 = 0x6D61_7463_6865_7301;

/// How many programs the checked-in corpus holds.
pub const BATCHES: usize = 24;

/// How many functions, each holding one `match`, one program holds.
const CASES: usize = 16;

/// One generated program and what it prints.
pub struct Batch {
    pub source: String,
    pub expected: String,
}

/// SplitMix64, as `fuzz.rs` and `benches/generate.rs` use it: no dependency,
/// and the same sequence on every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }

    /// True `percent` times in a hundred.
    fn percent(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Debug)]
enum Ty {
    Int,
    Str,
    Bool,
    List(Box<Ty>),
    Tuple(Vec<Ty>),
    Option(Box<Ty>),
    Result(Box<Ty>, Box<Ty>),
    Struct(usize),
    Enum(usize),
}

enum Payload {
    Unit,
    Tuple(Vec<Ty>),
    Record(Vec<(String, Ty)>),
}

impl Payload {
    fn tys(&self) -> Vec<Ty> {
        match self {
            Payload::Unit => Vec::new(),
            Payload::Tuple(tys) => tys.clone(),
            Payload::Record(fields) => fields.iter().map(|(_, t)| t.clone()).collect(),
        }
    }
}

struct EnumDef {
    name: String,
    variants: Vec<(String, Payload)>,
}

struct StructDef {
    name: String,
    fields: Vec<(String, Ty)>,
}

/// Every type one program declares. A declaration only names types declared
/// before it, so none is recursive.
#[derive(Default)]
struct World {
    enums: Vec<EnumDef>,
    structs: Vec<StructDef>,
}

const ENUM_NAMES: &[&str] = &["Shape", "Token", "Signal", "Packet", "Reading"];
const STRUCT_NAMES: &[&str] = &["Point", "Entry", "Frame", "Badge", "Slot"];
const VARIANT_NAMES: &[&str] = &[
    "Alpha", "Bravo", "Delta", "Echo", "Foxtrot", "Golf", "Hotel", "India", "Juliet", "Kilo",
    "Lima", "Mike", "November", "Oscar", "Papa", "Quebec", "Romeo", "Sierra", "Tango",
    "Uniform",
];

impl World {
    fn text(&self, ty: &Ty) -> String {
        match ty {
            Ty::Int => String::from("Int"),
            Ty::Str => String::from("Str"),
            Ty::Bool => String::from("Bool"),
            Ty::List(t) => format!("[{}]", self.text(t)),
            Ty::Tuple(ts) => {
                format!("({})", ts.iter().map(|t| self.text(t)).collect::<Vec<_>>().join(", "))
            }
            Ty::Option(t) => format!("Option<{}>", self.text(t)),
            Ty::Result(t, e) => format!("Result<{}, {}>", self.text(t), self.text(e)),
            Ty::Struct(i) => self.structs[*i].name.clone(),
            Ty::Enum(i) => self.enums[*i].name.clone(),
        }
    }

    fn leaf(rng: &mut Rng) -> Ty {
        match rng.below(10) {
            0..=4 => Ty::Int,
            5..=7 => Ty::Str,
            _ => Ty::Bool,
        }
    }

    /// How many constructors deep a type is: a leaf is 0, a list 1.
    fn depth(&self, ty: &Ty) -> usize {
        match ty {
            Ty::Int | Ty::Str | Ty::Bool => 0,
            Ty::List(t) | Ty::Option(t) => 1 + self.depth(t),
            Ty::Tuple(ts) => 1 + ts.iter().map(|t| self.depth(t)).max().unwrap_or(0),
            Ty::Result(t, e) => 1 + self.depth(t).max(self.depth(e)),
            Ty::Struct(i) => {
                1 + self.structs[*i].fields.iter().map(|(_, t)| self.depth(t)).max().unwrap_or(0)
            }
            Ty::Enum(i) => {
                1 + self.enums[*i]
                    .variants
                    .iter()
                    .flat_map(|(_, p)| p.tys())
                    .map(|t| self.depth(&t))
                    .max()
                    .unwrap_or(0)
            }
        }
    }

    /// A type to put in a field, a payload or a scrutinee, at most `depth`
    /// constructors deep.
    fn draw(&self, rng: &mut Rng, depth: usize) -> Ty {
        if depth <= 1 || rng.percent(30) {
            return if depth >= 1 && rng.percent(12) {
                Ty::List(Box::new(if rng.percent(50) { Ty::Int } else { Ty::Str }))
            } else {
                World::leaf(rng)
            };
        }
        let enums: Vec<usize> =
            (0..self.enums.len()).filter(|i| self.depth(&Ty::Enum(*i)) <= depth).collect();
        let structs: Vec<usize> =
            (0..self.structs.len()).filter(|i| self.depth(&Ty::Struct(*i)) <= depth).collect();
        loop {
            match rng.below(7) {
                0 => {
                    return Ty::List(Box::new(if rng.percent(60) { Ty::Int } else { Ty::Str }))
                }
                1 => {
                    let n = 2 + rng.below(2);
                    return Ty::Tuple((0..n).map(|_| self.draw(rng, depth - 1)).collect());
                }
                2 => return Ty::Option(Box::new(self.draw(rng, depth - 1))),
                3 => {
                    return Ty::Result(
                        Box::new(self.draw(rng, depth - 1)),
                        Box::new(self.draw(rng, depth - 1)),
                    )
                }
                4 | 5 if !enums.is_empty() => return Ty::Enum(*rng.pick(&enums)),
                6 if !structs.is_empty() => return Ty::Struct(*rng.pick(&structs)),
                _ => {}
            }
        }
    }

    /// Field names for `tys`, distinct within the declaration and named for
    /// what they hold.
    fn field_names(rng: &mut Rng, tys: &[Ty]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for t in tys {
            let pool: &[&str] = match t {
                Ty::Int => &["count", "code", "size", "rank"],
                Ty::Str => &["name", "label", "text"],
                Ty::Bool => &["flag", "open", "done"],
                Ty::List(_) => &["items", "tags", "parts"],
                _ => &["inner", "body", "head", "held"],
            };
            let base = *rng.pick(pool);
            let mut name = String::from(base);
            let mut k = 2;
            while out.contains(&name) {
                name = format!("{base}{k}");
                k += 1;
            }
            out.push(name);
        }
        out
    }

    fn declare(rng: &mut Rng) -> World {
        let mut w = World::default();
        let mut variant_names: Vec<&str> = VARIANT_NAMES.to_vec();
        let mut take_variant = |rng: &mut Rng| {
            let i = rng.below(variant_names.len());
            variant_names.remove(i).to_string()
        };
        let order = [true, false, true, false, true];
        for (k, is_enum) in order.iter().enumerate() {
            if *is_enum {
                let count = 2 + rng.below(3);
                let mut variants = Vec::new();
                for _ in 0..count {
                    let name = take_variant(rng);
                    let payload = match rng.below(10) {
                        0..=1 => Payload::Unit,
                        2..=6 => {
                            let n = 1 + rng.below(3);
                            Payload::Tuple((0..n).map(|_| w.draw(rng, 2)).collect())
                        }
                        _ => {
                            let n = 1 + rng.below(3);
                            let tys: Vec<Ty> = (0..n).map(|_| w.draw(rng, 2)).collect();
                            let names = World::field_names(rng, &tys);
                            Payload::Record(names.into_iter().zip(tys).collect())
                        }
                    };
                    variants.push((name, payload));
                }
                w.enums.push(EnumDef { name: ENUM_NAMES[k / 2].to_string(), variants });
            } else {
                let n = 1 + rng.below(3);
                let tys: Vec<Ty> = (0..n).map(|_| w.draw(rng, 2)).collect();
                let names = World::field_names(rng, &tys);
                w.structs.push(StructDef {
                    name: STRUCT_NAMES[k / 2].to_string(),
                    fields: names.into_iter().zip(tys).collect(),
                });
            }
        }
        w
    }

    fn declarations(&self) -> String {
        let mut out = String::new();
        // In declaration order, enums and structs interleaved as `declare`
        // made them, so a reader meets a type before its first use.
        let (mut e, mut s) = (0, 0);
        while e < self.enums.len() || s < self.structs.len() {
            if e < self.enums.len() && (e <= s || s >= self.structs.len()) {
                let def = &self.enums[e];
                let _ = writeln!(out, "enum {} {{", def.name);
                for (name, payload) in &def.variants {
                    match payload {
                        Payload::Unit => {
                            let _ = writeln!(out, "    {name},");
                        }
                        Payload::Tuple(tys) => {
                            let inner: Vec<String> = tys.iter().map(|t| self.text(t)).collect();
                            let _ = writeln!(out, "    {name}({}),", inner.join(", "));
                        }
                        Payload::Record(fields) => {
                            let inner: Vec<String> =
                                fields.iter().map(|(f, t)| format!("{f}: {}", self.text(t))).collect();
                            let _ = writeln!(out, "    {name} {{ {} }},", inner.join(", "));
                        }
                    }
                }
                out.push_str("}\n\n");
                e += 1;
            } else {
                let def = &self.structs[s];
                let inner: Vec<String> =
                    def.fields.iter().map(|(f, t)| format!("{f}: {}", self.text(t))).collect();
                let _ = writeln!(out, "struct {} {{ {} }}\n", def.name, inner.join(", "));
                s += 1;
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Debug)]
enum Val {
    Int(i64),
    Str(String),
    Bool(bool),
    List(Vec<Val>),
    Tuple(Vec<Val>),
    Some(Box<Val>),
    None,
    Ok(Box<Val>),
    Err(Box<Val>),
    Struct(Vec<Val>),
    Variant(usize, Vec<Val>),
}

/// Strings end in a digit so a heap copy can be built by `str.format` with the
/// digit interpolated, which is a fresh block rather than a static one.
const STRS: &[&str] = &["a1", "b2", "ab3", "cd4", "e5", ""];
const INTS: &[i64] = &[-2, 0, 0, 1, 1, 2, 3, 3, 5, 8, 13];

impl World {
    fn value(&self, rng: &mut Rng, ty: &Ty) -> Val {
        match ty {
            Ty::Int => Val::Int(*rng.pick(INTS)),
            Ty::Str => Val::Str(rng.pick(STRS).to_string()),
            Ty::Bool => Val::Bool(rng.percent(50)),
            Ty::List(t) => {
                let n = rng.below(4);
                Val::List((0..n).map(|_| self.value(rng, t)).collect())
            }
            Ty::Tuple(ts) => Val::Tuple(ts.iter().map(|t| self.value(rng, t)).collect()),
            Ty::Option(t) => {
                if rng.percent(25) {
                    Val::None
                } else {
                    Val::Some(Box::new(self.value(rng, t)))
                }
            }
            Ty::Result(t, e) => {
                if rng.percent(35) {
                    Val::Err(Box::new(self.value(rng, e)))
                } else {
                    Val::Ok(Box::new(self.value(rng, t)))
                }
            }
            Ty::Struct(i) => {
                Val::Struct(self.structs[*i].fields.iter().map(|(_, t)| self.value(rng, t)).collect())
            }
            Ty::Enum(i) => {
                let def = &self.enums[*i];
                let v = rng.below(def.variants.len());
                let tys = def.variants[v].1.tys();
                Val::Variant(v, tys.iter().map(|t| self.value(rng, t)).collect())
            }
        }
    }

    /// `v` with one piece somewhere inside it drawn again, so the values a
    /// function is called with mostly share their outer shape and differ
    /// below it — which is what puts several arms on one variant.
    fn mutate(&self, rng: &mut Rng, ty: &Ty, v: &Val) -> Val {
        if rng.percent(25) {
            return self.value(rng, ty);
        }
        match (ty, v) {
            (Ty::List(t), Val::List(xs)) if !xs.is_empty() && rng.percent(70) => {
                let mut xs = xs.clone();
                let i = rng.below(xs.len());
                if rng.percent(30) {
                    xs.remove(i);
                } else {
                    xs[i] = self.mutate(rng, t, &xs[i]);
                }
                Val::List(xs)
            }
            (Ty::Tuple(ts), Val::Tuple(xs)) => {
                let mut xs = xs.clone();
                let i = rng.below(xs.len());
                xs[i] = self.mutate(rng, &ts[i], &xs[i]);
                Val::Tuple(xs)
            }
            (Ty::Option(t), Val::Some(x)) => Val::Some(Box::new(self.mutate(rng, t, x))),
            (Ty::Result(t, _), Val::Ok(x)) => Val::Ok(Box::new(self.mutate(rng, t, x))),
            (Ty::Result(_, e), Val::Err(x)) => Val::Err(Box::new(self.mutate(rng, e, x))),
            (Ty::Struct(i), Val::Struct(xs)) => {
                let fields = &self.structs[*i].fields;
                let mut xs = xs.clone();
                let k = rng.below(xs.len());
                xs[k] = self.mutate(rng, &fields[k].1, &xs[k]);
                Val::Struct(xs)
            }
            (Ty::Enum(i), Val::Variant(vi, xs)) if !xs.is_empty() => {
                let tys = self.enums[*i].variants[*vi].1.tys();
                let mut xs = xs.clone();
                let k = rng.below(xs.len());
                xs[k] = self.mutate(rng, &tys[k], &xs[k]);
                Val::Variant(*vi, xs)
            }
            _ => self.value(rng, ty),
        }
    }

    /// `v` as a Buri expression, in a scope holding `ctx: C` with
    /// `C: Allocator`. Heap and static spellings are drawn independently at
    /// every string and list.
    fn expr(&self, rng: &mut Rng, ty: &Ty, v: &Val) -> String {
        match (ty, v) {
            (_, Val::Int(n)) => n.to_string(),
            (_, Val::Str(s)) => {
                if !s.is_empty() && rng.percent(50) {
                    let (head, digit) = s.split_at(s.len() - 1);
                    format!("str.format(ctx, \"{head}${{{digit}}}\")")
                } else {
                    format!("\"{s}\"")
                }
            }
            (_, Val::Bool(b)) => b.to_string(),
            (Ty::List(t), Val::List(xs)) => {
                if xs.is_empty() {
                    format!("list.empty<{}>()", self.text(t))
                } else if rng.percent(50) {
                    let items: Vec<String> = xs.iter().map(|x| self.expr(rng, t, x)).collect();
                    format!("[{}]", items.join(", "))
                } else {
                    let mut out = format!("list.empty<{}>()", self.text(t));
                    for x in xs {
                        let item = self.expr(rng, t, x);
                        let _ = write!(out, ".push(ctx, {item})");
                    }
                    out
                }
            }
            (Ty::Tuple(ts), Val::Tuple(xs)) => {
                let items: Vec<String> =
                    ts.iter().zip(xs).map(|(t, x)| self.expr(rng, t, x)).collect();
                format!("({})", items.join(", "))
            }
            (Ty::Option(t), Val::Some(x)) => format!("Option.Some({})", self.expr(rng, t, x)),
            (Ty::Option(_), Val::None) => String::from("Option.None"),
            (Ty::Result(t, _), Val::Ok(x)) => format!("Result.Ok({})", self.expr(rng, t, x)),
            (Ty::Result(_, e), Val::Err(x)) => format!("Result.Err({})", self.expr(rng, e, x)),
            (Ty::Struct(i), Val::Struct(xs)) => {
                let def = &self.structs[*i];
                let fields: Vec<String> = def
                    .fields
                    .iter()
                    .zip(xs)
                    .map(|((f, t), x)| format!("{f}: {}", self.expr(rng, t, x)))
                    .collect();
                format!("{} {{ {} }}", def.name, fields.join(", "))
            }
            (Ty::Enum(i), Val::Variant(vi, xs)) => {
                let def = &self.enums[*i];
                let (name, payload) = &def.variants[*vi];
                match payload {
                    Payload::Unit => format!("{}.{name}", def.name),
                    Payload::Tuple(tys) => {
                        let items: Vec<String> =
                            tys.iter().zip(xs).map(|(t, x)| self.expr(rng, t, x)).collect();
                        format!("{}.{name}({})", def.name, items.join(", "))
                    }
                    Payload::Record(fields) => {
                        let items: Vec<String> = fields
                            .iter()
                            .zip(xs)
                            .map(|((f, t), x)| format!("{f}: {}", self.expr(rng, t, x)))
                            .collect();
                        format!("{}.{name} {{ {} }}", def.name, items.join(", "))
                    }
                }
            }
            _ => unreachable!("a value that does not have its type: {v:?} : {ty:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Patterns
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Pat {
    Wild,
    Bind(String),
    Int(i64),
    Str(String),
    Bool(bool),
    Tuple(Vec<Pat>),
    /// The fields named, by index, and whether the pattern ends in `..`.
    Struct(usize, Vec<(usize, Pat)>, bool),
    Variant(usize, usize, VPat),
    Some(Box<Pat>),
    None,
    Ok(Box<Pat>),
    Err(Box<Pat>),
    List(Vec<Pat>, Rest),
    Or(Vec<Pat>),
    At(String, Box<Pat>),
}

#[derive(Clone, Debug)]
enum VPat {
    Unit,
    /// One pattern per payload value: the grammar has no `..` here.
    Tuple(Vec<Pat>),
    Record(Vec<(usize, Pat)>, bool),
}

#[derive(Clone, Debug)]
enum Rest {
    Exact,
    Anonymous,
    Named(String),
}

impl World {
    fn pat_text(&self, p: &Pat) -> String {
        match p {
            Pat::Wild => String::from("_"),
            Pat::Bind(n) => n.clone(),
            Pat::Int(n) => n.to_string(),
            Pat::Str(s) => format!("\"{s}\""),
            Pat::Bool(b) => b.to_string(),
            Pat::Tuple(ps) => {
                format!("({})", ps.iter().map(|p| self.pat_text(p)).collect::<Vec<_>>().join(", "))
            }
            Pat::Struct(i, fields, rest) => {
                let def = &self.structs[*i];
                let mut items: Vec<String> = fields
                    .iter()
                    .map(|(f, p)| format!("{}: {}", def.fields[*f].0, self.pat_text(p)))
                    .collect();
                if *rest {
                    items.push(String::from(".."));
                }
                format!("{} {{ {} }}", def.name, items.join(", "))
            }
            Pat::Variant(e, v, payload) => {
                let def = &self.enums[*e];
                let (name, decl) = &def.variants[*v];
                let head = format!(".{name}");
                match payload {
                    VPat::Unit => head,
                    VPat::Tuple(ps) => {
                        let items: Vec<String> = ps.iter().map(|p| self.pat_text(p)).collect();
                        format!("{head}({})", items.join(", "))
                    }
                    VPat::Record(fields, rest) => {
                        let Payload::Record(decl) = decl else { unreachable!() };
                        let mut items: Vec<String> = fields
                            .iter()
                            .map(|(f, p)| format!("{}: {}", decl[*f].0, self.pat_text(p)))
                            .collect();
                        if *rest {
                            items.push(String::from(".."));
                        }
                        format!("{head} {{ {} }}", items.join(", "))
                    }
                }
            }
            Pat::Some(p) => format!(".Some({})", self.pat_text(p)),
            Pat::None => String::from(".None"),
            Pat::Ok(p) => format!(".Ok({})", self.pat_text(p)),
            Pat::Err(p) => format!(".Err({})", self.pat_text(p)),
            Pat::List(ps, rest) => {
                let mut items: Vec<String> = ps.iter().map(|p| self.pat_text(p)).collect();
                match rest {
                    Rest::Exact => {}
                    Rest::Anonymous => items.push(String::from("..")),
                    Rest::Named(n) => items.push(format!("..{n}")),
                }
                format!("[{}]", items.join(", "))
            }
            Pat::Or(ps) => ps.iter().map(|p| self.pat_text(p)).collect::<Vec<_>>().join(" | "),
            Pat::At(n, p) => format!("{n} @ {}", self.pat_text(p)),
        }
    }
}

/// Does `p` match `v`, and if so, what does it bind.
fn bind(p: &Pat, v: &Val, env: &mut Vec<(String, Val)>) -> bool {
    match (p, v) {
        (Pat::Wild, _) => true,
        (Pat::Bind(n), v) => {
            env.push((n.clone(), v.clone()));
            true
        }
        (Pat::Int(a), Val::Int(b)) => a == b,
        (Pat::Str(a), Val::Str(b)) => a == b,
        (Pat::Bool(a), Val::Bool(b)) => a == b,
        (Pat::Tuple(ps), Val::Tuple(vs)) => ps.iter().zip(vs).all(|(p, v)| bind(p, v, env)),
        (Pat::Struct(_, fields, _), Val::Struct(vs)) => {
            fields.iter().all(|(f, p)| bind(p, &vs[*f], env))
        }
        (Pat::Variant(_, want, payload), Val::Variant(got, vs)) => {
            want == got
                && match payload {
                    VPat::Unit => true,
                    VPat::Tuple(ps) => ps.iter().zip(vs).all(|(p, v)| bind(p, v, env)),
                    VPat::Record(fields, _) => fields.iter().all(|(f, p)| bind(p, &vs[*f], env)),
                }
        }
        (Pat::Some(p), Val::Some(v)) | (Pat::Ok(p), Val::Ok(v)) | (Pat::Err(p), Val::Err(v)) => {
            bind(p, v, env)
        }
        (Pat::None, Val::None) => true,
        (Pat::List(ps, rest), Val::List(vs)) => {
            let fits = match rest {
                Rest::Exact => vs.len() == ps.len(),
                _ => vs.len() >= ps.len(),
            };
            if !fits || !ps.iter().zip(vs).all(|(p, v)| bind(p, v, env)) {
                return false;
            }
            if let Rest::Named(n) = rest {
                env.push((n.clone(), Val::List(vs[ps.len()..].to_vec())));
            }
            true
        }
        (Pat::Or(ps), v) => ps.iter().any(|p| {
            let mut scratch = Vec::new();
            bind(p, v, &mut scratch)
        }),
        (Pat::At(n, p), v) => {
            if bind(p, v, env) {
                env.push((n.clone(), v.clone()));
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

/// The names `p` binds and their types, in the order they appear.
fn bound(w: &World, p: &Pat, ty: &Ty, out: &mut Vec<(String, Ty)>) {
    match (p, ty) {
        (Pat::Bind(n), t) => out.push((n.clone(), t.clone())),
        (Pat::Tuple(ps), Ty::Tuple(ts)) => {
            for (p, t) in ps.iter().zip(ts) {
                bound(w, p, t, out);
            }
        }
        (Pat::Struct(i, fields, _), _) => {
            for (f, p) in fields {
                bound(w, p, &w.structs[*i].fields[*f].1, out);
            }
        }
        (Pat::Variant(e, v, payload), _) => {
            let tys = w.enums[*e].variants[*v].1.tys();
            match payload {
                VPat::Unit => {}
                VPat::Tuple(ps) => {
                    for (p, t) in ps.iter().zip(&tys) {
                        bound(w, p, t, out);
                    }
                }
                VPat::Record(fields, _) => {
                    for (f, p) in fields {
                        bound(w, p, &tys[*f], out);
                    }
                }
            }
        }
        (Pat::Some(p), Ty::Option(t)) | (Pat::Ok(p), Ty::Result(t, _)) => bound(w, p, t, out),
        (Pat::Err(p), Ty::Result(_, e)) => bound(w, p, e, out),
        (Pat::List(ps, rest), Ty::List(t)) => {
            for p in ps {
                bound(w, p, t, out);
            }
            if let Rest::Named(n) = rest {
                out.push((n.clone(), ty.clone()));
            }
        }
        (Pat::At(n, p), t) => {
            bound(w, p, t, out);
            out.push((n.clone(), t.clone()));
        }
        _ => {}
    }
}

/// Draws patterns, handing out binding names that are fresh in the function.
struct Patterns<'a> {
    w: &'a World,
    next: &'a mut usize,
}

impl Patterns<'_> {
    fn fresh(&mut self) -> String {
        let n = format!("b{}", *self.next);
        *self.next += 1;
        n
    }

    /// A pattern for a value of type `ty`, drawn from `v`: it usually matches
    /// `v`, and where it does not, it is because a literal in it was drawn to
    /// differ or a variant swapped. With `binds` false it binds nothing, which
    /// is what an or-pattern's alternatives need.
    fn from(&mut self, rng: &mut Rng, ty: &Ty, v: &Val, depth: usize, binds: bool) -> Pat {
        let roll = rng.below(100);
        if roll < 14 {
            return Pat::Wild;
        }
        if roll < 34 && binds {
            return Pat::Bind(self.fresh());
        }
        if depth == 0 {
            return if binds { Pat::Bind(self.fresh()) } else { Pat::Wild };
        }
        if roll < 40 && binds && !matches!(ty, Ty::Int | Ty::Str | Ty::Bool) {
            let inner = self.structural(rng, ty, v, depth, binds);
            let name = self.fresh();
            return Pat::At(name, Box::new(inner));
        }
        if roll < 46 {
            let a = self.structural(rng, ty, v, depth, false);
            let other = self.w.value(rng, ty);
            let b = self.structural(rng, ty, &other, depth, false);
            return Pat::Or(vec![a, b]);
        }
        self.structural(rng, ty, v, depth, binds)
    }

    fn structural(&mut self, rng: &mut Rng, ty: &Ty, v: &Val, depth: usize, binds: bool) -> Pat {
        let w = self.w;
        match (ty, v) {
            (Ty::Int, Val::Int(n)) => {
                Pat::Int(if rng.percent(75) { *n } else { *rng.pick(INTS) })
            }
            (Ty::Str, Val::Str(s)) => {
                Pat::Str(if rng.percent(75) { s.clone() } else { rng.pick(STRS).to_string() })
            }
            (Ty::Bool, Val::Bool(b)) => Pat::Bool(if rng.percent(80) { *b } else { !*b }),
            (Ty::List(t), Val::List(xs)) => {
                if xs.is_empty() || rng.percent(40) {
                    let ps = xs.iter().map(|x| self.from(rng, t, x, depth - 1, binds)).collect();
                    Pat::List(ps, Rest::Exact)
                } else {
                    let k = 1 + rng.below(xs.len());
                    let ps =
                        xs[..k].iter().map(|x| self.from(rng, t, x, depth - 1, binds)).collect();
                    let rest = if binds && rng.percent(50) {
                        Rest::Named(self.fresh())
                    } else {
                        Rest::Anonymous
                    };
                    Pat::List(ps, rest)
                }
            }
            (Ty::Tuple(ts), Val::Tuple(xs)) => Pat::Tuple(
                ts.iter().zip(xs).map(|(t, x)| self.from(rng, t, x, depth - 1, binds)).collect(),
            ),
            (Ty::Option(t), Val::Some(x)) => {
                if rng.percent(10) {
                    Pat::None
                } else {
                    Pat::Some(Box::new(self.from(rng, t, x, depth - 1, binds)))
                }
            }
            (Ty::Option(t), Val::None) => {
                if rng.percent(15) {
                    let x = w.value(rng, t);
                    Pat::Some(Box::new(self.from(rng, t, &x, depth - 1, binds)))
                } else {
                    Pat::None
                }
            }
            (Ty::Result(t, _), Val::Ok(x)) => Pat::Ok(Box::new(self.from(rng, t, x, depth - 1, binds))),
            (Ty::Result(_, e), Val::Err(x)) => {
                Pat::Err(Box::new(self.from(rng, e, x, depth - 1, binds)))
            }
            (Ty::Struct(i), Val::Struct(xs)) => {
                let fields = &w.structs[*i].fields;
                let (named, rest) = self.some_fields(rng, fields.len());
                let ps = named
                    .into_iter()
                    .map(|f| (f, self.from(rng, &fields[f].1, &xs[f], depth - 1, binds)))
                    .collect();
                Pat::Struct(*i, ps, rest)
            }
            (Ty::Enum(e), Val::Variant(vi, xs)) => {
                // Now and then a pattern for another variant of the same enum,
                // drawn from a value of that variant.
                if rng.percent(15) {
                    let other = w.value(rng, ty);
                    if let Val::Variant(oi, oxs) = &other {
                        if oi != vi {
                            return self.variant(rng, *e, *oi, oxs, depth, binds);
                        }
                    }
                }
                self.variant(rng, *e, *vi, xs, depth, binds)
            }
            _ => unreachable!("a value that does not have its type: {v:?} : {ty:?}"),
        }
    }

    fn variant(
        &mut self,
        rng: &mut Rng,
        e: usize,
        vi: usize,
        xs: &[Val],
        depth: usize,
        binds: bool,
    ) -> Pat {
        let w = self.w;
        let payload = match &w.enums[e].variants[vi].1 {
            Payload::Unit => VPat::Unit,
            Payload::Tuple(tys) => VPat::Tuple(
                tys.iter().zip(xs).map(|(t, x)| self.from(rng, t, x, depth - 1, binds)).collect(),
            ),
            Payload::Record(fields) => {
                let (named, rest) = self.some_fields(rng, fields.len());
                let ps = named
                    .into_iter()
                    .map(|f| (f, self.from(rng, &fields[f].1, &xs[f], depth - 1, binds)))
                    .collect();
                VPat::Record(ps, rest)
            }
        };
        Pat::Variant(e, vi, payload)
    }

    /// Which fields of a record a pattern names, in declaration order, and
    /// whether it ends in `..` for the rest.
    fn some_fields(&mut self, rng: &mut Rng, count: usize) -> (Vec<usize>, bool) {
        if rng.percent(65) {
            return ((0..count).collect(), false);
        }
        let named: Vec<usize> = (0..count).filter(|_| rng.percent(50)).collect();
        (named, true)
    }

    /// Patterns that between them match every value of `ty`, binding a name
    /// here and there: what a match that names its cases instead of ending in
    /// `_` is made of.
    fn cover(&mut self, rng: &mut Rng, ty: &Ty, depth: usize) -> Vec<Pat> {
        let w = self.w;
        let loose = |s: &mut Self, rng: &mut Rng| {
            if rng.percent(40) {
                Pat::Bind(s.fresh())
            } else {
                Pat::Wild
            }
        };
        if depth == 0 {
            return vec![loose(self, rng)];
        }
        match ty {
            Ty::Bool => vec![Pat::Bool(true), Pat::Bool(false)],
            Ty::Option(t) => {
                let mut out: Vec<Pat> = if rng.percent(40) {
                    self.cover(rng, t, depth - 1).into_iter().map(|p| Pat::Some(Box::new(p))).collect()
                } else {
                    vec![Pat::Some(Box::new(loose(self, rng)))]
                };
                out.push(Pat::None);
                out
            }
            Ty::Result(t, _) => {
                let mut out: Vec<Pat> = if rng.percent(40) {
                    self.cover(rng, t, depth - 1).into_iter().map(|p| Pat::Ok(Box::new(p))).collect()
                } else {
                    vec![Pat::Ok(Box::new(loose(self, rng)))]
                };
                out.push(Pat::Err(Box::new(loose(self, rng))));
                out
            }
            Ty::List(_) => {
                let rest = if rng.percent(50) { Rest::Named(self.fresh()) } else { Rest::Anonymous };
                vec![Pat::List(Vec::new(), Rest::Exact), Pat::List(vec![loose(self, rng)], rest)]
            }
            Ty::Enum(e) => {
                let def = &w.enums[*e];
                let mut out = Vec::new();
                for (vi, (_, payload)) in def.variants.iter().enumerate() {
                    let pv = match payload {
                        Payload::Unit => VPat::Unit,
                        Payload::Tuple(tys) => {
                            VPat::Tuple(tys.iter().map(|_| loose(self, rng)).collect())
                        }
                        Payload::Record(fields) => {
                            if rng.percent(30) {
                                VPat::Record(Vec::new(), true)
                            } else {
                                VPat::Record(
                                    (0..fields.len()).map(|f| (f, loose(self, rng))).collect(),
                                    false,
                                )
                            }
                        }
                    };
                    out.push(Pat::Variant(*e, vi, pv));
                }
                out
            }
            Ty::Tuple(ts) => {
                // One position split into its cases, the others loose.
                let at = rng.below(ts.len());
                let split = self.cover(rng, &ts[at], depth - 1);
                split
                    .into_iter()
                    .map(|q| {
                        Pat::Tuple(
                            (0..ts.len())
                                .map(|i| if i == at { q.clone() } else { loose(self, rng) })
                                .collect(),
                        )
                    })
                    .collect()
            }
            Ty::Struct(i) => {
                let fields = &w.structs[*i].fields;
                let at = rng.below(fields.len());
                let split = self.cover(rng, &fields[at].1, depth - 1);
                split.into_iter().map(|q| Pat::Struct(*i, vec![(at, q)], true)).collect()
            }
            Ty::Int | Ty::Str => vec![loose(self, rng)],
        }
    }
}

// ---------------------------------------------------------------------------
// Usefulness: which arms are reachable, and whether the arms are exhaustive
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Debug)]
enum Ct {
    Int(i64),
    Str(String),
    Bool(bool),
    /// The one constructor of a tuple or a struct.
    Single,
    Variant(usize),
    Some,
    None,
    Ok,
    Err,
    /// A list of exactly this many elements.
    Len(usize),
    /// A list of at least this many, before it is split by length.
    AtLeast(usize),
}

#[derive(Clone, Debug)]
enum Np {
    Wild,
    C(Ct, Vec<Np>),
    Or(Vec<Np>),
}

fn norm(w: &World, p: &Pat, ty: &Ty) -> Np {
    match (p, ty) {
        (Pat::Wild | Pat::Bind(_), _) => Np::Wild,
        (Pat::At(_, q), t) => norm(w, q, t),
        (Pat::Or(ps), t) => Np::Or(ps.iter().map(|q| norm(w, q, t)).collect()),
        (Pat::Int(n), _) => Np::C(Ct::Int(*n), Vec::new()),
        (Pat::Str(s), _) => Np::C(Ct::Str(s.clone()), Vec::new()),
        (Pat::Bool(b), _) => Np::C(Ct::Bool(*b), Vec::new()),
        (Pat::Tuple(ps), Ty::Tuple(ts)) => {
            Np::C(Ct::Single, ps.iter().zip(ts).map(|(q, t)| norm(w, q, t)).collect())
        }
        (Pat::Struct(i, fields, _), _) => {
            let decl = &w.structs[*i].fields;
            let mut args = vec![Np::Wild; decl.len()];
            for (f, q) in fields {
                args[*f] = norm(w, q, &decl[*f].1);
            }
            Np::C(Ct::Single, args)
        }
        (Pat::Variant(e, v, payload), _) => {
            let tys = w.enums[*e].variants[*v].1.tys();
            let mut args = vec![Np::Wild; tys.len()];
            match payload {
                VPat::Unit => {}
                VPat::Tuple(ps) => {
                    for (k, q) in ps.iter().enumerate() {
                        args[k] = norm(w, q, &tys[k]);
                    }
                }
                VPat::Record(fields, _) => {
                    for (f, q) in fields {
                        args[*f] = norm(w, q, &tys[*f]);
                    }
                }
            }
            Np::C(Ct::Variant(*v), args)
        }
        (Pat::Some(q), Ty::Option(t)) => Np::C(Ct::Some, vec![norm(w, q, t)]),
        (Pat::None, _) => Np::C(Ct::None, Vec::new()),
        (Pat::Ok(q), Ty::Result(t, _)) => Np::C(Ct::Ok, vec![norm(w, q, t)]),
        (Pat::Err(q), Ty::Result(_, e)) => Np::C(Ct::Err, vec![norm(w, q, e)]),
        (Pat::List(ps, rest), Ty::List(t)) => {
            let args = ps.iter().map(|q| norm(w, q, t)).collect();
            match rest {
                Rest::Exact => Np::C(Ct::Len(ps.len()), args),
                _ => Np::C(Ct::AtLeast(ps.len()), args),
            }
        }
        _ => unreachable!("a pattern that does not have its type: {p:?} : {ty:?}"),
    }
}

fn sub_tys(w: &World, ty: &Ty, c: &Ct) -> Vec<Ty> {
    match (ty, c) {
        (Ty::Tuple(ts), Ct::Single) => ts.clone(),
        (Ty::Struct(i), Ct::Single) => w.structs[*i].fields.iter().map(|(_, t)| t.clone()).collect(),
        (Ty::Enum(e), Ct::Variant(v)) => w.enums[*e].variants[*v].1.tys(),
        (Ty::Option(t), Ct::Some) | (Ty::Result(t, _), Ct::Ok) => vec![(**t).clone()],
        (Ty::Result(_, e), Ct::Err) => vec![(**e).clone()],
        (Ty::List(t), Ct::Len(m)) => vec![(**t).clone(); *m],
        _ => Vec::new(),
    }
}

/// The row with its head replaced by what is under it, for a value whose head
/// is `c`; `None` where the row's head cannot match such a value.
fn specialize(row: &[Np], c: &Ct, arity: usize) -> Option<Vec<Np>> {
    let mut out = match &row[0] {
        Np::Wild => vec![Np::Wild; arity],
        Np::C(hc, args) => match (hc, c) {
            (Ct::AtLeast(k), Ct::Len(m)) if k <= m => {
                let mut a = args.clone();
                a.resize(*m, Np::Wild);
                a
            }
            (hc, c) if hc == c => args.clone(),
            _ => return None,
        },
        Np::Or(_) => unreachable!("or-patterns are expanded before a row is specialized"),
    };
    out.extend_from_slice(&row[1..]);
    Some(out)
}

fn expand(row: &[Np], out: &mut Vec<Vec<Np>>) {
    match &row[0] {
        Np::Or(alts) => {
            for a in alts {
                let mut r = vec![a.clone()];
                r.extend_from_slice(&row[1..]);
                expand(&r, out);
            }
        }
        _ => out.push(row.to_vec()),
    }
}

/// The longest list length a column names, so lengths above it all behave
/// alike and one more than it stands for every one of them.
fn longest(rows: &[Vec<Np>], head: &Np) -> usize {
    let mut most = 0;
    for p in rows.iter().map(|r| &r[0]).chain(std::iter::once(head)) {
        if let Np::C(Ct::Len(n) | Ct::AtLeast(n), _) = p {
            most = most.max(*n);
        }
    }
    most
}

fn useful(w: &World, rows: &[Vec<Np>], v: &[Np], tys: &[Ty]) -> bool {
    if v.is_empty() {
        return rows.is_empty();
    }
    let mut flat = Vec::new();
    for r in rows {
        expand(r, &mut flat);
    }
    let rows = flat;
    let rest_tys = &tys[1..];
    let try_ct = |c: &Ct, head_args: Vec<Np>| {
        let sub = sub_tys(w, &tys[0], c);
        let srows: Vec<Vec<Np>> = rows.iter().filter_map(|r| specialize(r, c, sub.len())).collect();
        let mut sv = head_args;
        sv.extend_from_slice(&v[1..]);
        let mut stys = sub;
        stys.extend_from_slice(rest_tys);
        useful(w, &srows, &sv, &stys)
    };
    match &v[0] {
        Np::Or(alts) => alts.iter().any(|a| {
            let mut v2 = vec![a.clone()];
            v2.extend_from_slice(&v[1..]);
            useful(w, &rows, &v2, tys)
        }),
        Np::C(Ct::AtLeast(k), args) => {
            let most = longest(&rows, &v[0]);
            (*k..=most + 1).any(|m| {
                let mut a = args.clone();
                a.resize(m, Np::Wild);
                try_ct(&Ct::Len(m), a)
            })
        }
        Np::C(c, args) => try_ct(c, args.clone()),
        Np::Wild => {
            let all: Option<Vec<Ct>> = match &tys[0] {
                Ty::Int | Ty::Str => None,
                Ty::Bool => Some(vec![Ct::Bool(true), Ct::Bool(false)]),
                Ty::Tuple(_) | Ty::Struct(_) => Some(vec![Ct::Single]),
                Ty::Enum(e) => Some((0..w.enums[*e].variants.len()).map(Ct::Variant).collect()),
                Ty::Option(_) => Some(vec![Ct::Some, Ct::None]),
                Ty::Result(..) => Some(vec![Ct::Ok, Ct::Err]),
                Ty::List(_) => Some((0..=longest(&rows, &v[0]) + 1).map(Ct::Len).collect()),
            };
            // Split by constructor only where the column names every one of
            // them; otherwise the rows with a wildcard there decide, which is
            // what keeps this from growing with the product of every type's
            // constructors.
            let heads: Vec<&Ct> = rows
                .iter()
                .filter_map(|r| match &r[0] {
                    Np::C(c, _) => Some(c),
                    _ => None,
                })
                .collect();
            let named = |c: &Ct| {
                heads.iter().any(|h| match (h, c) {
                    (Ct::AtLeast(k), Ct::Len(m)) => k <= m,
                    (h, c) => *h == c,
                })
            };
            let all = all.filter(|all| all.iter().all(|c| named(c)));
            match all {
                Some(all) => all.iter().any(|c| {
                    let n = sub_tys(w, &tys[0], c).len();
                    try_ct(c, vec![Np::Wild; n])
                }),
                None => {
                    let rows: Vec<Vec<Np>> = rows
                        .iter()
                        .filter(|r| matches!(r[0], Np::Wild))
                        .map(|r| r[1..].to_vec())
                        .collect();
                    useful(w, &rows, &v[1..], rest_tys)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum IntE {
    Var(String),
    Lit(i64),
    Limit,
    /// `name.length()`, of a string or a list.
    Len(String),
}

#[derive(Clone, Copy, Debug)]
enum Op {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

const OPS: &[Op] = &[Op::Lt, Op::Le, Op::Gt, Op::Ge, Op::Eq, Op::Ne];

impl Op {
    fn text(self) -> &'static str {
        match self {
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::Eq => "==",
            Op::Ne => "!=",
        }
    }

    fn holds(self, a: i64, b: i64) -> bool {
        match self {
            Op::Lt => a < b,
            Op::Le => a <= b,
            Op::Gt => a > b,
            Op::Ge => a >= b,
            Op::Eq => a == b,
            Op::Ne => a != b,
        }
    }
}

#[derive(Clone, Debug)]
enum Guard {
    Cmp(IntE, Op, IntE),
    StrEq(String, String, bool),
    StartsWith(String, String),
    /// A `Bool` binding, negated when the flag is set.
    Flag(String, bool),
    /// `name.method()`, negated when the flag is set: `isEmpty` of a list,
    /// `isSome` of an option, `isOk` of a result.
    Probe(String, &'static str, bool),
    And(Box<Guard>, Box<Guard>),
    Or(Box<Guard>, Box<Guard>),
}

fn lookup<'a>(env: &'a [(String, Val)], name: &str) -> &'a Val {
    &env.iter().rev().find(|(n, _)| n == name).unwrap_or_else(|| panic!("`{name}` is not bound")).1
}

fn int_of(e: &IntE, env: &[(String, Val)], limit: i64) -> i64 {
    match e {
        IntE::Lit(n) => *n,
        IntE::Limit => limit,
        IntE::Var(n) => match lookup(env, n) {
            Val::Int(k) => *k,
            other => panic!("`{n}` is {other:?}, not an Int"),
        },
        IntE::Len(n) => match lookup(env, n) {
            Val::Str(s) => s.len() as i64,
            Val::List(xs) => xs.len() as i64,
            other => panic!("`{n}` has no length: {other:?}"),
        },
    }
}

impl Guard {
    fn holds(&self, env: &[(String, Val)], limit: i64) -> bool {
        match self {
            Guard::Cmp(a, op, b) => op.holds(int_of(a, env, limit), int_of(b, env, limit)),
            Guard::StrEq(n, s, negated) => {
                (matches!(lookup(env, n), Val::Str(x) if x == s)) != *negated
            }
            Guard::StartsWith(n, s) => matches!(lookup(env, n), Val::Str(x) if x.starts_with(s.as_str())),
            Guard::Flag(n, negated) => (matches!(lookup(env, n), Val::Bool(true))) != *negated,
            Guard::Probe(n, method, negated) => {
                let v = lookup(env, n);
                let yes = match *method {
                    "isEmpty" => matches!(v, Val::List(xs) if xs.is_empty()),
                    "isSome" => matches!(v, Val::Some(_)),
                    "isOk" => matches!(v, Val::Ok(_)),
                    other => panic!("no model for `{other}`"),
                };
                yes != *negated
            }
            Guard::And(a, b) => a.holds(env, limit) && b.holds(env, limit),
            Guard::Or(a, b) => a.holds(env, limit) || b.holds(env, limit),
        }
    }

    fn text(&self) -> String {
        let int = |e: &IntE| match e {
            IntE::Var(n) => n.clone(),
            IntE::Lit(k) => k.to_string(),
            IntE::Limit => String::from("limit"),
            IntE::Len(n) => format!("{n}.length()"),
        };
        match self {
            Guard::Cmp(a, op, b) => format!("{} {} {}", int(a), op.text(), int(b)),
            Guard::StrEq(n, s, negated) => {
                format!("{n} {} \"{s}\"", if *negated { "!=" } else { "==" })
            }
            Guard::StartsWith(n, s) => format!("{n}.startsWith(\"{s}\")"),
            Guard::Flag(n, negated) => format!("{}{n}", if *negated { "!" } else { "" }),
            Guard::Probe(n, method, negated) => {
                format!("{}{n}.{method}()", if *negated { "!" } else { "" })
            }
            Guard::And(a, b) => format!("{} && {}", a.operand(), b.operand()),
            Guard::Or(a, b) => format!("{} || {}", a.operand(), b.operand()),
        }
    }

    /// The text as one side of `&&` or `||`, parenthesised where it is itself
    /// one.
    fn operand(&self) -> String {
        match self {
            Guard::And(..) | Guard::Or(..) => format!("({})", self.text()),
            _ => self.text(),
        }
    }
}

/// One guard over the names in `names`, or `None` where none of them is of a
/// type a guard reads. It reads the values in `env` to draw its literals near
/// them, so it passes for some calls and fails for others.
fn atom(
    rng: &mut Rng,
    names: &[(String, Ty)],
    env: &[(String, Val)],
    limit: i64,
) -> Option<Guard> {
    let usable: Vec<&(String, Ty)> = names
        .iter()
        .filter(|(_, t)| {
            matches!(t, Ty::Int | Ty::Str | Ty::Bool | Ty::List(_) | Ty::Option(_) | Ty::Result(..))
        })
        .collect();
    if usable.is_empty() {
        return None;
    }
    let (name, ty) = (*rng.pick(&usable)).clone();
    let near = |rng: &mut Rng, k: i64| k + rng.below(3) as i64 - 1;
    let g = match ty {
        Ty::Int => {
            let k = int_of(&IntE::Var(name.clone()), env, limit);
            let ints: Vec<&(String, Ty)> =
                usable.iter().copied().filter(|(n, t)| *t == Ty::Int && *n != name).collect();
            let rhs = match rng.below(10) {
                0..=1 => IntE::Limit,
                2..=3 if !ints.is_empty() => IntE::Var(rng.pick(&ints).0.clone()),
                _ => IntE::Lit(near(rng, k)),
            };
            Guard::Cmp(IntE::Var(name), *rng.pick(OPS), rhs)
        }
        Ty::Str => {
            let Val::Str(s) = lookup(env, &name).clone() else { unreachable!() };
            match rng.below(3) {
                0 => {
                    let lit = if rng.percent(50) { s } else { rng.pick(STRS).to_string() };
                    Guard::StrEq(name, lit, rng.percent(25))
                }
                1 => {
                    let k = s.len() as i64;
                    Guard::Cmp(IntE::Len(name), *rng.pick(OPS), IntE::Lit(near(rng, k)))
                }
                _ => Guard::StartsWith(name, rng.pick(&["a", "b", "c", "ab"]).to_string()),
            }
        }
        Ty::Bool => Guard::Flag(name, rng.percent(40)),
        Ty::List(_) => {
            if rng.percent(50) {
                Guard::Probe(name, "isEmpty", rng.percent(50))
            } else {
                let Val::List(xs) = lookup(env, &name) else { unreachable!() };
                let k = xs.len() as i64;
                Guard::Cmp(IntE::Len(name), *rng.pick(OPS), IntE::Lit(near(rng, k)))
            }
        }
        Ty::Option(_) => Guard::Probe(name, "isSome", rng.percent(40)),
        Ty::Result(..) => Guard::Probe(name, "isOk", rng.percent(40)),
        _ => return None,
    };
    Some(g)
}

fn guard(
    rng: &mut Rng,
    names: &[(String, Ty)],
    env: &[(String, Val)],
    limit: i64,
) -> Option<Guard> {
    let a = atom(rng, names, env, limit)?;
    if rng.percent(30) {
        if let Some(b) = atom(rng, names, env, limit) {
            return Some(if rng.percent(50) {
                Guard::And(Box::new(a), Box::new(b))
            } else {
                Guard::Or(Box::new(a), Box::new(b))
            });
        }
    }
    Some(a)
}

// ---------------------------------------------------------------------------
// Matches
// ---------------------------------------------------------------------------

/// What an arm prints about a name it bound.
#[derive(Clone, Debug)]
enum Show {
    /// An `Int`, `Str` or `Bool`, interpolated as it is.
    Value(String),
    /// A list's or a string's length.
    Len(String),
}

#[derive(Clone, Debug)]
struct Arm {
    pat: Pat,
    guard: Option<Guard>,
    label: String,
    shows: Vec<Show>,
    /// A `match` on one of the arm's compound bindings, inside a block, whose
    /// answer the arm prints after its own label.
    nested: Option<(String, Box<Match>)>,
}

#[derive(Clone, Debug)]
struct Match {
    arms: Vec<Arm>,
}

fn show_text(s: &Show) -> String {
    match s {
        Show::Value(n) => format!("${{{n}}}"),
        Show::Len(n) => format!("${{{n}.length()}}"),
    }
}

fn shown(s: &Show, env: &[(String, Val)]) -> String {
    match s {
        Show::Value(n) => match lookup(env, n) {
            Val::Int(k) => k.to_string(),
            Val::Str(x) => x.clone(),
            Val::Bool(b) => b.to_string(),
            other => panic!("`{n}` is shown but is {other:?}"),
        },
        Show::Len(n) => int_of(&IntE::Len(n.clone()), env, 0).to_string(),
    }
}

impl Match {
    /// Which arm a value takes and what it answers, as the language says it
    /// should: the first arm whose pattern matches and whose guard holds.
    fn run(&self, v: &Val, limit: i64) -> String {
        for arm in &self.arms {
            let mut env = Vec::new();
            if !bind(&arm.pat, v, &mut env) {
                continue;
            }
            if let Some(g) = &arm.guard {
                if !g.holds(&env, limit) {
                    continue;
                }
            }
            let mut out = arm.label.clone();
            if let Some((name, inner)) = &arm.nested {
                out.push(' ');
                out.push_str(&inner.run(lookup(&env, name), limit));
            }
            for s in &arm.shows {
                out.push(' ');
                out.push_str(&shown(s, &env));
            }
            return out;
        }
        panic!("no arm of an exhaustive match took {v:?}")
    }

    /// The match as source, each arm's answer handed to `wrap` — the
    /// identity for a match whose value is used, `say(ctx, ..)` for one used
    /// as a statement.
    fn text(&self, w: &World, scrutinee: &str, prefix: &str, wrap: &dyn Fn(String) -> String) -> String {
        let mut out = format!("match ({scrutinee}) {{\n");
        for arm in &self.arms {
            let mut line = w.pat_text(&arm.pat);
            if let Some(g) = &arm.guard {
                let _ = write!(line, " if {}", g.text());
            }
            let mut template = format!("{prefix}{}", arm.label);
            let mut lets = String::new();
            if let Some((name, inner)) = &arm.nested {
                let local = format!("{name}n");
                let _ = write!(lets, "let {local} = {};\n", inner.text(w, name, "", &|s| s));
                let _ = write!(template, " ${{{local}}}");
            }
            for s in &arm.shows {
                template.push(' ');
                template.push_str(&show_text(s));
            }
            let answer = if template.contains("${") {
                format!("str.format(ctx, \"{template}\")")
            } else {
                format!("\"{template}\"")
            };
            let body = wrap(answer);
            if lets.is_empty() {
                let _ = writeln!(out, "{line} => {body},");
            } else {
                let _ = writeln!(out, "{line} => {{\n{lets}{body}\n}},");
            }
        }
        out.push('}');
        out
    }
}

/// Draws one match over `ty`, from the values it will be asked about.
fn draw_match(
    rng: &mut Rng,
    names: &mut Patterns,
    ty: &Ty,
    values: &[Val],
    limit: i64,
    nest: bool,
) -> Match {
    let w = names.w;
    let mut arms: Vec<Arm> = Vec::new();
    let wanted = 2 + rng.below(5);
    for _ in 0..wanted {
        let source = if rng.percent(50) { &values[0] } else { rng.pick(values) };
        let pat = if rng.percent(8) {
            // An arm whose whole pattern is an or-pattern.
            let other = w.value(rng, ty);
            let a = names.structural(rng, ty, source, 3, false);
            let b = names.structural(rng, ty, &other, 3, false);
            Pat::Or(vec![a, b])
        } else {
            names.structural(rng, ty, source, 3, true)
        };
        arms.push(arm(rng, names, pat, ty, source, limit, nest));
    }
    // A closing arm or arms, so the match is exhaustive whatever was drawn.
    let closing: Vec<Pat> = match rng.below(10) {
        0..=3 => vec![Pat::Wild],
        4..=5 => vec![Pat::Bind(names.fresh())],
        _ => names.cover(rng, ty, 2),
    };
    for pat in closing {
        let source = values[0].clone();
        arms.push(arm(rng, names, pat, ty, &source, limit, nest));
    }
    let mut m = Match { arms: Vec::new() };
    let mut rows: Vec<Vec<Np>> = Vec::new();
    for mut a in arms {
        // An or-pattern keeps only the alternatives something can reach.
        if let Pat::Or(alts) = &a.pat {
            let mut kept: Vec<Pat> = Vec::new();
            let mut seen = rows.clone();
            for alt in alts {
                let n = norm(w, alt, ty);
                if useful(w, &seen, &[n.clone()], std::slice::from_ref(ty)) {
                    seen.push(vec![n]);
                    kept.push(alt.clone());
                }
            }
            a.pat = match kept.len() {
                0 => continue,
                1 => kept.pop().unwrap(),
                _ => Pat::Or(kept),
            };
        }
        let n = norm(w, &a.pat, ty);
        if !useful(w, &rows, &[n.clone()], std::slice::from_ref(ty)) {
            continue;
        }
        if a.guard.is_none() {
            rows.push(vec![n]);
        }
        m.arms.push(a);
    }
    if useful(w, &rows, &[Np::Wild], std::slice::from_ref(ty)) {
        let source = values[0].clone();
        let mut last = arm(rng, names, Pat::Wild, ty, &source, limit, false);
        last.guard = None;
        m.arms.push(last);
    }
    for (i, a) in m.arms.iter_mut().enumerate() {
        a.label = if nest { format!("a{i}") } else { format!("i{i}") };
    }
    m
}

fn arm(
    rng: &mut Rng,
    names: &mut Patterns,
    pat: Pat,
    ty: &Ty,
    source: &Val,
    limit: i64,
    nest: bool,
) -> Arm {
    let w = names.w;
    let mut bindings = Vec::new();
    bound(w, &pat, ty, &mut bindings);
    let mut env = Vec::new();
    let matched = bind(&pat, source, &mut env);
    let guard = if matched && rng.percent(40) { guard(rng, &bindings, &env, limit) } else { None };
    let mut shows = Vec::new();
    for (name, t) in &bindings {
        match t {
            Ty::Int | Ty::Str | Ty::Bool if rng.percent(70) => shows.push(Show::Value(name.clone())),
            Ty::List(_) if rng.percent(70) => shows.push(Show::Len(name.clone())),
            _ => {}
        }
    }
    let mut nested = None;
    if nest && matched && rng.percent(30) {
        let compound: Vec<&(String, Ty)> = bindings
            .iter()
            .filter(|(_, t)| !matches!(t, Ty::Int | Ty::Str | Ty::Bool))
            .collect();
        if !compound.is_empty() {
            let (name, t) = (*rng.pick(&compound)).clone();
            let v = lookup(&env, &name).clone();
            let mut values = vec![v.clone()];
            values.push(w.mutate(rng, &t, &v));
            let inner = draw_match(rng, names, &t, &values, limit, false);
            nested = Some((name, Box::new(inner)));
        }
    }
    Arm { pat, guard, label: String::new(), shows, nested }
}

// ---------------------------------------------------------------------------
// Functions and programs
// ---------------------------------------------------------------------------

/// How the scrutinee is built from the function's parameter `v: T`.
#[derive(Clone, Copy)]
enum Form {
    Direct,
    /// `id(v)`, a generic helper.
    Identity,
    /// `some(v)`, a generic helper answering `Option<T>`.
    Some,
    /// `okay(v, limit)`, a generic helper answering `Result<T, Int>`.
    Okay,
    /// `make(v, limit)` with `make` a lambda answering `Option.Some((x, n))`,
    /// whose type is only inferred.
    LambdaSomePair,
    /// `make(v, limit)` with `make` a lambda answering `(n, x)`.
    LambdaPair,
    /// `make(v)` with `make` a lambda answering `Option.Some(x)`.
    LambdaSome,
    /// `held.1`, a field of a tuple local, so the payloads the arms bind are
    /// words of a block the match does not own.
    Projected,
}

const FORMS: &[Form] = &[
    Form::Direct,
    Form::Direct,
    Form::Direct,
    Form::Identity,
    Form::Some,
    Form::Okay,
    Form::LambdaSomePair,
    Form::LambdaPair,
    Form::LambdaSome,
    Form::Projected,
];

impl Form {
    fn ty(self, t: &Ty) -> Ty {
        let t = t.clone();
        match self {
            Form::Direct | Form::Identity | Form::Projected => t,
            Form::Some | Form::LambdaSome => Ty::Option(Box::new(t)),
            Form::Okay => Ty::Result(Box::new(t), Box::new(Ty::Int)),
            Form::LambdaSomePair => Ty::Option(Box::new(Ty::Tuple(vec![t, Ty::Int]))),
            Form::LambdaPair => Ty::Tuple(vec![Ty::Int, t]),
        }
    }

    fn value(self, v: &Val, limit: i64) -> Val {
        let v = v.clone();
        match self {
            Form::Direct | Form::Identity | Form::Projected => v,
            Form::Some | Form::LambdaSome => Val::Some(Box::new(v)),
            Form::Okay => {
                if limit > 2 {
                    Val::Err(Box::new(Val::Int(limit)))
                } else {
                    Val::Ok(Box::new(v))
                }
            }
            Form::LambdaSomePair => Val::Some(Box::new(Val::Tuple(vec![v, Val::Int(limit)]))),
            Form::LambdaPair => Val::Tuple(vec![Val::Int(limit), v]),
        }
    }

    /// The `let` that comes before the match, if any, and the scrutinee.
    fn text(self, w: &World, t: &Ty) -> (String, String) {
        let t = w.text(t);
        match self {
            Form::Direct => (String::new(), String::from("v")),
            Form::Identity => (String::new(), String::from("id(v)")),
            Form::Some => (String::new(), String::from("some(v)")),
            Form::Okay => (String::new(), String::from("okay(v, limit)")),
            Form::LambdaSomePair => (
                format!("let make = fn(x: {t}, n: Int) => Option.Some((x, n));\n"),
                String::from("make(v, limit)"),
            ),
            Form::LambdaPair => (
                format!("let make = fn(x: {t}, n: Int) => (n, x);\n"),
                String::from("make(v, limit)"),
            ),
            Form::LambdaSome => (
                format!("let make = fn(x: {t}) => Option.Some(x);\n"),
                String::from("make(v)"),
            ),
            Form::Projected => (String::from("let held = (limit, v);\n"), String::from("held.1")),
        }
    }
}

/// How the function uses its match.
#[derive(Clone, Copy)]
enum Use {
    /// The match is the function's result.
    Result,
    /// The match is bound by `let`, and the function answers a string built
    /// around it.
    Bound,
    /// The match is a statement whose arms print.
    Statement,
    /// The match is bound by `let` and `v` is read again after it, so the
    /// match borrows its scrutinee rather than consuming it.
    Kept,
}

const PRELUDE: &str = r#"from "platform/effect" import { Allocator, Stdout };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "native" import { NativeHost };

fn say<C: Stdout>(ctx: C, what: Str): () {
    let _ = io.println(ctx, what).ignore();
    ()
}

fn id<T>(value: T): T { value }

fn some<T>(value: T): Option<T> { .Some(value) }

fn okay<T>(value: T, limit: Int): Result<T, Int> {
    if (limit > 2) { .Err(limit) } else { .Ok(value) }
}
"#;

/// The program [`SEED`] and `index` name, and what it prints.
pub fn batch(seed: u64, index: usize) -> Batch {
    let mut rng = Rng(seed ^ (index as u64).wrapping_mul(0xA24B_AED4_963E_E407));
    let w = World::declare(&mut rng);
    let mut functions = String::new();
    let mut calls = String::new();
    let mut expected = String::new();
    for k in 0..CASES {
        let t = if rng.percent(70) {
            match rng.below(3) {
                0 => Ty::Enum(rng.below(w.enums.len())),
                1 => Ty::Struct(rng.below(w.structs.len())),
                _ => w.draw(&mut rng, 3),
            }
        } else {
            w.draw(&mut rng, 3)
        };
        let form = *rng.pick(FORMS);
        let use_ = match rng.below(10) {
            0..=3 => Use::Result,
            4..=5 => Use::Bound,
            6 => Use::Kept,
            _ => Use::Statement,
        };
        // The values it is called with: one, and two or one more that mostly
        // share its outer shape.
        let first = w.value(&mut rng, &t);
        let mut args = vec![first.clone()];
        for _ in 0..1 + rng.below(2) {
            let next = w.mutate(&mut rng, &t, &first);
            args.push(next);
        }
        let limits: Vec<i64> = args.iter().map(|_| rng.below(5) as i64).collect();
        let s_ty = form.ty(&t);
        let s_vals: Vec<Val> =
            args.iter().zip(&limits).map(|(v, l)| form.value(v, *l)).collect();
        let mut counter = 0;
        let mut names = Patterns { w: &w, next: &mut counter };
        let m = draw_match(&mut rng, &mut names, &s_ty, &s_vals, limits[0], true);
        let (setup, scrutinee) = form.text(&w, &t);
        let t_text = w.text(&t);
        match use_ {
            Use::Result => {
                let body = m.text(&w, &scrutinee, "", &|s| s);
                let _ = writeln!(
                    functions,
                    "fn m{k}<C: Allocator>(ctx: C, v: {t_text}, limit: Int): Str {{\n{setup}{body}\n}}\n"
                );
            }
            Use::Bound => {
                let body = m.text(&w, &scrutinee, "", &|s| s);
                let _ = writeln!(
                    functions,
                    "fn m{k}<C: Allocator>(ctx: C, v: {t_text}, limit: Int): Str {{\n{setup}\
                     let answer = {body};\nstr.format(ctx, \"<${{answer}}>\")\n}}\n"
                );
            }
            Use::Kept => {
                let body = m.text(&w, &scrutinee, "", &|s| s);
                let _ = writeln!(
                    functions,
                    "fn m{k}<C: Allocator>(ctx: C, v: {t_text}, limit: Int): Str {{\n{setup}\
                     let answer = {body};\nlet kept = some(v).isSome();\n\
                     str.format(ctx, \"<${{answer}}|${{kept}}>\")\n}}\n"
                );
            }
            Use::Statement => {
                let body = m.text(&w, &scrutinee, &format!("s{k} "), &|s| format!("say(ctx, {s})"));
                let _ = writeln!(
                    functions,
                    "fn s{k}<C: Allocator + Stdout>(ctx: C, v: {t_text}, limit: Int): () {{\n{setup}\
                     let _ = {body};\n()\n}}\n"
                );
            }
        }
        for ((arg, limit), s_val) in args.iter().zip(&limits).zip(&s_vals) {
            let arg_text = w.expr(&mut rng, &t, arg);
            let answer = m.run(s_val, *limit);
            match use_ {
                Use::Result => {
                    let _ = writeln!(
                        calls,
                        "let _ = say(ctx, str.format(ctx, \"m{k} ${{m{k}(ctx, {arg_text}, {limit})}}\"));"
                    );
                    let _ = writeln!(expected, "m{k} {answer}");
                }
                Use::Bound => {
                    let _ = writeln!(
                        calls,
                        "let _ = say(ctx, str.format(ctx, \"m{k} ${{m{k}(ctx, {arg_text}, {limit})}}\"));"
                    );
                    let _ = writeln!(expected, "m{k} <{answer}>");
                }
                Use::Kept => {
                    let _ = writeln!(
                        calls,
                        "let _ = say(ctx, str.format(ctx, \"m{k} ${{m{k}(ctx, {arg_text}, {limit})}}\"));"
                    );
                    let _ = writeln!(expected, "m{k} <{answer}|true>");
                }
                Use::Statement => {
                    let _ = writeln!(calls, "let _ = s{k}(ctx, {arg_text}, {limit});");
                    let _ = writeln!(expected, "s{k} {answer}");
                }
            }
        }
    }
    let mut source = format!(
        "// Generated by cli/tests/native/matches/generate.rs from seed {seed:#x}, batch \
         {index}.\n// Regenerate with `BURI_BLESS=1 cargo test -p buri --test native matches::`.\n\n"
    );
    source.push_str(PRELUDE);
    source.push('\n');
    source.push_str(&w.declarations());
    source.push_str(&functions);
    source.push_str(
        "export fn main(host: NativeHost): Result<(), Str> {\n\
         let ctx = context { Allocator: host.alloc, Stdout: host.stdout };\n",
    );
    source.push_str(&calls);
    source.push_str(".Ok(())\n}\n");
    let mut map = buri::diagnostics::SourceMap::new();
    let id = map.add(String::from("main.buri"), std::path::PathBuf::from("main.buri"), source.clone());
    let errors: String = buri::parsing::parser::parse(&source, id)
        .errors
        .iter()
        .map(|e| map.render(e, false))
        .collect();
    assert!(errors.is_empty(), "batch {index} of seed {seed:#x} does not parse:\n{errors}\n{source}");
    let source = buri::formatting::source(&source)
        .unwrap_or_else(|| panic!("batch {index} of seed {seed:#x} does not format:\n{source}"));
    Batch { source, expected }
}
