//! Writes Buri packages whose coverage is known by construction, and works out
//! what `buri test --coverage` and `--coverage=mcdc` must report for them.
//!
//! Nothing here runs coverage. Each shape below is a function with a fixed
//! layout and random parameters, and its model counts what a run of the test's
//! calls does: lines reached, branches taken, condition values seen. The
//! report's format (`design/COVERAGE.md`) turns the counts into text.
//!
//! The shapes:
//!
//! - an `if` over an `&&`/`||`/`!` chain of up to four conditions, laid out on
//!   one line or several, called on random truth tables, and such a chain in
//!   tail position;
//! - a recursion of N steps whose `if` is taken K times, through a `%`, and
//!   the same in tail position;
//! - a `match` on an `Int`, with guards, one of them an `&&`;
//! - `?` returning early on known inputs;
//! - a generic function at `Int`, `Str` and `Bool`, or at nothing;
//! - a lambda a fold calls once an element;
//! - an `&&` whose right side is a tail call, which MC/DC leaves to branches;
//! - derived `Equal`, `Ordered` and `Show` on an enum and a struct holding it,
//!   and an `&&` ending in a derived `==`;
//! - `/`, in a function generic over `Divide` or not, and a `core/bits` shift,
//!   each aborting a test of its own, and a `/` aborting partway through an
//!   `&&`.
//!
//! A third of the packages have a second package beside them whose suite
//! makes some of the calls, so counts add up across suites.

use std::collections::BTreeMap;
use std::fmt::Write as _;

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

    fn int(&mut self, low: i64, high: i64) -> i64 {
        low + self.below((high - low + 1) as usize) as i64
    }

    fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }

    fn bool(&mut self) -> bool {
        self.chance(2)
    }
}

/// One package, and sometimes a second one whose suite calls it too: their
/// files, by path from the repository root, and what each mode must report for
/// each counted file.
pub struct Package {
    pub files: Vec<(String, String)>,
    /// Each counted file, and its `[--coverage, --coverage=mcdc]` report.
    pub counted: Vec<(String, [Expect; 2])>,
    pub passed: usize,
    /// Each failing test's block, as the runner prints it.
    pub failures: Vec<String>,
}

/// What one file's report says.
#[derive(Default)]
pub struct Expect {
    /// Every line a site starts on, and how often it was reached.
    lines: BTreeMap<usize, u64>,
    /// In report order.
    decisions: Vec<Decision>,
    trees: Vec<Tree>,
}

struct Decision {
    line: usize,
    /// Each branch's name and count.
    branches: Vec<(String, u64)>,
}

struct Tree {
    line: usize,
    whole: String,
    suffix: String,
    /// Each condition's text, and whether it was shown.
    conditions: Vec<(String, bool)>,
}

impl Expect {
    fn line(&mut self, line: usize, n: u64) {
        *self.lines.entry(line).or_default() += n;
    }

    fn numbered(&mut self, line: usize, counts: &[u64], suffix: &str) {
        let branches = counts.iter().enumerate().map(|(i, n)| (format!("{i}{suffix}"), *n)).collect();
        self.decisions.push(Decision { line, branches });
    }

    /// Report order: by line, and in the order each line's decisions were
    /// added.
    fn sorted(mut self) -> Expect {
        self.decisions.sort_by_key(|d| d.line);
        self.trees.sort_by_key(|t| t.line);
        self
    }
}

/// A function and its calls, as written, and its model.
struct Shape {
    /// The function's lines, from its own first.
    decl: Vec<String>,
    /// The calls passing tests make, each a statement or a few, and the calls
    /// that abort a test each, with the message.
    calls: Vec<String>,
    aborts: Vec<(String, &'static str)>,
    /// Fills in a mode's expectation, given the line the declaration starts on
    /// and whether it's MC/DC.
    model: Box<dyn Fn(usize, bool, &mut Expect)>,
}

/// One test source under construction.
struct Suite {
    package: String,
    text: String,
    line: usize,
    passed: usize,
    failures: Vec<String>,
}

impl Suite {
    fn new(package: &str, imports: &[String]) -> Suite {
        let text: String = imports.iter().map(|i| format!("{i}\n")).collect();
        Suite { package: package.to_string(), line: imports.len(), text, passed: 0, failures: Vec::new() }
    }

    /// Writes a test, and answers the line it starts on.
    fn block(&mut self, title: &str, body: &[String]) -> usize {
        let _ = write!(self.text, "\ntest \"{title}\" {{\n");
        self.line += 2;
        let start = self.line;
        for l in body {
            let _ = writeln!(self.text, "    {l}");
            self.line += l.lines().count();
        }
        self.text.push_str("}\n");
        self.line += 1;
        start
    }

    fn passing(&mut self, title: &str, body: &[String]) {
        if !body.is_empty() {
            self.block(title, body);
            self.passed += 1;
        }
    }

