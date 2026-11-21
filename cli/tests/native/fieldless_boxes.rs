//! Recursive enums whose variants have no fields, such as a tree's `.Leaf`:
//! what they answer, on every native backend built in, under the heap check,
//! and how many blocks a tree costs, counted through the allocation probe.

use crate::collection_costs::per_op;
use crate::shared::{heap_checked, probed, ran_command};

/// A tree built and walked, and an expression whose fieldless variants sit
/// before and after its variants with fields. `main` takes the mode and its
/// count: `trees` builds that many trees of depth [`DEPTH`], `exprs` that many
/// expressions, `many` boxes [`many`]'s variant of that index twice and reads
/// it back, and `none` does nothing.
const PROGRAM: &str = r#"
from "core/env" import * as env;
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

derive Equal for Tree;

enum Tree {
  Leaf,
  Node(Tree, Int, Tree),
}

enum Expr {
  Zero,
  One,
  Neg(Expr),
  Add(Expr, Expr),
  Two,
}

MANY

fn wrapped(k: Int, d: Int): Many {
  if (d == 0) { pick(k) } else { .Wrap(wrapped(k, d - 1)) }
}

fn unwrapped(m: Many): Int {
  match (m) {
    .Wrap(inner) => 1000 + unwrapped(inner),
    _ => index(m),
  }
}

fn build(d: Int, v: Int): Tree {
  if (d == 0) { .Leaf } else { .Node(build(d - 1, v * 2), v, build(d - 1, v * 2 + 1)) }
}

fn sum(t: Tree): Int {
  match (t) {
    .Leaf => 1,
    .Node(l, v, r) => sum(l) + v % 7 + sum(r),
  }
}

fn trees(i: Int, n: Int, acc: Int): Int {
  if (i >= n) {
    acc
  } else {
    let t = build(DEPTH, i);
    let same = if (t == build(DEPTH, i)) { 1 } else { 0 };
    trees(i + 1, n, acc + sum(t) + same)
  }
}

fn leaf(k: Int): Expr {
  match (k % 3) {
    0 => .Zero,
    1 => .One,
    _ => .Two,
  }
}

fn expr(k: Int): Expr {
  if (k == 0) { .Two } else if (k % 2 == 0) { .Add(leaf(k), expr(k - 1)) } else { .Neg(.Add(expr(k - 1), leaf(k))) }
}

fn eval(e: Expr): Int {
  match (e) {
    .Zero => 0,
    .One => 1,
    .Two => 2,
    .Neg(x) => 0 - eval(x),
    .Add(a, b) => eval(a) + eval(b),
  }
}

fn show<C: Allocator>(ctx: C, e: Expr): Str {
  match (e) {
    .Zero => "0",
    .One => "1",
    .Two => "2",
    .Neg(x) => "-".concat(ctx, show(ctx, x)),
    .Add(a, b) => "(".concat(ctx, show(ctx, a)).concat(ctx, "+").concat(ctx, show(ctx, b)).concat(ctx, ")"),
  }
}

