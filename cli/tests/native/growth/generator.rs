//! The generator behind `cli/tests/growth/`: from one seed, a few hundred
//! programs that each grow a list, a string, or a record holding a list, and
//! what each must print.
//!
//! A case is a point in [`Shape`]'s space. [`source`] writes it as a whole
//! program and [`expected`] runs the same steps over Rust values, so the
//! answer a case pins comes from the generator rather than from a compiler.
//! The draw is greedy over pairs: each case is the candidate, of a handful
//! drawn, that covers the most pairs of dimension values no earlier case did.

use std::collections::BTreeSet;
use std::fmt::Write as _;

/// A splitmix64 stream: small, and the same on every host.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in `0..n`.
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<T: Copy>(&mut self, from: &[T]) -> T {
        from[self.below(from.len())]
    }
}

/// What a case grows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Value {
    Ints,
    /// Literal strings, which allocate nothing.
    Strs,
    /// Strings built per element, `str.fromInt(ctx, code)`, so the list holds
    /// counted blocks of its own.
    Names,
    Text,
}

/// What the loop carries: the value alone, a `(count, value)` tuple, a record
/// with the value in a field, or that record in a field of another, which a
/// helper steps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Acc {
    Bare,
    Tuple,
    Record,
    Nested,
}

/// How a step reaches the value inside its accumulator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Access {
    /// `let (n, items) = acc;`
    Destructure,
    /// `acc.1`, `acc.items`, or a struct update.
    Field,
    /// `match (acc) { (n, items) => .. }`; on a bare value, a match on the
    /// step's index whose arms each grow it.
    Match,
}

/// Who pushes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pusher {
    /// The step itself.
    Inline,
    /// A one-line helper the step hands the value to, small enough to inline.
    Small,
    /// A helper called from two places, too large to inline.
    Large,
}

/// What reads the value before it grows: the reader borrows it, and the push
/// after must still find it unique.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reader {
    None,
    /// `items.length()` in the step.
    Inline,
    /// A one-line helper.
    Small,
    /// A helper called twice, too large to inline.
    Large,
}

/// What runs the steps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Driver {
    /// `list.range(..).foldCtx(..)`, or `foldResultCtx` when a step can fail.
    Fold,
    /// The same with the step's body written in the lambda.
    FoldLambda,
    /// A tail call per step: a `while` loop.
    Loop,
    /// A call per step that is not a tail call: the step runs on the way out.
    Recursion,
}

/// How two drivers nest.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Nest {
    None,
    /// An outer driver whose step is a whole inner run, seeded with the outer
    /// accumulator: the printer's shape.
    Seeded,
    /// `list.range(..).mapCtx(..)` with a whole inner run per element, each
    /// from an empty accumulator.
    Mapped,
}

/// One case.
#[derive(Clone, Debug)]
pub struct Shape {
    pub value: Value,
    pub acc: Acc,
    pub access: Access,
    pub pusher: Pusher,
    pub reader: Reader,
    /// `let held = items;` before the value grows.
    pub alias: bool,
    /// The driver that calls the step.
    pub inner: Driver,
    /// The driver around it, when nested.
    pub outer: Driver,
    pub nest: Nest,
    /// Stop growing once the value is this long: an early exit from the loop,
    /// or a step that hands its accumulator back untouched.
    pub stop: Option<usize>,
    /// The step reads a `Result` with `?`, which fails at this index when it
    /// is in range, and the drivers carry the `Result` out.
    pub fail: Option<usize>,
    /// Outer rounds, when nested, and steps per round.
    pub rounds: usize,
    pub steps: usize,
    /// The element pushed at index `i` is chosen from `i * scale + offset`.
    pub scale: usize,
    pub offset: usize,
    /// Keep the accumulator from half way, grow on from it, and read it again
    /// at the end: the push after the half must copy, and what was kept must
    /// read as it did. Only unnested and infallible.
    pub snapshot: bool,
}

impl Shape {
    /// One string per dimension, for the pairwise coverage the draw maximises
    /// and the summary at the top of each file.
    fn dimensions(&self) -> Vec<String> {
        vec![
            format!("value={:?}", self.value),
            format!("acc={:?}", self.acc),
            format!("access={:?}", self.access),
            format!("pusher={:?}", self.pusher),
            format!("reader={:?}", self.reader),
            format!("alias={}", self.alias),
            format!("inner={:?}", self.inner),
            format!("outer={:?}", if self.nest == Nest::Seeded { Some(self.outer) } else { None }),
            format!("nest={:?}", self.nest),
            format!("snapshot={}", self.snapshot),
            format!("stop={}", self.stop.is_some()),
            format!(
                "fail={}",
                match self.fail {
                    None => "none",
                    Some(f) if f < self.total() => "fires",
                    Some(_) => "holds",
                }
            ),
        ]
    }

    fn total(&self) -> usize {
        self.rounds * self.steps
    }