    fn aborting(&mut self, title: &str, call: &str, message: &str) {
        let at = self.block(title, &[call.to_string()]);
        let p = &self.package;
        let source = if p.starts_with('t') { "t" } else { "g" };
        self.failures.push(format!("FAIL //lib/{p}  test/{source}.buri  \"{title}\"\n  {message}\n  --> lib/{p}/test/{source}.buri:{at}:1"));
    }
}

/// The packages for `seed`.
pub fn packages(seed: u64, count: usize) -> Vec<Package> {
    let mut rng = Rng::new(seed);
    (0..count).map(|i| package(&mut rng, i)).collect()
}

const KINDS: usize = 12;

const BUILD: &str = "library {\n    sources: [\"g.buri\"]\n\n    test {\n        sources: [\"test/g.buri\"]\n    }\n}\n";

fn package(rng: &mut Rng, index: usize) -> Package {
    let (name, other) = (format!("s{index:02}"), format!("t{index:02}"));
    let mut header: Vec<String> = Vec::new();
    let mut exports: Vec<String> = Vec::new();
    let mut shapes: Vec<(String, Shape)> = Vec::new();
    let mut derived: Option<Derived> = None;
    let n = 5 + rng.below(4);
    let mut uses_bits = false;
    for i in 0..n {
        let f = format!("f{i}");
        let shape = match rng.below(KINDS) {
            0 => chain(rng, &f),
            1 => recursion(rng, &f),
            2 => tail_recursion(rng, &f),
            3 => matching(rng, &f),
            4 => tried(rng, &f),
            5 => generic(rng, &f),
            6 => lambda(rng, &f),
            7 => tail_call(rng, &f),
            8 if derived.is_none() => {
                let d = Derived::new(rng);
                let shape = d.same(rng, &f);
                derived = Some(d);
                shape
            }
            8 | 9 => tail_chain(rng, &f),
            10 => guarded_divide(rng, &f),
            _ => {
                if rng.bool() {
                    divide(rng, &f)
                } else {
                    uses_bits = true;
                    shift(rng, &f)
                }
            }
        };
        exports.push(f.clone());
        shapes.push((f, shape));
    }
    // A second package, whose suite makes some of the calls.
    let shared = rng.chance(3);

    if uses_bits {
        header.push("from \"core/bits\" import * as bits;".into());
        header.push(String::new());
    }
    let mut derived_at = 0;
    if let Some(d) = &derived {
        derived_at = header.len() + 1;
        header.extend(d.decls());
        header.push(String::new());
        exports.push("E".into());
        exports.push("R".into());
    }

    // The source, with each declaration's first line.
    let mut source: Vec<String> = header;
    let mut starts = Vec::new();
    for (i, (_, shape)) in shapes.iter().enumerate() {
        if i > 0 {
            source.push(String::new());
        }
        starts.push(source.len() + 1);
        source.extend(shape.decl.iter().cloned());
    }
    let source = source.join("\n") + "\n";

    let expect = [false, true].map(|mcdc| {
        let mut e = Expect::default();
        for ((_, shape), start) in shapes.iter().zip(&starts) {
            (shape.model)(*start, mcdc, &mut e);
        }
        if let (Some(d), true) = (&derived, mcdc) {
            d.model(derived_at, &mut e);
        }
        e.sorted()
    });

    // The tests: each call in this package's suite, or the other's.
    let mut imports = vec!["from \"core/testing/assert\" import * as assert;".to_string()];
    if derived.as_ref().is_some_and(|d| d.shows()) {
        imports.push("from \"platform/effect\" import { Allocator };".into());
        imports.push("from \"platform/effect/testing\" import { alloc };".into());
    }
    imports.push(format!("from \"//lib/{name}\" import * as g;"));
    if derived.is_some() {
        imports.push(format!("from \"//lib/{name}\" import {{ E, R }};"));
    }
    let mut mine = Suite::new(&name, &imports);
    imports.push(format!("from \"//lib/{other}\" import * as t;"));
    let mut theirs = Suite::new(&other, &imports);
    for (f, shape) in &shapes {
        let (mut here, mut there) = (Vec::new(), Vec::new());
        for call in &shape.calls {
            if shared && rng.bool() { there.push(call.clone()) } else { here.push(call.clone()) }
        }
        mine.passing(f, &here);
        theirs.passing(f, &there);
        for (k, (call, message)) in shape.aborts.iter().enumerate() {
            let suite = if shared && rng.bool() { &mut theirs } else { &mut mine };
            suite.aborting(&format!("{f} aborts {k}"), call, message);
        }
    }
    if let Some(d) = &derived {
        let suite = if shared && rng.bool() { &mut theirs } else { &mut mine };
        suite.passing("derived", &d.calls());
    }

    exports.sort_by_key(|e| e.to_lowercase());
    let lib = format!("from \"//lib/{name}/g.buri\" export {{ {} }};\n", exports.join(", "));
    assert!(lib.len() <= 100, "{name}: an export list the formatter would wrap: {lib}");
    let mut files = vec![
        (format!("lib/{name}/lib.buri"), lib),
        (format!("lib/{name}/g.buri"), source),
        (format!("lib/{name}/test/g.buri"), mine.text),
    ];
    let mut counted = vec![(format!("lib/{name}/g.buri"), expect)];
    let (mut passed, mut failures) = (mine.passed, mine.failures);
    if !shared {
        files.push((format!("lib/{name}/BUILD.buri"), BUILD.to_string()));
        return Package { files, counted, passed, failures };
    }
    files.push((
        format!("lib/{name}/BUILD.buri"),
        BUILD.replace("[\"g.buri\"]\n", &format!("[\"g.buri\"]\n    visibility: [\"//lib/{other}\"]\n")),
    ));

    // The other package's own function, which its suite calls too.
    let picks: Vec<bool> = (0..calls(rng)).map(|_| rng.bool()).collect();
    let body: Vec<String> = picks.iter().map(|&a| assert(format!("t.pick({a})"), u8::from(a))).collect();
    theirs.passing("pick", &body);
    let (n, yes) = (picks.len() as u64, picks.iter().filter(|a| **a).count() as u64);
    let expect = [false, true].map(|_| {
        let mut e = Expect::default();
        e.line(1, n);
        e.line(2, n);
        e.numbered(2, &[yes, n - yes], "");
        e
    });
    counted.push((format!("lib/{other}/t.buri"), expect));
    let build = format!(
        "library {{\n    sources: [\"t.buri\"]\n    dependencies: [\"//lib/{name}\"]\n\n    test {{\n        sources: [\"test/t.buri\"]\n    }}\n}}\n"
    );
    files.extend([
        (format!("lib/{other}/BUILD.buri"), build),
        (format!("lib/{other}/lib.buri"), format!("from \"//lib/{other}/t.buri\" export {{ pick }};\n")),
        (format!("lib/{other}/t.buri"), "export fn pick(a: Bool): Int {\n    if (a) { 1 } else { 0 }\n}\n".to_string()),
        (format!("lib/{other}/test/t.buri"), theirs.text),
    ]);
    passed += theirs.passed;
    failures.extend(theirs.failures);
    Package { files, counted, passed, failures }
}

/// `assert.equal(<call>, <answer>);`
fn assert(call: String, answer: impl std::fmt::Display) -> String {
    format!("assert.equal({call}, {answer});")
}

/// How many calls a shape gets: sometimes none, so its lines read 0 and its
/// decisions `-`.
fn calls(rng: &mut Rng) -> usize {
    if rng.chance(8) { 0 } else { 1 + rng.below(6) }
}

// ---- `if` over a chain ----

enum Node {
    /// A parameter's index, and whether it's negated.
    Leaf(usize, bool),
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
    Not(Box<Node>),
}

const NAMES: [&str; 4] = ["a", "b", "c", "d"];

fn tree(rng: &mut Rng, lo: usize, hi: usize, top: bool) -> Node {
    if hi - lo == 1 {
        return Node::Leaf(lo, rng.chance(3));
    }
    let split = lo + 1 + rng.below(hi - lo - 1);
    let (l, r) = (Box::new(tree(rng, lo, split, false)), Box::new(tree(rng, split, hi, false)));
    let node = if rng.bool() { Node::And(l, r) } else { Node::Or(l, r) };
    if !top && rng.chance(5) || top && rng.chance(8) { Node::Not(Box::new(node)) } else { node }
}

impl Node {
    /// The source, which keeps `a && (b && c)` the tree it is. Also each
    /// `&&`/`||`'s right side's offset, in node order (pre-order).
    fn source(&self, out: &mut String, sides: &mut Vec<(usize, usize)>, next: &mut usize) {
        match self {
            Node::Leaf(i, negated) => {
                if *negated {
                    out.push('!');
                }
                out.push_str(NAMES[*i]);
            }
            Node::And(l, r) | Node::Or(l, r) => {
                let id = *next;
                *next += 1;
                let and = matches!(self, Node::And(..));
                let wrap = |n: &Node, right: bool| match n {
                    Node::Or(..) => and || right,
                    Node::And(..) => and && right,
                    _ => false,
                };
                let side = |out: &mut String, n: &Node, right: bool, sides: &mut Vec<(usize, usize)>, next: &mut usize| {
                    if right {
                        sides.push((id, out.len()));
                    }
                    if wrap(n, right) {
                        out.push('(');
                        n.source(out, sides, next);
                        out.push(')');
                    } else {
                        n.source(out, sides, next);
                    }
                };
                side(out, l, false, sides, next);
                out.push_str(if and { " && " } else { " || " });
                side(out, r, true, sides, next);
            }
            Node::Not(x) => {
                out.push_str("!(");
                x.source(out, sides, next);
                out.push(')');
            }
        }
    }

    /// The text the report names the decision by: the fewest parentheses.
    fn text(&self) -> (String, u8) {
        match self {
            Node::Leaf(i, negated) => (format!("{}{}", if *negated { "!" } else { "" }, NAMES[*i]), 2),
            Node::And(l, r) => {
                let side = |n: &Node| {
                    let (t, b) = n.text();
                    if b < 1 { format!("({t})") } else { t }
                };
                (format!("{} && {}", side(l), side(r)), 1)
            }
            Node::Or(l, r) => (format!("{} || {}", l.text().0, r.text().0), 0),
            Node::Not(x) => (format!("!({})", x.text().0), 2),
        }
    }

