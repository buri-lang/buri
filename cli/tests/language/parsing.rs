//! What the parser reads where its fast paths stop: a name or a literal
//! against what follows it, a bare type or binding against a path, the
//! nesting and chain budgets either side of their limits, each literal as its
//! value, and a file denser than the parser sets room aside for.
use crate::harness::*;

/// `//app`, a JavaScript binary whose `main.buri` is `source`.
fn app(source: &str) -> Scratch {
    let scratch = Scratch::repo("parsing");
    scratch.binary_package("app", source);
    scratch
}

const NODE_MAIN: &str = "from \"node\" import { NodeHost };\n\n";

/// `main`, with `body` before its `.Ok(())`, and `items` above it.
fn main_with(items: &str, body: &str) -> String {
    format!("{NODE_MAIN}{items}export fn main(host: NodeHost): Result<(), Str> {{\n{body}    .Ok(())\n}}\n")
}

/// **Operands, operators and postfix chains read as written**: precedence,
/// grouping, calls, fields, tuple elements, `?`, type arguments, struct
/// literals, bare and generic types, and bindings beside paths and
/// alternatives.
#[test]
fn expressions_read_as_written() {
    let scratch = app(r#"from "node" import { NodeHost };
from "platform/effect" import { Allocator, Stdout };
from "core/io" import * as io;
from "core/list" import * as list;

struct Point {
    x: Int,
    y: Int,
}

impl Point {
    fn sum(self): Int {
        self.x + self.y
    }
}

enum Shape {
    Dot,
    Square(Int),
    Rect { w: Int, h: Int },
}

fn area(s: Shape): Int {
    match (s) {
        .Dot => 0,
        .Square(side) => side * side,
        .Rect { w, h } => w * h,
    }
}

fn first(xs: [Int]): Result<Int, Str> {
    let n = xs.get(0).okOr("empty")?;
    .Ok(n + 1)
}

fn classify(n: Int): Int {
    match (n) {
        0 | 1 => 10,
        m => m * 2,
    }
}

export fn main(host: NodeHost): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
    let a = 1 + 2 * 3 - 4 / 2;
    let b = (1 + 2) * (3 - 4) % 5;
    let c = a < b || b <= a && !(a == b);
    let p = Point { x: a, y: b };
    let q = (p.sum(), p.x, -p.y);
    let empty = list.empty<Int>();
    let e = first(empty).withDefault(0) + first([41]).withDefault(0);
    let shapes = [area(.Dot), area(.Square(3)), area(.Rect { w: 2, h: 5 })];
    let _ = io.println(ctx, "${a} ${b} ${c} ${q.0} ${q.1} ${q.2} ${e} ${shapes} ${classify(1)} ${classify(7)}").ignore();
    .Ok(())
}
"#);
    scratch.run(&["build", "//app"]).ok();
    scratch
        .exec_js("app")
        .ok()
        .says("5 -3 true 2 5 3 42 [0, 9, 10] 10 14");
}

/// **Each budget is spent the same at its limit**: the deepest nesting and
/// the longest chain parse, and one more is refused with its own diagnostic.
#[test]
fn nesting_and_chains_are_refused_one_past_their_limits() {
    let deep = |n: usize| ("(".repeat(n) + "1" + &")".repeat(n), String::new());
    let negated = |n: usize| ("-".repeat(n) + "1", String::new());
    let summed = |n: usize| ("1".to_string() + &" + 1".repeat(n), String::new());
    let typed = |n: usize| {
        let ty = "[".repeat(n) + "Int" + &"]".repeat(n);
        (
            "1".to_string(),
            format!("fn f(a: {ty}): Int {{\n    1\n}}\n\n"),
        )
    };
    // A shape at `n`: the value `main` binds, and the items above `main`.
    type Shape = fn(usize) -> (String, String);
    let cases: [(&str, Shape, usize, &str); 4] = [
        ("parentheses", deep, 126, "expression-too-deep"),
        ("prefix minus", negated, 253, "expression-too-deep"),
        ("a sum", summed, 2048, "chain-too-long"),
        ("array types", typed, 255, "expression-too-deep"),
    ];
    for (what, make, limit, code) in cases {
        for (n, refused) in [(limit, false), (limit + 1, true)] {
            let (value, items) = make(n);
            let scratch = app(&main_with(&items, &format!("    let _ = {value};\n")));
            let run = scratch.run(&["build", "//app"]);
            if refused {
                run.exits(1);
                run.says(&format!("[{code}]"));
            } else {
                run.ok();
            }
            assert_eq!(
                strip_ansi(&run.all()).contains(code),
                refused,
                "{what} at {n}:\n{}",
                indent(&run.all())
            );
        }
    }
}