    /// Whether a step returns a `Result`.
    fn fallible(&self) -> bool {
        self.fail.is_some()
    }
}

fn draw(rng: &mut Rng) -> Shape {
    use Driver::{Fold, FoldLambda, Loop, Recursion};
    let nest = rng.pick(&[Nest::None, Nest::None, Nest::Seeded, Nest::Mapped]);
    let (rounds, steps) = match nest {
        Nest::None => (1, rng.pick(&[400, 600, 800, 1000])),
        _ => (rng.pick(&[4, 8, 16]), rng.pick(&[50, 100, 150])),
    };
    let total = rounds * steps;
    let stop = (rng.below(4) == 0).then(|| total / 3 + rng.below(total / 2));
    let fail = (rng.below(4) == 0).then(|| {
        // Half of them never fire, so the `?` is on the path and the value is
        // still printed; the other half fire part-way.
        if rng.below(2) == 0 {
            total + 1
        } else {
            total / 4 + rng.below(total / 2)
        }
    });
    Shape {
        value: rng.pick(&[Value::Ints, Value::Strs, Value::Names, Value::Text]),
        acc: rng.pick(&[Acc::Bare, Acc::Tuple, Acc::Record, Acc::Nested]),
        access: rng.pick(&[Access::Destructure, Access::Field, Access::Match]),
        pusher: rng.pick(&[Pusher::Inline, Pusher::Small, Pusher::Large]),
        reader: rng.pick(&[Reader::None, Reader::Inline, Reader::Small, Reader::Large]),
        alias: rng.below(2) == 0,
        inner: rng.pick(&[Fold, FoldLambda, Loop, Recursion]),
        outer: rng.pick(&[Fold, FoldLambda, Loop, Recursion]),
        nest,
        stop,
        fail,
        rounds,
        steps,
        scale: rng.pick(&[1, 3, 7]),
        offset: rng.below(10),
        snapshot: nest == Nest::None && fail.is_none() && rng.below(3) == 0,
    }
}

/// `count` shapes from `seed`, each chosen for the pairs it adds.
pub fn shapes(seed: u64, count: usize) -> Vec<Shape> {
    let mut rng = Rng::new(seed);
    let mut covered: BTreeSet<(String, String)> = BTreeSet::new();
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let mut best: Option<(usize, Shape)> = None;
        for _ in 0..16 {
            let shape = draw(&mut rng);
            let new = pairs(&shape).filter(|p| !covered.contains(p)).count();
            if best.as_ref().is_none_or(|(n, _)| new > *n) {
                best = Some((new, shape));
            }
        }
        let (_, shape) = best.unwrap();
        covered.extend(pairs(&shape));
        out.push(shape);
    }
    out
}

fn pairs(shape: &Shape) -> impl Iterator<Item = (String, String)> {
    let dims = shape.dimensions();
    let mut out = Vec::new();
    for (a, x) in dims.iter().enumerate() {
        for y in &dims[a + 1..] {
            out.push((x.clone(), y.clone()));
        }
    }
    out.into_iter()
}

/// The pairs of dimension values a long draw reaches that `shapes` miss, and
/// how many the draw reached. Some pairs cannot happen, such as an outer
/// driver without nesting, so the draw is what says which can.
pub fn uncovered(shapes: &[Shape]) -> (Vec<(String, String)>, usize) {
    let mut rng = Rng::new(1);
    let reachable: BTreeSet<_> = (0..20_000).flat_map(|_| pairs(&draw(&mut rng))).collect();
    let covered: BTreeSet<_> = shapes.iter().flat_map(pairs).collect();
    let total = reachable.len();
    (reachable.into_iter().filter(|p| !covered.contains(p)).collect(), total)
}

// ---------------------------------------------------------------------------
// The model
// ---------------------------------------------------------------------------

const POOL: [&str; 3] = ["a", "bb", "ccc"];

#[derive(Clone)]
enum Grown {
    Ints(Vec<i64>),
    Strs(Vec<String>),
    Text(String),
}

impl Grown {
    fn empty(value: Value) -> Grown {
        match value {
            Value::Ints => Grown::Ints(Vec::new()),
            Value::Strs | Value::Names => Grown::Strs(Vec::new()),
            Value::Text => Grown::Text(String::new()),
        }
    }

    fn len(&self) -> usize {
        match self {
            Grown::Ints(v) => v.len(),
            Grown::Strs(v) => v.len(),
            Grown::Text(s) => s.len(),
        }
    }

    fn push(&mut self, value: Value, code: i64) {
        match self {
            Grown::Ints(v) => v.push(code),
            Grown::Strs(v) if value == Value::Names => v.push(code.to_string()),
            Grown::Strs(v) => v.push(String::from(POOL[(code % 3) as usize])),
            Grown::Text(s) => s.push_str(POOL[(code % 3) as usize]),
        }
    }