    fn leaves(&self, out: &mut Vec<String>) {
        match self {
            Node::Leaf(..) => out.push(self.text().0),
            Node::And(l, r) | Node::Or(l, r) => {
                l.leaves(out);
                r.leaves(out);
            }
            Node::Not(x) => x.leaves(out),
        }
    }

    /// Evaluates, short-circuiting: each leaf's value where it ran, and each
    /// `&&`/`||`'s right side, in node order, `[ran, didn't]`.
    fn eval(&self, args: &[bool], values: &mut Vec<Option<bool>>, sides: &mut [[u64; 2]], next: &mut usize, leaf: &mut usize) -> bool {
        match self {
            Node::Leaf(i, negated) => {
                let v = args[*i] != *negated;
                values[*leaf] = Some(v);
                *leaf += 1;
                v
            }
            Node::And(l, r) | Node::Or(l, r) => {
                let id = *next;
                *next += 1;
                let and = matches!(self, Node::And(..));
                let left = l.eval(args, values, sides, next, leaf);
                let goes_on = left == and;
                if goes_on {
                    sides[id][0] += 1;
                    r.eval(args, values, sides, next, leaf)
                } else {
                    sides[id][1] += 1;
                    // Skip the right side's nodes and leaves.
                    *next += r.nodes();
                    *leaf += r.count();
                    left
                }
            }
            Node::Not(x) => !x.eval(args, values, sides, next, leaf),
        }
    }

    fn nodes(&self) -> usize {
        match self {
            Node::Leaf(..) => 0,
            Node::And(l, r) | Node::Or(l, r) => 1 + l.nodes() + r.nodes(),
            Node::Not(x) => x.nodes(),
        }
    }