fn exprs<C: Allocator>(ctx: C, i: Int, n: Int, acc: Int): Int {
  if (i >= n) { acc } else { exprs(ctx, i + 1, n, acc + eval(expr(i % 9)) + show(ctx, expr(i % 5)).length()) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let n = args.get(1).andThen(fn(s) => s.toInt()).withDefault(0);
  let answer = match (args.first()) {
    .Some("trees") => trees(0, n, 0),
    .Some("exprs") => exprs(ctx, 0, n, 0),
    .Some("many") => unwrapped(wrapped(n, 2)),
    .Some("shown") => {
      let _ = io.println(ctx, show(ctx, expr(6))).ignore();
      eval(expr(6))
    },
    _ => 0,
  };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// How many fieldless variants `Many` has: more than 64, so a tag past every
/// bit of a 64-bit word is boxed too.
const MANY: u64 = 70;

/// `enum Many`, whose fieldless variants are `M0` to `M69` and whose last
/// variant `Wrap` boxes one, with `pick(k)` making variant `Mk` and
/// `index(m)` answering `k`.
fn many() -> String {
    let names: Vec<String> = (0..MANY).map(|k| format!("M{k}")).collect();
    let picks: Vec<String> = (0..MANY).map(|k| format!("    {k} => .M{k},")).collect();
    let indexes: Vec<String> = (0..MANY).map(|k| format!("    .M{k} => {k},")).collect();
    format!(
        "enum Many {{ {}, Wrap(Many) }}\n\nfn pick(k: Int): Many {{\n  match (k) {{\n{}\n    _ => .M0,\n  }}\n}}\n\nfn index(m: Many): Int {{\n  match (m) {{\n{}\n    .Wrap(_) => -1,\n  }}\n}}",
        names.join(", "),
        picks.join("\n"),
        indexes.join("\n")
    )
}

/// The depth of each tree `trees` builds: 2^10 leaves.
const DEPTH: u64 = 10;

/// How many trees the counted run builds.
const TREES: u64 = 40;

/// [`PROGRAM`] with [`DEPTH`] put in.
fn program() -> String {
    PROGRAM
        .replace("DEPTH", &DEPTH.to_string())
        .replace("MANY", &many())
}

/// What `trees` prints for `n` trees: each tree's walk, plus one for the
/// second build comparing equal.
fn trees_answer(n: u64) -> String {
    fn sum(d: u64, v: u64) -> u64 {
        if d == 0 {
            1
        } else {
            sum(d - 1, v * 2) + v % 7 + sum(d - 1, v * 2 + 1)
        }
    }
    let total: u64 = (0..n).map(|i| sum(DEPTH, i) + 1).sum();
    format!("{total}\n")
}

/// [`PROGRAM`]'s `Expr`, with its three fieldless variants as one.
enum E {
    Leaf(i64),
    Neg(Box<E>),
    Add(Box<E>, Box<E>),
}

/// [`PROGRAM`]'s `expr`.
fn expr(k: i64) -> E {
    let leaf = |k: i64| E::Leaf(k % 3);
    if k == 0 {
        E::Leaf(2)
    } else if k % 2 == 0 {
        E::Add(Box::new(leaf(k)), Box::new(expr(k - 1)))
    } else {
        E::Neg(Box::new(E::Add(Box::new(expr(k - 1)), Box::new(leaf(k)))))
    }
}

/// [`PROGRAM`]'s `eval`.
fn eval(e: &E) -> i64 {
    match e {
        E::Leaf(v) => *v,
        E::Neg(x) => -eval(x),
        E::Add(a, b) => eval(a) + eval(b),
    }
}

/// [`PROGRAM`]'s `show`.
fn show(e: &E) -> String {
    match e {
        E::Leaf(v) => v.to_string(),
        E::Neg(x) => format!("-{}", show(x)),
        E::Add(a, b) => format!("({}+{})", show(a), show(b)),
    }
}

/// What `exprs` prints for `n` expressions.
fn exprs_answer(n: i64) -> String {
    let total: i64 = (0..n)
        .map(|i| eval(&expr(i % 9)) + show(&expr(i % 5)).len() as i64)
        .sum();
    format!("{total}\n")
}

/// **A variant with no fields, put in a boxed field, answers as it was
/// built**: matched, compared, walked and shown, whichever position its
/// variant has among the ones with fields, and whichever tag it has in an
/// enum of more than 64 variants.
#[test]
fn fieldless_variants_in_boxed_fields_read_back_as_built() {
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("fieldless-reads", &program());
        for (args, want) in [
            (["exprs", "50"], exprs_answer(50)),
            (["trees", "3"], trees_answer(3)),
            (["many", "3"], "2003\n".to_string()),
            (["many", "15"], "2015\n".to_string()),
            (["many", "16"], "2016\n".to_string()),
            (["many", "69"], "2069\n".to_string()),
        ] {
            let mut cmd = std::process::Command::new(&binary);
            cmd.args(args);
            let r = ran_command(heap_checked(&mut cmd));
            assert_eq!(r.status, 0, "{backend} {args:?}: {}", r.stderr);
            assert_eq!(r.stdout, want, "{backend} {args:?}");
            assert_eq!(
                probed(&r.stderr).1,
                0,
                "{backend} {args:?}: blocks live at exit"
            );
        }
        let mut cmd = std::process::Command::new(&binary);
        cmd.arg("shown");
        let r = ran_command(heap_checked(&mut cmd));
        let e = expr(6);
        assert_eq!(r.status, 0, "{backend} shown: {}", r.stderr);
        assert_eq!(
            r.stdout,
            format!("{}\n{}\n", show(&e), eval(&e)),
            "{backend} shown"
        );
    }
}

/// **A tree's `.Leaf` costs no block on the copy-and-patch backend.** Each
/// node's two children are boxed, so a tree of depth 10 boxed 2,046 values,
/// 1,024 of them leaves. A leaf is now `buri_rt_fieldless`'s block for its
/// tag. Each tree is built twice, so a tree cost 4,092 blocks and now costs
/// 2,044. The bound sits between the two, and blocks are counted exactly, so
/// the gap needs no headroom for noise. The floor, half the new count, only
/// says the probe counted the nodes' boxes at all.
#[test]
fn a_leaf_in_a_boxed_field_costs_no_block() {
    let boxes = 2 * ((2u64 << DEPTH) - 2);
    let bound = boxes * 3 / 4;
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("fieldless-blocks", &program());
        let count = TREES.to_string();
        let (n, blocks) = per_op(
            backend,
            &binary,
            "trees",
            &[&count],
            TREES,
            &trees_answer(TREES),
        );
        let each = blocks / TREES;
        eprintln!("{backend}: {n:?} instructions and {each} blocks a tree");
        assert!(each >= boxes / 4, "{backend}: {each} blocks a tree");
        if backend == "stencil" {
            assert!(
                each <= bound,
                "{backend}: {each} blocks a tree, over {bound}"
            );
        }
    }
}