    fn hash(&self) -> i64 {
        match self {
            Grown::Ints(v) => v.iter().fold(7, |h, x| (h * 31 + x) % 1_000_003),
            Grown::Strs(v) => v.iter().fold(7, |h, x| (h * 31 + x.len() as i64) % 1_000_003),
            Grown::Text(s) => s.chars().fold(7, |h, c| (h * 31 + letter(c)) % 1_000_003),
        }
    }
}

fn letter(c: char) -> i64 {
    match c {
        'a' => 1,
        'b' => 2,
        _ => 3,
    }
}

struct State {
    n: i64,
    grown: Grown,
}

/// Every step from `0` to the case's total, in order, against one state.
/// `Err(())` where the `?` fired.
fn run(shape: &Shape, state: &mut State, from: usize, to: usize) -> Result<(), ()> {
    for i in from..to {
        if shape.stop.is_some_and(|l| state.grown.len() >= l) {
            continue;
        }
        if shape.fail == Some(i) {
            return Err(());
        }
        let i64i = i as i64;
        let mut code = i64i * shape.scale as i64 + shape.offset as i64;
        code += match shape.reader {
            Reader::None => 0,
            Reader::Inline | Reader::Small => state.grown.len() as i64 % 7,
            Reader::Large => (2 * state.grown.len() as i64) % 7,
        };
        if shape.fallible() {
            code += i64i % 5;
        }
        if shape.pusher == Pusher::Large {
            code += i64i % 2;
        }
        if shape.acc == Acc::Bare && shape.access == Access::Match && i % 2 == 1 {
            code += 1;
        }
        state.n += 1;
        state.grown.push(shape.value, code);
    }
    Ok(())
}

/// The line a case prints.
pub fn expected(shape: &Shape) -> String {
    let fresh = || State { n: 0, grown: Grown::empty(shape.value) };
    match shape.nest {
        Nest::None if shape.snapshot => {
            let mut state = fresh();
            let half = shape.steps / 2;
            let _ = run(shape, &mut state, 0, half);
            let kept = format!("{} {}", state.grown.len(), state.grown.hash());
            let _ = run(shape, &mut state, half, shape.total());
            format!("{} | {kept}", shown(shape, &state))
        }
        Nest::None | Nest::Seeded => {
            let mut state = fresh();
            match run(shape, &mut state, 0, shape.total()) {
                Ok(()) => shown(shape, &state),
                Err(()) => String::from("stopped bad"),
            }
        }
        Nest::Mapped => {
            let mut lengths = 0;
            let mut hashes = 0;
            for r in 0..shape.rounds {
                let mut state = fresh();
                match run(shape, &mut state, r * shape.steps, (r + 1) * shape.steps) {
                    Ok(()) => {
                        lengths += state.grown.len() as i64;
                        hashes = (hashes + state.grown.hash()) % 1_000_003;
                    }
                    Err(()) => lengths -= 1,
                }
            }
            format!("{} {lengths} {hashes}", shape.rounds)
        }
    }
}

fn shown(shape: &Shape, state: &State) -> String {
    let mut out = String::new();
    match shape.acc {
        Acc::Bare => {}
        Acc::Tuple => write!(out, "{} ", state.n).unwrap(),
        Acc::Record => write!(out, "{} {} ", state.n, TAG).unwrap(),
        Acc::Nested => write!(out, "{} {} {} ", state.n, state.n, TAG).unwrap(),
    }
    let len = state.grown.len();
    match &state.grown {
        Grown::Ints(v) => write!(
            out,
            "{len} {} {} {}",
            v.first().copied().unwrap_or(-1),
            v.last().copied().unwrap_or(-1),
            state.grown.hash()
        )
        .unwrap(),
        Grown::Strs(v) => write!(
            out,
            "{len} {} {} {}",
            v.first().map_or("-", String::as_str),
            v.last().map_or("-", String::as_str),
            state.grown.hash()
        )
        .unwrap(),
        Grown::Text(s) => {
            let head = &s[..s.len().min(8)];
            let tail = &s[s.len().saturating_sub(8)..];
            write!(out, "{len} {head} {tail} {}", state.grown.hash()).unwrap();
        }
    }
    out
}

const TAG: i64 = 42;

// ---------------------------------------------------------------------------
// The program
// ---------------------------------------------------------------------------

/// The imports every case opens with, which a batch writes once.
pub const IMPORTS: &str = r#"from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator };
"#;

/// How every case's `main` starts, which a batch writes once.
pub const MAIN: &str = "export fn main(host: NativeHost): Result<(), Str> {";

/// How every case's `main` starts once it is formatted, which a batch drops.
pub const MAIN_START: &str = "export fn main(";

/// Names and types one case's text is written with.
#[derive(Clone)]
struct Names {
    p: String,
    value_ty: &'static str,
    /// The record a `Record` accumulator is, and a `Nested` one holds.
    rec_ty: String,
    acc_ty: String,
    /// The type a driver returns.
    out_ty: String,
}