    fn count(&self) -> usize {
        match self {
            Node::Leaf(..) => 1,
            Node::And(l, r) | Node::Or(l, r) => l.count() + r.count(),
            Node::Not(x) => x.count(),
        }
    }
}

/// Whether two paths that ran show condition `c` deciding the outcome alone:
/// unique-cause MC/DC with masking, as `design/COVERAGE.md` defines it.
fn shown(paths: &[(Vec<Option<bool>>, bool)], c: usize) -> bool {
    paths.iter().any(|(a, ao)| {
        paths.iter().any(|(b, bo)| {
            matches!((a[c], b[c]), (Some(x), Some(y)) if x != y)
                && ao != bo
                && (0..a.len()).all(|k| k == c || a[k].is_none() || b[k].is_none() || a[k] == b[k])
        })
    })
}

/// A decision's MC/DC record and its sides, from the argument lists it ran on.
struct Ran {
    outcomes: Vec<bool>,
    sides: Vec<[u64; 2]>,
    paths: Vec<(Vec<Option<bool>>, bool)>,
}

fn run(node: &Node, calls: &[Vec<bool>]) -> Ran {
    let mut sides = vec![[0u64; 2]; node.nodes()];
    let mut paths = Vec::new();
    let mut outcomes = Vec::new();
    for args in calls {
        let mut values = vec![None; node.count()];
        let out = node.eval(args, &mut values, &mut sides, &mut 0, &mut 0);
        outcomes.push(out);
        if !paths.contains(&(values.clone(), out)) {
            paths.push((values, out));
        }
    }
    Ran { outcomes, sides, paths }
}

/// The `&&`/`||` decisions and the MC/DC record of `node` on `line`, after
/// whatever the line already has. `sides` are the right sides' offsets.
fn record(e: &mut Expect, line: usize, node: &Node, ran: &Ran, offsets: &[(usize, usize)], mcdc: bool) {
    let mut order: Vec<(usize, usize)> = offsets.to_vec();
    order.sort_by_key(|&(_, at)| at);
    for (id, _) in order {
        e.numbered(line, &ran.sides[id], "");
    }
    if mcdc && node.count() >= 2 {
        let mut leaves = Vec::new();
        node.leaves(&mut leaves);
        let conditions = leaves.into_iter().enumerate().map(|(c, t)| (t, shown(&ran.paths, c))).collect();
        e.trees.push(Tree { line, whole: node.text().0, suffix: String::new(), conditions });
    }
}

fn chain(rng: &mut Rng, f: &str) -> Shape {
    let k = 1 + rng.below(4);
    let node = tree(rng, 0, k, true);
    let mut expr = String::new();
    let mut offsets = Vec::new();
    node.source(&mut expr, &mut offsets, &mut 0);
    let params: Vec<String> = NAMES[..k].iter().map(|n| format!("{n}: Bool")).collect();
    let one_line = rng.bool();
    let mut decl = vec![format!("export fn {f}({}): Int {{", params.join(", "))];
    if one_line {
        decl.push(format!("    if ({expr}) {{ 1 }} else {{ 0 }}"));
    } else {
        decl.extend([
            format!("    if ({expr}) {{"),
            "        let r = 1;".into(),
            "        r".into(),
            "    } else {".into(),
            "        let r = 0;".into(),
            "        r".into(),
            "    }".into(),
        ]);
    }
    decl.push("}".into());
    let inputs: Vec<Vec<bool>> = (0..calls(rng)).map(|_| (0..k).map(|_| rng.bool()).collect()).collect();
    let ran = run(&node, &inputs);
    let calls = inputs
        .iter()
        .zip(&ran.outcomes)
        .map(|(args, out)| {
            let args: Vec<String> = args.iter().map(bool::to_string).collect();
            assert(format!("g.{f}({})", args.join(", ")), u8::from(*out))
        })
        .collect();
    let n = inputs.len() as u64;
    let yes = ran.outcomes.iter().filter(|o| **o).count() as u64;
    Shape {
        decl,
        calls,
        aborts: Vec::new(),
        model: Box::new(move |at, mcdc, e| {
            e.line(at, n);
            e.line(at + 1, n);
            if !one_line {
                for l in [2, 3] {
                    e.line(at + l, yes);
                }
                for l in [4, 5, 6] {
                    e.line(at + l, n - yes);
                }
            }
            e.numbered(at + 1, &[yes, n - yes], "");
            record(e, at + 1, &node, &ran, &offsets, mcdc);
        }),
    }
}

// ---- recursion ----

/// Steps from `n` down to 1, and how many take the `if`.
fn steps(ns: &[i64], m: i64, r: i64) -> (u64, u64) {
    let total = ns.iter().sum::<i64>() as u64;
    let taken = ns.iter().map(|&n| (1..=n).filter(|k| k % m == r).count() as u64).sum();
    (total, taken)
}

fn recursion(rng: &mut Rng, f: &str) -> Shape {
    let (m, ns) = (rng.int(2, 5), (0..calls(rng)).map(|_| rng.int(0, 30)).collect::<Vec<_>>());
    let r = rng.int(0, m - 1);
    let decl = vec![
        format!("export fn {f}(n: Int): Int {{"),
        "    if (n == 0) {".into(),
        "        0".into(),
        "    } else {".into(),
        format!("        let here = if (n % {m} == {r}) {{ 1 }} else {{ 0 }};"),
        format!("        here + {f}(n - 1)"),
        "    }".into(),
        "}".into(),
    ];
    let calls = ns.iter().map(|&n| assert(format!("g.{f}({n})"), steps(&[n], m, r).1)).collect();
    let (total, taken) = steps(&ns, m, r);
    let entered = total + ns.len() as u64;
    let n = ns.len() as u64;
    Shape {
        decl,
        calls,
        aborts: Vec::new(),
        model: Box::new(move |at, mcdc, e| {
            for (l, count) in [(0, entered), (1, entered), (2, n), (3, total), (4, total), (5, total)] {
                e.line(at + l, count);
            }
            e.numbered(at + 1, &[n, total], "");
            e.numbered(at + 4, &[taken, total - taken], "");
            if mcdc {
                e.numbered(at + 4, &[total, 0], "");
            }
        }),
    }
}

fn tail_recursion(rng: &mut Rng, f: &str) -> Shape {
    let (m, ns) = (rng.int(2, 5), (0..calls(rng)).map(|_| rng.int(0, 5000)).collect::<Vec<_>>());
    let r = rng.int(0, m - 1);
    let decl = vec![
        format!("export fn {f}(n: Int, acc: Int): Int {{"),
        "    if (n == 0) {".into(),
        "        acc".into(),
        "    } else {".into(),
        format!("        let step = if (n % {m} == {r}) {{ 1 }} else {{ 0 }};"),
        format!("        {f}(n - 1, acc + step)"),
        "    }".into(),
        "}".into(),
    ];
    let calls = ns.iter().map(|&n| assert(format!("g.{f}({n}, 0)"), steps(&[n], m, r).1)).collect();
    let (total, taken) = steps(&ns, m, r);
    let entered = total + ns.len() as u64;
    let n = ns.len() as u64;
    Shape {
        decl,
        calls,
        aborts: Vec::new(),
        model: Box::new(move |at, mcdc, e| {
            for (l, count) in [(0, entered), (1, entered), (2, n), (3, total), (4, total), (5, total)] {
                e.line(at + l, count);
            }
            e.numbered(at + 1, &[n, total], "");
            e.numbered(at + 4, &[taken, total - taken], "");
            if mcdc {
                e.numbered(at + 4, &[total, 0], "");
            }
        }),
    }
}

// ---- match ----

fn matching(rng: &mut Rng, f: &str) -> Shape {
    let literals = 1 + rng.below(3) as i64;
    let guarded = if rng.bool() { Some(rng.int(0, literals - 1)) } else { None };
    let range = if rng.bool() { Some(rng.int(3, 8)) } else { None };
    let mut decl = vec![format!("export fn {f}(n: Int, flag: Bool): Int {{"), "    match (n) {".into()];
    for i in 0..literals {
        let guard = if guarded == Some(i) { " if flag" } else { "" };
        decl.push(format!("        {i}{guard} => {},", 10 + i));
    }
    if let Some(t) = range {
        decl.push(format!("        k if k > {t} && !flag => 100,"));
    }
    decl.extend(["        _ => 99,".to_string(), "    }".into(), "}".into()]);

    let arms = literals as usize + usize::from(range.is_some()) + 1;
    let mut taken = vec![0u64; arms];
    // The literal guard's [true, false]; the range guard's, its `&&`'s right
    // side [ran, didn't], and its paths.
    let mut guard = [0u64; 2];
    let (mut range_guard, mut side) = ([0u64; 2], [0u64; 2]);
    let mut paths: Vec<(Vec<Option<bool>>, bool)> = Vec::new();
    let mut calls = Vec::new();
    for _ in 0..self::calls(rng) {
        let (n, flag) = (rng.int(-1, 10), rng.bool());
        let mut arm = None;
        if (0..literals).contains(&n) {
            if guarded == Some(n) {
                guard[usize::from(!flag)] += 1;
                if flag {
                    arm = Some(n as usize);
                }
            } else {
                arm = Some(n as usize);
            }
        }
        if let (None, Some(t)) = (arm, range) {
            let big = n > t;
            let path = if big {
                side[0] += 1;
                (vec![Some(true), Some(!flag)], !flag)
            } else {
                side[1] += 1;
                (vec![Some(false), None], false)
            };
            range_guard[usize::from(!path.1)] += 1;
            if path.1 {
                arm = Some(literals as usize);
            }
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        let arm = arm.unwrap_or(arms - 1);
        taken[arm] += 1;
        let answer = if arm < literals as usize { 10 + arm } else if arm == arms - 1 { 99 } else { 100 };
        calls.push(assert(format!("g.{f}({n}, {flag})"), answer));
    }
    let n = calls.len() as u64;
    Shape {
        decl,
        calls,
        aborts: Vec::new(),
        model: Box::new(move |at, mcdc, e| {
            e.line(at, n);
            e.line(at + 1, n);
            e.numbered(at + 1, &taken, "");
            for (i, count) in taken.iter().enumerate() {
                let line = at + 2 + i;
                e.line(line, *count);
                if guarded == Some(i as i64) {
                    e.numbered(line, &guard, "");
                }
                if range.is_some() && i == literals as usize {
                    e.numbered(line, &range_guard, "");
                    e.numbered(line, &side, "");
                    if mcdc {
                        let t = range.unwrap_or_default();
                        let conditions =
                            vec![(format!("k > {t}"), shown(&paths, 0)), ("!flag".to_string(), shown(&paths, 1))];
                        e.trees.push(Tree { line, whole: format!("k > {t} && !flag"), suffix: String::new(), conditions });
                    }
                }
            }
        }),
    }
}

// ---- `?` ----

fn tried(rng: &mut Rng, f: &str) -> Shape {
    let check = format!("{f}check");
    let decl = vec![
        format!("fn {check}(n: Int): Result<Int, Str> {{"),
        "    if (n < 0) { .Err(\"negative\") } else { .Ok(n) }".into(),
        "}".into(),
        String::new(),
        format!("export fn {f}(a: Int, b: Int): Result<Int, Str> {{"),
        format!("    let x = {check}(a)?;"),
        format!("    let y = {check}(b)?;"),
        "    .Ok(x + y)".into(),
        "}".into(),
    ];
    let inputs: Vec<(i64, i64)> = (0..calls(rng)).map(|_| (rng.int(-3, 9), rng.int(-3, 9))).collect();
    let calls = inputs
        .iter()
        .map(|&(a, b)| {
            let answer = if a < 0 || b < 0 { ".Err(\"negative\")".to_string() } else { format!(".Ok({})", a + b) };
            assert(format!("g.{f}({a}, {b})"), answer)
        })
        .collect();
    let n = inputs.len() as u64;
    let first = inputs.iter().filter(|(a, _)| *a >= 0).count() as u64;
    let both = inputs.iter().filter(|(a, b)| *a >= 0 && *b >= 0).count() as u64;
    let checked = n + first;
    let negative = inputs.iter().map(|&(a, b)| u64::from(a < 0) + u64::from(a >= 0 && b < 0)).sum::<u64>();
    Shape {
        decl,
        calls,
        aborts: Vec::new(),
        model: Box::new(move |at, _, e| {
            e.line(at, checked);
            e.line(at + 1, checked);
            e.numbered(at + 1, &[negative, checked - negative], "");
            for (l, count) in [(4, n), (5, n), (6, first), (7, both)] {
                e.line(at + l, count);
            }
            e.numbered(at + 5, &[first, n - first], "");
            e.numbered(at + 6, &[both, first - both], "");
        }),
    }
}

// ---- generic ----

fn generic(rng: &mut Rng, f: &str) -> Shape {
    let decl = vec![
        format!("export fn {f}<T>(flag: Bool, x: T, y: T): T {{"),
        "    if (flag) {".into(),
        "        let r = x;".into(),
        "        r".into(),
        "    } else {".into(),
        "        let r = y;".into(),
        "        r".into(),
        "    }".into(),
        "}".into(),
    ];
    // By the instantiation's name, in the report's order: [then, else].
    let mut at_type: BTreeMap<&'static str, [u64; 2]> = BTreeMap::new();
    let mut calls = Vec::new();
    for _ in 0..self::calls(rng) {
        let flag = rng.bool();
        let (label, x, y) = match rng.below(3) {
            0 => ("T = Bool", "true".to_string(), "false".to_string()),
            1 => ("T = I64", rng.int(-9, 9).to_string(), rng.int(-9, 9).to_string()),
            _ => ("T = Str", format!("\"{}\"", rng.int(0, 9)), format!("\"{}\"", rng.int(0, 9))),
        };
        at_type.entry(label).or_default()[usize::from(!flag)] += 1;
        let answer = if flag { &x } else { &y };
        calls.push(assert(format!("g.{f}({flag}, {x}, {y})"), answer));
    }
    let n = calls.len() as u64;
    let yes: u64 = at_type.values().map(|t| t[0]).sum();
    Shape {
        decl,
        calls,
        aborts: Vec::new(),
        model: Box::new(move |at, mcdc, e| {
            for (l, count) in [(0, n), (1, n), (2, yes), (3, yes), (4, n - yes), (5, n - yes), (6, n - yes)] {
                e.line(at + l, count);
            }
            if !mcdc {
                e.numbered(at + 1, &[yes, n - yes], "");
            } else if at_type.is_empty() {
                e.numbered(at + 1, &[0, 0], " (never instantiated)");
            } else {
                for (label, counts) in &at_type {
                    e.numbered(at + 1, counts, &format!(" ({label})"));
                }
            }
        }),
    }
}

// ---- a lambda ----

fn lambda(rng: &mut Rng, f: &str) -> Shape {
    let t = rng.int(0, 5);
    let decl = vec![
        format!("export fn {f}(items: [Int]): Int {{"),
        format!("    items.fold(fn(acc, x) => if (x > {t}) {{ acc + x }} else {{ acc }}, 0)"),
        "}".into(),
    ];
    let lists: Vec<Vec<i64>> = (0..calls(rng)).map(|_| (0..rng.below(5)).map(|_| rng.int(0, 9)).collect()).collect();
    let calls = lists
        .iter()
        .map(|l| {
            let items: Vec<String> = l.iter().map(i64::to_string).collect();
            assert(format!("g.{f}([{}])", items.join(", ")), l.iter().filter(|&&x| x > t).sum::<i64>())
        })
        .collect();
    let n = lists.len() as u64;
    let elements = lists.iter().map(Vec::len).sum::<usize>() as u64;
    let big = lists.iter().flatten().filter(|&&x| x > t).count() as u64;
    Shape {
        decl,
        calls,
        aborts: Vec::new(),
        model: Box::new(move |at, _, e| {
            e.line(at, n);
            // The call's statement, and the lambda's body once an element.
            e.line(at + 1, n + elements);
            e.numbered(at + 1, &[big, elements - big], "");
        }),
    }
}

// ---- a tail call under `&&` ----

fn tail_call(rng: &mut Rng, f: &str) -> Shape {
    let t = rng.int(0, 9);
    let helper = format!("{f}big");
    let decl = vec![
        format!("export fn {f}(a: Bool, n: Int): Bool {{"),
        format!("    a && {helper}(n)"),
        "}".into(),
        String::new(),
        format!("fn {helper}(n: Int): Bool {{"),
        format!("    n > {t}"),
        "}".into(),
    ];
    let inputs: Vec<(bool, i64)> = (0..calls(rng)).map(|_| (rng.bool(), rng.int(0, 12))).collect();
    let calls = inputs.iter().map(|&(a, n)| assert(format!("g.{f}({a}, {n})"), a && n > t)).collect();
    let n = inputs.len() as u64;
    let ran = inputs.iter().filter(|(a, _)| *a).count() as u64;
    Shape {
        decl,
        calls,
        aborts: Vec::new(),
        model: Box::new(move |at, _, e| {
            e.line(at, n);
            e.line(at + 1, n);
            e.numbered(at + 1, &[ran, n - ran], "");
            e.line(at + 4, ran);
            e.line(at + 5, ran);
        }),
    }
}

// ---- a chain in tail position ----

/// `fn f(a: Bool, ...): Bool { <chain> }`: in tail position, with no call to
/// keep there, so an MC/DC decision.
fn tail_chain(rng: &mut Rng, f: &str) -> Shape {
    let k = 2 + rng.below(3);
    let node = tree(rng, 0, k, true);
    let mut expr = String::new();
    let mut offsets = Vec::new();
    node.source(&mut expr, &mut offsets, &mut 0);
    let params: Vec<String> = NAMES[..k].iter().map(|n| format!("{n}: Bool")).collect();
    let decl = vec![format!("export fn {f}({}): Bool {{", params.join(", ")), format!("    {expr}"), "}".into()];
    let inputs: Vec<Vec<bool>> = (0..calls(rng)).map(|_| (0..k).map(|_| rng.bool()).collect()).collect();
    let ran = run(&node, &inputs);
    let calls = inputs
        .iter()
        .zip(&ran.outcomes)
        .map(|(args, out)| {
            let args: Vec<String> = args.iter().map(bool::to_string).collect();
            assert(format!("g.{f}({})", args.join(", ")), out)
        })
        .collect();
    let n = inputs.len() as u64;
    Shape {
        decl,
        calls,
        aborts: Vec::new(),
        model: Box::new(move |at, mcdc, e| {
            e.line(at, n);
            e.line(at + 1, n);
            record(e, at + 1, &node, &ran, &offsets, mcdc);
        }),
    }
}

// ---- a division inside an `&&` ----

/// `a && n / d > t`, where a zero `d` aborts partway through the decision.
fn guarded_divide(rng: &mut Rng, f: &str) -> Shape {
    let t = rng.int(0, 4);
    let decl = vec![format!("export fn {f}(a: Bool, n: Int, d: Int): Bool {{"), format!("    a && n / d > {t}"), "}".into()];
    let inputs: Vec<(bool, i64, i64)> = (0..calls(rng)).map(|_| (rng.bool(), rng.int(0, 20), rng.int(1, 4))).collect();
    let calls = inputs.iter().map(|&(a, n, d)| assert(format!("g.{f}({a}, {n}, {d})"), a && n / d > t)).collect();
    let aborts: Vec<(String, &'static str)> =
        (0..rng.below(3)).map(|_| (assert(format!("g.{f}(true, {}, 0)", rng.int(0, 9)), false), "division by zero")).collect();
    let n = inputs.len() as u64;
    let aborted = aborts.len() as u64;
    // An abort reaches the right side and stops there, so it counts no path.
    let ran = inputs.iter().filter(|(a, ..)| *a).count() as u64;
    let mut paths: Vec<(Vec<Option<bool>>, bool)> = Vec::new();
    for &(a, n, d) in &inputs {
        let path = if a { (vec![Some(true), Some(n / d > t)], n / d > t) } else { (vec![Some(false), None], false) };
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    Shape {
        decl,
        calls,
        aborts,
        model: Box::new(move |at, mcdc, e| {
            e.line(at, n + aborted);
            e.line(at + 1, n + aborted);
            e.numbered(at + 1, &[ran + aborted, n - ran], "");
            if mcdc {
                e.numbered(at + 1, &[ran, aborted], "");
                let conditions = vec![("a".to_string(), shown(&paths, 0)), (format!("n / d > {t}"), shown(&paths, 1))];
                e.trees.push(Tree { line: at + 1, whole: format!("a && n / d > {t}"), suffix: String::new(), conditions });
            }
        }),
    }
}

// ---- aborts ----

/// `a / b` at `Int`, or in a function generic over `Divide` called at `Int`.
fn divide(rng: &mut Rng, f: &str) -> Shape {
    let generic = rng.bool();
    let head = if generic { format!("export fn {f}<T: Divide>(a: T, b: T): T {{") } else { format!("export fn {f}(a: Int, b: Int): Int {{") };
    let decl = vec![head, "    a / b".into(), "}".into()];
    let inputs: Vec<(i64, i64)> = (0..calls(rng)).map(|_| (rng.int(0, 30), rng.int(1, 5))).collect();
    let calls = inputs.iter().map(|&(a, b)| assert(format!("g.{f}({a}, {b})"), a / b)).collect();
    let aborts: Vec<(String, &'static str)> =
        (0..rng.below(3)).map(|_| (assert(format!("g.{f}({}, 0)", rng.int(0, 9)), 0), "division by zero")).collect();
    // A generic one nothing calls is no abort: its `/` is on a `T`.
    let built = !inputs.is_empty() || !aborts.is_empty();
    let instance = match (generic, built) {
        (false, _) => Some(""),
        (true, true) => Some(" (T = I64)"),
        (true, false) => None,
    };
    trap(decl, calls, aborts, inputs.len() as u64, instance)
}

fn shift(rng: &mut Rng, f: &str) -> Shape {
    let decl = vec![format!("export fn {f}(n: Int): Int {{"), "    bits.shiftLeft(1, n)".into(), "}".into()];
    let inputs: Vec<i64> = (0..calls(rng)).map(|_| rng.int(0, 10)).collect();
    let calls = inputs.iter().map(|&n| assert(format!("g.{f}({n})"), 1i64 << n)).collect();
    let aborts: Vec<(String, &'static str)> =
        (0..rng.below(3)).map(|_| (assert(format!("g.{f}({})", rng.int(-3, -1)), 0), "shift out of range")).collect();
    trap(decl, calls, aborts, inputs.len() as u64, Some(""))
}

/// A one-line function that can abort, called `n` times without and once
/// more for each abort. Under MC/DC, an abort decision with `suffix`, if any.
fn trap(decl: Vec<String>, calls: Vec<String>, aborts: Vec<(String, &'static str)>, n: u64, suffix: Option<&'static str>) -> Shape {
    let aborted = aborts.len() as u64;
    Shape {
        decl,
        calls,
        aborts,
        model: Box::new(move |at, mcdc, e| {
            e.line(at, n + aborted);
            e.line(at + 1, n + aborted);
            if let (true, Some(suffix)) = (mcdc, suffix) {
                e.numbered(at + 1, &[n, aborted], suffix);
            }
        }),
    }
}

// ---- derived code ----

#[derive(Clone, Copy, PartialEq, Debug)]
enum Field {
    Int,
    Bool,
    E,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Value {
    Int(i64),
    Bool(bool),
    /// A variant of `E`: `A`, `B(Int)` or `C(Int, Int)`. Derived `Ordered`
    /// orders by variant, then fields, as this does.
    E(usize, Vec<i64>),
}

const VARIANTS: [&str; 3] = ["A", "B", "C"];

impl Value {
    fn source(&self) -> String {
        match self {
            Value::Int(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::E(v, xs) if xs.is_empty() => format!(".{}", VARIANTS[*v]),
            Value::E(v, xs) => {
                let xs: Vec<String> = xs.iter().map(i64::to_string).collect();
                format!(".{}({})", VARIANTS[*v], xs.join(", "))
            }
        }
    }
}

/// A comparison a derived shadow walks: `Equal` or `Ordered`, and what its
/// field decisions are called.
#[derive(Clone, Copy)]
struct Op {
    name: &'static str,
    outcomes: [&'static str; 2],
}

const EQUAL: Op = Op { name: "Equal", outcomes: ["same", "differs"] };
const ORDERED: Op = Op { name: "Ordered", outcomes: ["equal", "decides"] };

/// `E`, and a struct `R` holding it, with what the tests do to them.
struct Derived {
    fields: Vec<Field>,
    /// Whether `R` derives `Ordered` and `Show` too.
    orders_r: bool,
    shows_r: bool,
    /// `==` and `<` on two `E`s and two `R`s, `E.show`, `R.show`.
    eq_e: Vec<(Value, Value)>,
    eq_r: Vec<(Vec<Value>, Vec<Value>)>,
    lt_e: Vec<(Value, Value)>,
    lt_r: Vec<(Vec<Value>, Vec<Value>)>,
    show_e: Vec<Value>,
    show_r: Vec<Vec<Value>>,
    /// The calls of the `&&` ending in an `==`.
    same: std::cell::RefCell<Vec<(bool, Vec<Value>, Vec<Value>)>>,
}

/// The derived decisions the walks reached.
#[derive(Default)]
struct Walks {
    /// By (operation and type, order in its walk): label, branch names, counts.
    decisions: BTreeMap<(String, usize), (String, Vec<String>, Vec<u64>)>,
}

impl Walks {
    fn hit(&mut self, group: &str, order: usize, what: &str, names: &[&str], branch: usize) {
        self.declare(group, order, what, names)[branch] += 1;
    }

    fn declare(&mut self, group: &str, order: usize, what: &str, names: &[&str]) -> &mut Vec<u64> {
        &mut self
            .decisions
            .entry((group.to_string(), order))
            .or_insert_with(|| (format!("{group}, {what}"), names.iter().map(|s| s.to_string()).collect(), vec![0; names.len()]))
            .2
    }
}

const OTHER: [&str; 2] = ["same variant", "another variant"];

impl Derived {
    fn new(rng: &mut Rng) -> Derived {
        let fields = (0..1 + rng.below(3)).map(|_| [Field::Int, Field::Bool, Field::E][rng.below(3)]).collect();
        let mut d = Derived {
            fields,
            orders_r: rng.bool(),
            shows_r: rng.bool(),
            eq_e: Vec::new(),
            eq_r: Vec::new(),
            lt_e: Vec::new(),
            lt_r: Vec::new(),
            show_e: Vec::new(),
            show_r: Vec::new(),
            same: Default::default(),
        };
        for _ in 0..rng.below(4) {
            let pair = d.pair_e(rng);
            d.eq_e.push(pair);
        }
        for _ in 0..rng.below(4) {
            let pair = d.pair_r(rng);
            d.eq_r.push(pair);
        }
        for _ in 0..rng.below(3) {
            let pair = d.pair_e(rng);
            d.lt_e.push(pair);
        }
        if d.orders_r {
            for _ in 0..rng.below(4) {
                let pair = d.pair_r(rng);
                d.lt_r.push(pair);
            }
        }
        for _ in 0..rng.below(3) {
            let v = d.value(rng, Field::E);
            d.show_e.push(v);
        }
        if d.shows_r {
            for _ in 0..rng.below(3) {
                let r = d.record(rng);
                d.show_r.push(r);
            }
        }
        d
    }

    fn value(&self, rng: &mut Rng, f: Field) -> Value {
        match f {
            Field::Int => Value::Int(rng.int(-2, 2)),
            Field::Bool => Value::Bool(rng.bool()),
            Field::E => {
                let v = rng.below(3);
                Value::E(v, (0..v).map(|_| rng.int(0, 2)).collect())
            }
        }
    }

    fn pair_e(&self, rng: &mut Rng) -> (Value, Value) {
        let a = self.value(rng, Field::E);
        let b = if rng.bool() { a.clone() } else { self.value(rng, Field::E) };
        (a, b)
    }

    fn record(&self, rng: &mut Rng) -> Vec<Value> {
        self.fields.iter().map(|f| self.value(rng, *f)).collect()
    }

    /// Two records, the second the first or it with one field redrawn, so
    /// comparisons reach past the first field.
    fn pair_r(&self, rng: &mut Rng) -> (Vec<Value>, Vec<Value>) {
        let a = self.record(rng);
        let mut b = a.clone();
        if rng.bool() {
            let i = rng.below(b.len());
            b[i] = self.value(rng, self.fields[i]);
        }
        (a, b)
    }

    fn shows(&self) -> bool {
        !self.show_e.is_empty() || !self.show_r.is_empty()
    }

    fn decls(&self) -> Vec<String> {
        let mut traits = vec!["Equal"];
        if self.orders_r {
            traits.push("Ordered");
        }
        if self.shows_r {
            traits.push("Show");
        }
        let mut out = vec![
            "derive Equal, Ordered, Show for E;".to_string(),
            "export enum E {".into(),
            "    A,".into(),
            "    B(Int),".into(),
            "    C(Int, Int),".into(),
            "}".into(),
            String::new(),
            format!("derive {} for R;", traits.join(", ")),
            "export struct R {".into(),
        ];
        for (i, f) in self.fields.iter().enumerate() {
            let ty = match f {
                Field::Int => "Int",
                Field::Bool => "Bool",
                Field::E => "E",
            };
            out.push(format!("    export f{i}: {ty},"));
        }
        out.push("}".into());
        out
    }

    fn r_source(&self, r: &[Value]) -> String {
        let fields: Vec<String> = r.iter().enumerate().map(|(i, v)| format!("f{i}: {}", v.source())).collect();
        format!("R {{ {} }}", fields.join(", "))
    }

    /// `export fn f(a: Bool, p: R, q: R): Bool { a && p == q }`.
    fn same(&self, rng: &mut Rng, f: &str) -> Shape {
        let decl = vec![format!("export fn {f}(a: Bool, p: R, q: R): Bool {{"), "    a && p == q".into(), "}".into()];
        let mut inputs = Vec::new();
        for _ in 0..calls(rng) {
            let (p, q) = self.pair_r(rng);
            inputs.push((rng.bool(), p, q));
        }
        // Bound first, so no line is long enough for the formatter to break.
        let calls = inputs
            .iter()
            .enumerate()
            .map(|(i, (a, p, q))| {
                format!(
                    "let p{i} = {};\n    let q{i} = {};\n    {}",
                    self.r_source(p),
                    self.r_source(q),
                    assert(format!("g.{f}({a}, p{i}, q{i})"), *a && p == q)
                )
            })
            .collect();
        *self.same.borrow_mut() = inputs.clone();
        let n = inputs.len() as u64;
        let ran = inputs.iter().filter(|(a, ..)| *a).count() as u64;
        let mut paths: Vec<(Vec<Option<bool>>, bool)> = Vec::new();
        for (a, p, q) in &inputs {
            let path = if *a { (vec![Some(true), Some(p == q)], p == q) } else { (vec![Some(false), None], false) };
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        Shape {
            decl,
            calls,
            aborts: Vec::new(),
            model: Box::new(move |at, mcdc, e| {
                e.line(at, n);
                e.line(at + 1, n);
                e.numbered(at + 1, &[ran, n - ran], "");
                // A function nothing built is judged on its checked body,
                // where `==` is a trait call in tail position.
                if mcdc && n > 0 {
                    let conditions = vec![("a".to_string(), shown(&paths, 0)), ("p == q".to_string(), shown(&paths, 1))];
                    e.trees.push(Tree { line: at + 1, whole: "a && p == q".into(), suffix: String::new(), conditions });
                }
            }),
        }
    }

    /// The `derived` test's statements. Records are bound first, so no line
    /// is long enough for the formatter to break.
    fn calls(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (a, b) in &self.eq_e {
            out.push(assert(format!("E{} == E{}", a.source(), b.source()), a == b));
        }
        for (i, (a, b)) in self.eq_r.iter().enumerate() {
            out.push(format!("let a{i} = {};", self.r_source(a)));
            out.push(format!("let b{i} = {};", self.r_source(b)));
            out.push(assert(format!("a{i} == b{i}"), a == b));
        }
        for (a, b) in &self.lt_e {
            out.push(assert(format!("E{} < E{}", a.source(), b.source()), a < b));
        }
        for (i, (a, b)) in self.lt_r.iter().enumerate() {
            out.push(format!("let c{i} = {};", self.r_source(a)));
            out.push(format!("let d{i} = {};", self.r_source(b)));
            out.push(assert(format!("c{i} < d{i}"), a < b));
        }
        if self.shows() {
            out.push("let ctx = context {\n        Allocator: alloc(),\n    };".into());
        }
        for v in &self.show_e {
            out.push(assert(format!("E{}.show(ctx)", v.source()), format!("\"{}\"", v.source())));
        }
        for (i, r) in self.show_r.iter().enumerate() {
            out.push(format!("let r{i} = {};", self.r_source(r)));
            out.push(assert(format!("r{i}.show(ctx)"), format!("\"{}\"", self.r_source(r))));
        }
        out
    }

    /// Every decision of `E`'s shadow for `op`, so one nothing reached still
    /// reads `-`. In the shadow's order: which variant, then each variant's
    /// other value, then its fields'.
    fn declare_e(w: &mut Walks, op: Op) {
        let group = format!("{} E", op.name);
        w.declare(&group, 0, "variant", &VARIANTS);
        for (v, name) in VARIANTS.iter().enumerate() {
            w.declare(&group, v + 1, &format!("{name}, the other"), &OTHER);
        }
        w.declare(&group, 4, "C.0", &op.outcomes);
    }

    /// `op` on two `E`s, through the shadow: answers whether they're equal.
    fn walk_e(w: &mut Walks, op: Op, a: &Value, b: &Value) -> bool {
        let (Value::E(va, xa), Value::E(vb, xb)) = (a, b) else { unreachable!() };
        let group = format!("{} E", op.name);
        Self::declare_e(w, op);
        w.hit(&group, 0, "variant", &VARIANTS, *va);
        w.hit(&group, va + 1, &format!("{}, the other", VARIANTS[*va]), &OTHER, usize::from(va != vb));
        if va != vb {
            return false;
        }
        // `C`'s two fields: one decision, after the first.
        if *va == 2 {
            let same = xa[0] == xb[0];
            w.hit(&group, 4, "C.0", &op.outcomes, usize::from(!same));
            return same && xa[1] == xb[1];
        }
        xa == xb
    }

    /// Every decision of `R`'s shadow for `op`: one after each field but the
    /// last.
    fn declare_r(&self, w: &mut Walks, op: Op) {
        for i in 1..self.fields.len() {
            w.declare(&format!("{} R", op.name), i - 1, &format!("f{}", i - 1), &op.outcomes);
        }
    }

    fn walk_r(&self, w: &mut Walks, op: Op, a: &[Value], b: &[Value]) -> bool {
        let last = self.fields.len() - 1;
        for i in 0..=last {
            let same = match self.fields[i] {
                Field::E => Self::walk_e(w, op, &a[i], &b[i]),
                _ => a[i] == b[i],
            };
            if i == last {
                return same;
            }
            w.hit(&format!("{} R", op.name), i, &format!("f{i}"), &op.outcomes, usize::from(!same));
            if !same {
                return false;
            }
        }
        true
    }

    fn walk_show_e(w: &mut Walks, v: &Value) {
        let Value::E(variant, _) = v else { unreachable!() };
        w.hit("Show E", 0, "variant", &VARIANTS, *variant);
    }

    fn model(&self, at: usize, e: &mut Expect) {
        let mut w = Walks::default();
        let has_e = self.fields.contains(&Field::E);
        let same_calls = self.same.borrow().clone();
        // A shadow exists for each operation the program calls, and for each
        // its walk reaches.
        for (op, on_e, on_r) in [(EQUAL, !self.eq_e.is_empty(), !self.eq_r.is_empty() || !same_calls.is_empty()), (ORDERED, !self.lt_e.is_empty(), !self.lt_r.is_empty())] {
            if on_e || on_r && has_e {
                Self::declare_e(&mut w, op);
            }
            if on_r {
                self.declare_r(&mut w, op);
            }
        }
        for (a, b) in &self.eq_e {
            Self::walk_e(&mut w, EQUAL, a, b);
        }
        for (a, b) in &self.eq_r {
            self.walk_r(&mut w, EQUAL, a, b);
        }
        for (a, p, q) in &same_calls {
            if *a {
                self.walk_r(&mut w, EQUAL, p, q);
            }
        }
        for (a, b) in &self.lt_e {
            Self::walk_e(&mut w, ORDERED, a, b);
        }
        for (a, b) in &self.lt_r {
            self.walk_r(&mut w, ORDERED, a, b);
        }
        if !self.show_e.is_empty() || !self.show_r.is_empty() && has_e {
            w.declare("Show E", 0, "variant", &VARIANTS);
        }
        for v in &self.show_e {
            Self::walk_show_e(&mut w, v);
        }
        for r in &self.show_r {
            for (f, v) in self.fields.iter().zip(r) {
                if *f == Field::E {
                    Self::walk_show_e(&mut w, v);
                }
            }
        }
        // `E` is declared on `at`, `R` seven lines below.
        for ((group, _), (label, names, counts)) in &w.decisions {
            let line = if group.ends_with(" E") { at } else { at + 7 };
            let branches = names.iter().zip(counts).map(|(name, n)| (format!("{label}: {name}"), *n)).collect();
            e.decisions.push(Decision { line, branches });
        }
    }
}

/// The lcov the report writes for `file`, from `e`, and its summary row's
/// counts: branches, conditions, decisions, each `(covered, total)`.
pub fn lcov(file: &str, e: &Expect, mcdc: bool) -> (String, [(usize, usize); 3]) {
    let mut out = format!("TN:\nSF:{file}\n");
    let mut lines = e.lines.clone();
    let mut block = 0;
    let mut last = 0;
    let mut hit = 0;
    let mut total = 0;
    // A line no site starts on reads how often its decisions were reached.
    let mut off_site: BTreeMap<usize, u64> = BTreeMap::new();
    for d in &e.decisions {
        block = if d.line == last { block + 1 } else { 0 };
        last = d.line;
        let reached = d.branches.iter().any(|(_, n)| *n > 0);
        for (name, n) in &d.branches {
            let taken = if reached { n.to_string() } else { "-".into() };
            let _ = writeln!(out, "BRDA:{},{block},{name},{taken}", d.line);
            total += 1;
            hit += usize::from(*n > 0);
        }
        if !e.lines.contains_key(&d.line) {
            let sum = d.branches.iter().map(|b| b.1).sum::<u64>();
            let row = off_site.entry(d.line).or_default();
            *row = (*row).max(sum);
        }
    }
    lines.extend(off_site);
    let _ = write!(out, "BRF:{total}\nBRH:{hit}\n");
    let mut conditions = (0, 0);
    let mut decisions = (0, 0);
    if mcdc {
        let mut by_line: BTreeMap<usize, Vec<(String, bool)>> = BTreeMap::new();
        for t in &e.trees {
            decisions.1 += 1;
            decisions.0 += usize::from(t.conditions.iter().all(|c| c.1));
            for (c, s) in &t.conditions {
                by_line.entry(t.line).or_default().push((format!("'{c}' in '{}'{}", t.whole, t.suffix), *s));
            }
        }
        for (line, group) in &by_line {
            for (index, (expression, s)) in group.iter().enumerate() {
                for sense in ["t", "f"] {
                    let _ = writeln!(out, "MCDC:{line},{},{sense},{},{index},{expression}", group.len(), u8::from(*s));
                }
                conditions.1 += 1;
                conditions.0 += usize::from(*s);
            }
        }
        let _ = write!(out, "MCF:{}\nMCH:{}\n", conditions.1 * 2, conditions.0 * 2);
    }
    for (line, n) in &lines {
        let _ = writeln!(out, "DA:{line},{n}");
    }
    let _ = write!(out, "LF:{}\nLH:{}\nend_of_record\n", lines.len(), lines.values().filter(|n| **n > 0).count());
    (out, [(hit, total), conditions, decisions])
}

/// The summary the report prints after the verdicts, from each file's row.
pub fn summary(rows: &[(String, [(usize, usize); 3])], mcdc: bool) -> String {
    let mut rows = rows.to_vec();
    let mut total = [(0, 0); 3];
    for (_, r) in &rows {
        for (t, c) in total.iter_mut().zip(r) {
            t.0 += c.0;
            t.1 += c.1;
        }
    }
    rows.push(("total".into(), total));
    let columns = if mcdc { 3 } else { 1 };
    let cell = |(c, t): (usize, usize)| {
        let percent = if t == 0 { "-".to_string() } else { format!("{:.1}%", c as f64 * 100.0 / t as f64) };
        format!("{c}/{t}  {percent:>6}")
    };
    let cells: Vec<Vec<String>> = rows.iter().map(|(_, r)| r[..columns].iter().map(|c| cell(*c)).collect()).collect();
    let title = if mcdc { "mc/dc coverage" } else { "branch coverage" };
    let mut width = rows.iter().map(|r| r.0.len()).max().unwrap_or(0);
    if mcdc {
        width = width.max(title.len() - 2);
    }
    let widths: Vec<usize> = (0..columns).map(|i| cells.iter().map(|c| c[i].len()).max().unwrap_or(0)).collect();
    let mut out = String::from("\n");
    if mcdc {
        let mut head = format!("{title:<w$}", w = width + 2);
        for (column, w) in ["branches", "conditions", "decisions"].iter().zip(&widths) {
            let _ = write!(head, "  {column:>w$}");
        }
        out.push_str(&head);
    } else {
        out.push_str(title);
    }
    out.push('\n');
    for ((name, _), cells) in rows.iter().zip(&cells) {
        let mut line = format!("  {name:<width$}");
        for (c, w) in cells.iter().zip(&widths) {
            let _ = write!(line, "  {c:>w$}");
        }
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str("lcov: .buri/coverage/lcov.info\n");
    out
}
