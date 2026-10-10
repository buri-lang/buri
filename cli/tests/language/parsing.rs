//! What the parser reads where its fast paths stop: a name or a literal
//! against what follows it, a bare type or binding against a path, and the
//! nesting and chain budgets either side of their limits.
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
    scratch.exec_js("app").ok().says("5 -3 true 2 5 3 42 [0, 9, 10] 10 14");
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
        ("1".to_string(), format!("fn f(a: {ty}): Int {{\n    1\n}}\n\n"))
    };
    let cases: [(&str, &dyn Fn(usize) -> (String, String), usize, &str); 4] = [
        ("parentheses", &deep, 126, "expression-too-deep"),
        ("prefix minus", &negated, 253, "expression-too-deep"),
        ("a sum", &summed, 2048, "chain-too-long"),
        ("array types", &typed, 255, "expression-too-deep"),
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