impl Names {
    fn of(id: usize, shape: &Shape) -> Names {
        let p = format!("g{id:03}");
        let value_ty = match shape.value {
            Value::Ints => "[Int]",
            Value::Strs | Value::Names => "[Str]",
            Value::Text => "Str",
        };
        let rec_ty = format!("G{id:03}Acc");
        let acc_ty = match shape.acc {
            Acc::Bare => String::from(value_ty),
            Acc::Tuple => format!("(Int, {value_ty})"),
            Acc::Record => rec_ty.clone(),
            Acc::Nested => format!("G{id:03}Outer"),
        };
        let out_ty = result_of(shape, &acc_ty);
        Names { p, value_ty, rec_ty, acc_ty, out_ty }
    }
}

/// What a step over `ty` returns: `ty`, or a `Result` of it when it can fail.
fn result_of(shape: &Shape, ty: &str) -> String {
    if shape.fallible() { format!("Result<{ty}, Str>") } else { String::from(ty) }
}

/// The text of one case, header and all.
pub fn source(id: usize, shape: &Shape, blocks: u64) -> String {
    let n = Names::of(id, shape);
    let p = &n.p;
    let mut out = String::new();
    writeln!(out, "// Generated by `cli/tests/native/growth/generator.rs`; do not edit.").unwrap();
    writeln!(out, "// {}", shape.dimensions().join(" ")).unwrap();
    writeln!(out, "// expect: {}", expected(shape)).unwrap();
    writeln!(out, "// blocks: {blocks}").unwrap();
    out.push_str(IMPORTS);
    out.push('\n');

    if matches!(shape.acc, Acc::Record | Acc::Nested) {
        writeln!(out, "struct {} {{ n: Int, items: {}, tag: Int }}\n", n.rec_ty, n.value_ty).unwrap();
    }
    helpers(&mut out, shape, &n);
    if shape.acc == Acc::Nested {
        writeln!(out, "struct {} {{ inner: {}, k: Int }}\n", n.acc_ty, n.rec_ty).unwrap();
        // The record inside is stepped by a helper, the way a record is
        // stepped at the top.
        let inner = Shape { acc: Acc::Record, stop: None, ..shape.clone() };
        let names = Names { acc_ty: n.rec_ty.clone(), out_ty: result_of(shape, &n.rec_ty), ..n.clone() };
        writeln!(out, "fn {p}_inner<C: Allocator>(ctx: C, acc: {}, i: Int): {} {{", names.acc_ty, names.out_ty)
            .unwrap();
        writeln!(out, "  {}", step_body(&inner, &names)).unwrap();
        out.push_str("}\n\n");
    }
    if shape.inner != Driver::FoldLambda {
        writeln!(out, "fn {p}_step<C: Allocator>(ctx: C, acc: {}, i: Int): {} {{", n.acc_ty, n.out_ty)
            .unwrap();
        writeln!(out, "  {}", step_body(shape, &n)).unwrap();
        out.push_str("}\n\n");
    }

    // The innermost driver calls the step; a seeded outer driver calls a whole
    // inner run as its step.
    driver(&mut out, shape, &n, &format!("{p}_run"), shape.inner, &format!("{p}_step(ctx, acc, i)"), true);
    if shape.nest == Nest::Seeded {
        let round = format!("{p}_run(ctx, acc, i * {}, {})", shape.steps, shape.steps);
        driver(&mut out, shape, &n, &format!("{p}_rounds"), shape.outer, &round, false);
    }
    summary(&mut out, shape, &n);
    entry(&mut out, shape, &n);

    writeln!(out, "{MAIN}").unwrap();
    writeln!(out, "  let _ = io.println(host.stdout, {p}(host.alloc)).ignore();").unwrap();
    out.push_str("  .Ok(())\n}\n");
    // Laid out the way `buri format` lays it out, since every source in the
    // repository is held to that.
    buri::commands::format::file(&format!("case_{id:03}.buri"), &out)
        .unwrap_or_else(|| panic!("the formatter refused case {id}:\n{out}"))
}

fn empty_value(shape: &Shape) -> &'static str {
    match shape.value {
        Value::Ints => "list.empty<Int>()",
        Value::Strs | Value::Names => "list.empty<Str>()",
        Value::Text => "\"\"",
    }
}

fn empty_acc(shape: &Shape, n: &Names) -> String {
    let v = empty_value(shape);
    match shape.acc {
        Acc::Bare => String::from(v),
        Acc::Tuple => format!("(0, {v})"),
        Acc::Record => format!("{} {{ n: 0, items: {v}, tag: {TAG} }}", n.acc_ty),
        Acc::Nested => {
            format!("{} {{ inner: {} {{ n: 0, items: {v}, tag: {TAG} }}, k: 0 }}", n.acc_ty, n.rec_ty)
        }
    }
}