/// **Each literal reads as the value written**: integers past 32 bits,
/// floats, strings with and without escapes, template pieces either side of
/// an interpolation, the negative and string literals of patterns, a tuple
/// element, and a literal the parser reads again after trying it as
/// something else.
#[test]
fn literals_read_as_their_values() {
    let scratch = app(r#"from "node" import { NodeHost };
from "platform/effect" import { Allocator, Stdout };
from "core/io" import * as io;

fn sign(n: Int): Int {
    match (n) {
        -1 => 100,
        0 => 200,
        4294967296 => 300,
        _ => 400,
    }
}

fn half(x: Float): Int {
    match (x) {
        -0.5 => 1,
        0.5 => 2,
        _ => 3,
    }
}

fn word(s: Str): Int {
    match (s) {
        "" => 10,
        "one" => 20,
        _ => 30,
    }
}

export fn main(host: NodeHost): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
    let big = 9007199254740993;
    let t = (1.25, 4294967297, "mid");
    let plain = "plain";
    let escaped = "a\tb";
    let a = 7;
    let b = "x";
    let pieces = "${a}-${b}";
    let around = "head ${a} mid ${b} tail";
    let tried = (2 < 5, 6 > 3);
    let _ = io.println(ctx, "${sign(-1)} ${sign(0)} ${sign(4294967296)} ${sign(3)}").ignore();
    let _ = io.println(ctx, "${half(-0.5)} ${half(0.5)} ${half(2.0)}").ignore();
    let _ = io.println(ctx, "${word("")} ${word("one")} ${word("two")}").ignore();
    let _ = io.println(ctx, "${big} ${t.0} ${t.1} ${t.2} ${tried.0} ${tried.1}").ignore();
    let _ = io.println(ctx, "${plain}|${escaped}|").ignore();
    let _ = io.println(ctx, pieces).ignore();
    let _ = io.println(ctx, around).ignore();
    .Ok(())
}
"#);
    scratch.run(&["build", "//app"]).ok();
    scratch.exec_js("app").ok().says(
        "100 200 300 400\n1 2 3\n10 20 30\n9007199254740993 1.25 4294967297 mid true true\nplain|a\tb|\n7-x\nhead 7 mid x tail",
    );
}

/// **A file denser than any the parser sizes for builds the same**: type
/// annotations nested three deep in every parameter, and a sum of hundreds of
/// calls, outgrow the room set aside for types and nodes up front.
#[test]
fn a_file_of_dense_types_and_calls_builds() {
    let n = 300;
    let ty = "[[[Int]]]";
    let items: String = (0..n)
        .map(|k| format!("fn f{k}(a: {ty}, b: {ty}, c: ({ty}, {ty})): Int {{\n    {k}\n}}\n\n"))
        .collect();
    let calls: Vec<String> = (0..n).map(|k| format!("f{k}([], [], ([], []))")).collect();
    let body = format!(
        "    let ctx = context {{ Allocator: host.alloc, Stdout: host.stdout }};\n    let total = {};\n    let _ = io.println(ctx, \"${{total}}\").ignore();\n",
        calls.join(" + ")
    );
    let imports = "from \"platform/effect\" import { Allocator, Stdout };\nfrom \"core/io\" import * as io;\n\n";
    let scratch = app(&main_with(&format!("{imports}{items}"), &body));
    scratch.run(&["build", "//app"]).ok();
    let total: usize = (0..n).sum();
    scratch.exec_js("app").ok().says(&total.to_string());
}