/// `x` grown by the element coded by `code`.
fn grow_expr(shape: &Shape, n: &Names, x: &str, code: &str) -> String {
    let p = &n.p;
    let elem = |code: &str| element(shape, p, code);
    let grow = |x: &str, e: String| match shape.value {
        Value::Text => format!("{x}.concat(ctx, {e})"),
        _ => format!("{x}.push(ctx, {e})"),
    };
    match shape.pusher {
        Pusher::Inline => grow(x, elem(code)),
        Pusher::Small => format!("{p}_add(ctx, {x}, {code})"),
        Pusher::Large => format!(
            "if (i % 2 == 0) {{ {p}_put(ctx, {x}, {code}, 0) }} else {{ {p}_put(ctx, {x}, {code}, 1) }}"
        ),
    }
}

/// The element coded by `code`.
fn element(shape: &Shape, p: &str, code: &str) -> String {
    match shape.value {
        Value::Ints => String::from(code),
        Value::Names => format!("str.fromInt(ctx, {code})"),
        Value::Strs | Value::Text => format!("{p}_pick({code})"),
    }
}

/// `ctx` is a keyword everywhere but a function's parameter list, so a lambda
/// names its context `c`.
fn in_lambda(text: &str) -> String {
    text.replace("ctx", "c")
}

fn length_of(x: &str) -> String {
    format!("{x}.length()")
}

fn helpers(out: &mut String, shape: &Shape, n: &Names) {
    let p = &n.p;
    let v = n.value_ty;
    if matches!(shape.value, Value::Strs | Value::Text) {
        writeln!(
            out,
            "fn {p}_pick(code: Int): Str {{\n  if (code % 3 == 0) {{ \"a\" }} else if (code % 3 == 1) {{ \"bb\" }} else {{ \"ccc\" }}\n}}\n"
        )
        .unwrap();
    }
    let elem = |code: &str| element(shape, p, code);
    let grow = |e: String| match shape.value {
        Value::Text => format!("items.concat(ctx, {e})"),
        _ => format!("items.push(ctx, {e})"),
    };
    match shape.pusher {
        Pusher::Inline => {}
        Pusher::Small => writeln!(
            out,
            "fn {p}_add<C: Allocator>(ctx: C, items: {v}, code: Int): {v} {{ {} }}\n",
            grow(elem("code"))
        )
        .unwrap(),
        Pusher::Large => writeln!(
            out,
            "fn {p}_put<C: Allocator>(ctx: C, items: {v}, code: Int, salt: Int): {v} {{\n  let k = code + salt;\n  if (k < 0) {{ items }} else {{ {} }}\n}}\n",
            grow(elem("k"))
        )
        .unwrap(),
    }
    match shape.reader {
        Reader::None | Reader::Inline => {}
        Reader::Small => {
            writeln!(out, "fn {p}_measure(items: {v}): Int {{ {} }}\n", length_of("items"))
                .unwrap()
        }
        Reader::Large => writeln!(
            out,
            "fn {p}_survey(items: {v}): Int {{\n  let size = {};\n  if (size == 0) {{ 0 }} else if (size % 2 == 0) {{ size }} else {{ size + 0 }}\n}}\n",
            length_of("items")
        )
        .unwrap(),
    }
    if shape.fallible() {
        writeln!(
            out,
            "fn {p}_gate(i: Int): Result<Int, Str> {{\n  if (i == {}) {{ .Err(\"bad\") }} else {{ .Ok(i % 5) }}\n}}\n",
            shape.fail.unwrap()
        )
        .unwrap();
    }
}

/// The step: take the value out of the accumulator, read it, grow it, and put
/// it back.
fn step_body(shape: &Shape, n: &Names) -> String {
    let body = if shape.acc == Acc::Nested { nested_body(shape, n) } else { flat_body(shape, n) };
    // A fold has nowhere else to stop, so its step hands the accumulator back
    // untouched; the recursive drivers stop themselves.
    let stops_here = matches!(shape.inner, Driver::Fold | Driver::FoldLambda);
    match shape.stop {
        Some(limit) if stops_here => {
            let back = if shape.fallible() { ".Ok(acc)" } else { "acc" };
            format!("if ({} >= {limit}) {{ {back} }} else {{ {body} }}", value_length(shape, "acc"))
        }
        _ => body,
    }
}

/// A `Nested` step: take the inner record out, hand it to the helper that
/// steps it, and put what comes back in.
fn nested_body(shape: &Shape, n: &Names) -> String {
    let p = &n.p;
    let o = &n.acc_ty;
    let q = if shape.fallible() { "?" } else { "" };
    let wrap = |e: String| if shape.fallible() { format!(".Ok({e})") } else { e };
    let (mut open, mut x) = match shape.access {
        Access::Destructure => (format!("let {o} {{ inner, k }} = acc; "), String::from("inner")),
        Access::Field => (String::new(), String::from("acc.inner")),
        Access::Match => (String::new(), String::from("inner")),
    };
    if shape.alias {
        open.push_str(&format!("let held = {x}; "));
        x = String::from("held");
    }
    let call = format!("{p}_inner(ctx, {x}, i){q}");
    match shape.access {
        Access::Destructure => format!("{open}{}", wrap(format!("{o} {{ inner: {call}, k: k + 1 }}"))),
        Access::Field => format!("{open}{}", wrap(format!("{o} {{ ..acc, k: acc.k + 1, inner: {call} }}"))),
        Access::Match => format!(
            "match (acc) {{ {o} {{ inner, k }} => {{ {open}{} }}, }}",
            wrap(format!("{o} {{ inner: {call}, k: k + 1 }}"))
        ),
    }
}

/// A step over a bare value, a tuple, or a record.
fn flat_body(shape: &Shape, n: &Names) -> String {
    let p = &n.p;
    let mut lets = String::new();
    if shape.fallible() {
        lets.push_str(&format!("let q = {p}_gate(i)?; "));
    }
    // Where the value comes from, and how the new accumulator is built from
    // the grown one.
    type Rebuild = Box<dyn Fn(&str) -> String>;
    let (open, value, rebuild): (String, String, Rebuild) =
        match (shape.acc, shape.access) {
            (Acc::Bare, Access::Destructure) => {
                (String::from("let items = acc; "), String::from("items"), Box::new(|g| g.to_string()))
            }
            (Acc::Bare, _) => (String::new(), String::from("acc"), Box::new(|g| g.to_string())),
            (Acc::Tuple, Access::Destructure) => (
                String::from("let (n, items) = acc; "),
                String::from("items"),
                Box::new(|g| format!("(n + 1, {g})")),
            ),
            (Acc::Tuple, Access::Field) => {
                (String::new(), String::from("acc.1"), Box::new(|g| format!("(acc.0 + 1, {g})")))
            }
            (Acc::Tuple, Access::Match) => (
                String::new(),
                String::from("items"),
                Box::new(|g| format!("(n + 1, {g})")),
            ),
            (Acc::Record, Access::Destructure) => {
                let ty = n.acc_ty.clone();
                (
                    format!("let {ty} {{ n, items, tag }} = acc; "),
                    String::from("items"),
                    Box::new(move |g| format!("{ty} {{ n: n + 1, items: {g}, tag }}")),
                )
            }
            (Acc::Record, Access::Field) => {
                let ty = n.acc_ty.clone();
                (
                    String::new(),
                    String::from("acc.items"),
                    Box::new(move |g| format!("{ty} {{ ..acc, n: acc.n + 1, items: {g} }}")),
                )
            }
            (Acc::Record, Access::Match) => {
                let ty = n.acc_ty.clone();
                (
                    String::new(),
                    String::from("items"),
                    Box::new(move |g| format!("{ty} {{ n: n + 1, items: {g}, tag }}")),
                )
            }
            (Acc::Nested, _) => panic!("`nested_body` writes a nested step"),
        };
    let mut body = open;
    let mut x = value;
    if shape.alias {
        body.push_str(&format!("let held = {x}; "));
        x = String::from("held");
    }
    let mut code = format!("i * {} + {}", shape.scale, shape.offset);
    match shape.reader {
        Reader::None => {}
        Reader::Inline => {
            body.push_str(&format!("let seen = {}; ", length_of(&x)));
            code.push_str(" + seen % 7");
        }
        Reader::Small => {
            body.push_str(&format!("let seen = {p}_measure({x}); "));
            code.push_str(" + seen % 7");
        }
        Reader::Large => {
            body.push_str(&format!("let seen = {p}_survey({x}) + {p}_survey({x}); "));
            code.push_str(" + seen % 7");
        }
    }
    if shape.fallible() {
        code.push_str(" + q");
    }
    let grown = if shape.acc == Acc::Bare && shape.access == Access::Match {
        format!(
            "match (i % 2) {{ 0 => {}, _ => {}, }}",
            grow_expr(shape, n, &x, &code),
            grow_expr(shape, n, &x, &format!("{code} + 1"))
        )
    } else {
        grow_expr(shape, n, &x, &code)
    };
    let mut done = rebuild(&grown);
    if shape.fallible() {
        done = format!(".Ok({done})");
    }
    body.push_str(&done);
    let body = match (shape.acc, shape.access) {
        (Acc::Tuple, Access::Match) => format!("match (acc) {{ (n, items) => {{ {body} }}, }}"),
        (Acc::Record, Access::Match) => {
            format!("match (acc) {{ {} {{ n, items, tag }} => {{ {body} }}, }}", n.acc_ty)
        }
        _ => body,
    };
    format!("{lets}{body}")
}

/// The length of the value inside an accumulator named `acc`.
fn value_length(shape: &Shape, acc: &str) -> String {
    match shape.acc {
        Acc::Bare => length_of(acc),
        Acc::Tuple => length_of(&format!("{acc}.1")),
        Acc::Record => length_of(&format!("{acc}.items")),
        Acc::Nested => length_of(&format!("{acc}.inner.items")),
    }
}

/// A driver named `name`, `(ctx, acc, base, count)`, that applies `step` (an
/// expression in `ctx`, `acc` and `i`) for each `i` from `base`.
fn driver(out: &mut String, shape: &Shape, n: &Names, name: &str, kind: Driver, step: &str, innermost: bool) {
    let acc_ty = &n.acc_ty;
    let out_ty = &n.out_ty;
    let fallible = shape.fallible();
    let stop = if innermost { shape.stop } else { None };
    writeln!(out, "fn {name}<C: Allocator>(ctx: C, acc: {acc_ty}, base: Int, count: Int): {out_ty} {{")
        .unwrap();
    match kind {
        Driver::Fold | Driver::FoldLambda => {
            let fold = if fallible { "foldResultCtx" } else { "foldCtx" };
            let lambda = if kind == Driver::FoldLambda && innermost {
                format!("fn(c, acc: {acc_ty}, i) => {{ {} }}", in_lambda(&step_body(shape, n)))
            } else {
                format!("fn(c, acc, i) => {}", in_lambda(step))
            };
            writeln!(out, "  list.range(ctx, base, base + count).{fold}(ctx, {lambda}, acc)").unwrap();
        }
        Driver::Loop => {
            let next = if fallible { format!("{step}?") } else { String::from(step) };
            let done = if fallible { ".Ok(acc)" } else { "acc" };
            let halt = match stop {
                Some(limit) => format!("i == end || {} >= {limit}", value_length(shape, "acc")),
                None => String::from("i == end"),
            };
            writeln!(out, "  {name}Loop(ctx, acc, base, base + count)\n}}\n").unwrap();
            writeln!(
                out,
                "fn {name}Loop<C: Allocator>(ctx: C, acc: {acc_ty}, i: Int, end: Int): {out_ty} {{"
            )
            .unwrap();
            writeln!(
                out,
                "  if ({halt}) {{ {done} }} else {{ {name}Loop(ctx, {next}, i + 1, end) }}"
            )
            .unwrap();
        }
        Driver::Recursion => {
            let done = if fallible { ".Ok(acc)" } else { "acc" };
            let q = if fallible { "?" } else { "" };
            // The step reads `acc` and `i`, so the earlier steps' answer is
            // bound to those names.
            let step_here = step;
            let rest = match stop {
                Some(limit) => {
                    let back = if fallible { ".Ok(acc)" } else { "acc" };
                    format!(
                        "let acc = {name}(ctx, acc, base, count - 1){q}; let i = base + count - 1; if ({} >= {limit}) {{ {back} }} else {{ {step_here} }}",
                        value_length(shape, "acc")
                    )
                }
                None => format!(
                    "let acc = {name}(ctx, acc, base, count - 1){q}; let i = base + count - 1; {step_here}"
                ),
            };
            writeln!(out, "  if (count == 0) {{ {done} }} else {{ {rest} }}").unwrap();
        }
    }
    out.push_str("}\n\n");
}

fn summary(out: &mut String, shape: &Shape, n: &Names) {
    let p = &n.p;
    let v = n.value_ty;
    match shape.value {
        Value::Ints => writeln!(
            out,
            "fn {p}_hash(items: {v}): Int {{ items.fold(fn(h, x) => (h * 31 + x) % 1000003, 7) }}\n"
        )
        .unwrap(),
        Value::Strs | Value::Names => writeln!(
            out,
            "fn {p}_hash(items: {v}): Int {{ items.fold(fn(h, x) => (h * 31 + x.length()) % 1000003, 7) }}\n"
        )
        .unwrap(),
        Value::Text => writeln!(
            out,
            "fn {p}_hash(items: {v}): Int {{ {p}_walk(items, 0, 7) }}\n\nfn {p}_walk(text: Str, at: Int, h: Int): Int {{\n  match (text.charAt(at)) {{\n    .Some(c) => {p}_walk(text, at + 1, (h * 31 + if (c == 'a') {{ 1 }} else if (c == 'b') {{ 2 }} else {{ 3 }}) % 1000003),\n    .None => h,\n  }}\n}}\n"
        )
        .unwrap(),
    }
}

fn entry(out: &mut String, shape: &Shape, n: &Names) {
    let p = &n.p;
    let run = |seed: &str, base: &str| {
        if shape.nest == Nest::Seeded {
            format!("{p}_rounds(ctx, {seed}, 0, {})", shape.rounds)
        } else {
            format!("{p}_run(ctx, {seed}, {base}, {})", shape.steps)
        }
    };
    let value = |acc: &str| match shape.acc {
        Acc::Bare => String::from(acc),
        Acc::Tuple => format!("{acc}.1"),
        Acc::Record => format!("{acc}.items"),
        Acc::Nested => format!("{acc}.inner.items"),
    };
    writeln!(out, "fn {p}<C: Allocator>(ctx: C): Str {{").unwrap();
    match shape.nest {
        Nest::None | Nest::Seeded => {
            let got = run(&empty_acc(shape, n), "0");
            let items = value("acc");
            let prefix = match shape.acc {
                Acc::Bare => "",
                Acc::Tuple => "${acc.0} ",
                Acc::Record => "${acc.n} ${acc.tag} ",
                Acc::Nested => "${acc.k} ${acc.inner.n} ${acc.inner.tag} ",
            };
            let ends = match shape.value {
                Value::Ints => format!(
                    "let first = match ({items}.first()) {{ .Some(x) => x, .None => -1, }}; let last = match ({items}.last()) {{ .Some(x) => x, .None => -1, }};"
                ),
                Value::Strs | Value::Names => format!(
                    "let first = match ({items}.first()) {{ .Some(x) => x, .None => \"-\", }}; let last = match ({items}.last()) {{ .Some(x) => x, .None => \"-\", }};"
                ),
                Value::Text => format!(
                    "let size = {items}.length(); let first = {items}.slice(0, 8); let last = {items}.slice(size - 8, size);"
                ),
            };
            let show = format!(
                "{ends} let hash = {p}_hash({items}); str.format(ctx, \"{prefix}${{{}}} ${{first}} ${{last}} ${{hash}}\")",
                length_of(&items)
            );
            if shape.snapshot {
                let half = shape.steps / 2;
                let mid = value("mid");
                let show = show.replace(
                    "${hash}\")",
                    &format!("${{hash}} | ${{{}}} ${{kept}}\")", length_of(&mid)),
                );
                writeln!(
                    out,
                    "  let mid = {p}_run(ctx, {}, 0, {half});\n  let acc = {p}_run(ctx, mid, {half}, {});\n  let kept = {p}_hash({mid});\n  {show}",
                    empty_acc(shape, n),
                    shape.steps - half
                )
                .unwrap();
            } else if shape.fallible() {
                writeln!(
                    out,
                    "  match ({got}) {{\n    .Ok(acc) => {{ {show} }},\n    .Err(e) => str.format(ctx, \"stopped ${{e}}\"),\n  }}"
                )
                .unwrap();
            } else {
                writeln!(out, "  let acc = {got};\n  {show}").unwrap();
            }
        }
        Nest::Mapped => {
            let got = run(&empty_acc(shape, n), &format!("r * {}", shape.steps));
            writeln!(
                out,
                "  let all = list.range(ctx, 0, {}).mapCtx(ctx, fn(c, r) => {});",
                shape.rounds,
                in_lambda(&got)
            )
            .unwrap();
            let len = length_of(&value("acc"));
            let hash = format!("{p}_hash({})", value("acc"));
            if shape.fallible() {
                writeln!(out, "  let lengths = all.fold(fn(t, got) => t + match (got) {{ .Ok(acc) => {len}, .Err(_e) => -1, }}, 0);").unwrap();
                writeln!(out, "  let hashes = all.fold(fn(t, got) => match (got) {{ .Ok(acc) => (t + {hash}) % 1000003, .Err(_e) => t, }}, 0);").unwrap();
            } else {
                writeln!(out, "  let lengths = all.fold(fn(t, acc) => t + {len}, 0);").unwrap();
                writeln!(out, "  let hashes = all.fold(fn(t, acc) => (t + {hash}) % 1000003, 0);").unwrap();
            }
            writeln!(out, "  str.format(ctx, \"${{all.length()}} ${{lengths}} ${{hashes}}\")").unwrap();
        }
    }
    out.push_str("}\n\n");
}

// ---------------------------------------------------------------------------
// The bound
// ---------------------------------------------------------------------------

/// What a batch's `main` allocates beside its cases.
pub const BATCH_BLOCKS: u64 = 8;

/// At most how many blocks a case allocates when every value it grows grows in
/// place: a block per doubling of each value, and a few per run of a driver
/// for its range and its closure. A push that copies allocates a block per
/// push, which every case has hundreds of.
pub fn blocks(shape: &Shape) -> u64 {
    let runs = shape.rounds as u64 + 1;
    let values = if shape.nest == Nest::Mapped { shape.rounds as u64 } else { 1 };
    let longest = shape.total() as u64 * 3;
    let doublings = 64 - longest.leading_zeros() as u64;
    // A built element is a block of its own.
    let elements = if shape.value == Value::Names { shape.total() as u64 } else { 0 };
    // The kept half is copied once, and the copy grows on.
    let copies = if shape.snapshot { doublings + 6 } else { 0 };
    8 + 4 * runs + values * (doublings + 4) + elements + copies
}
