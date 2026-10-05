//! `middle::derives` on whole snippets, through the real front end and the
//! JavaScript backend. Here rather than beside the pass because a snippet
//! compiles through `driver`, which is in `buri`.

use crate::compiler::middle::derives::*;
use crate::compiler::middle::monomorphize::{self, Program};
use crate::compiler::semantics::typed::{self, Expr, ExprKind, PatKind, Pattern, Stmt, TemplatePart};
use crate::compiler::semantics::types::FuncIdx;
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
        .diagnostics
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
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;

struct P { x: Int, y: Str }
derive Equal, Ordered, Show, Hash for P;

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let a = P { x: 1, y: "a" };
  let b = P { x: 2, y: "b" };
  let _ = io.println(ctx, "${a == b}").ignore();
  let _ = io.println(ctx, a.show(ctx)).ignore();
  let _ = io.println(ctx, "${a.hash()}").ignore();
  let o = a.compare(b);
  let _ = io.println(ctx, "${o == .Less}").ignore();
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
        ExprKind::Int(v, _) => format!("{}", v.get()),
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
///
/// The concatenation is one call to the shared arity-5 joiner rather than a
/// `cat[…]` written out here, which is [`Generator::joined`]: the chain is
/// emitted once for the program instead of once per rendered shape.
#[test]
fn rendering_is_a_concatenation() {
    let (mut program, _) = compile(POINT);
    let out = run(&mut program);
    let joiner = joiner_of(&program, 5).expect("an arity-5 joiner");
    assert_eq!(
        printed(&program, &out, Op::Show),
        vec![format!(
            "$derive$show$P(l0) = f{joiner}(\"P {{ x: \", derivePrimShow(l0.0), \", y: \", \
             derivePrimShow(l0.1), \" }}\")"
        )]
    );
}

/// The index of the joiner of this arity, and the proof that there is
/// exactly one of it.
fn joiner_of(program: &Program, arity: usize) -> Option<usize> {
    let name = format!("derive$join${arity}");
    let found: Vec<usize> = program
        .funcs
        .iter()
        .enumerate()
        .filter(|(_, f)| f.symbol == name)
        .map(|(i, _)| i)
        .collect();
    assert!(found.len() <= 1, "{name} was minted {} times", found.len());
    found.first().copied()
}

/// Two shapes that render with the same number of pieces share one joiner,
/// which is the whole point of moving the chain off the site.
#[test]
fn one_joiner_serves_every_shape_of_the_same_width() {
    let src = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;

struct A { x: Int, y: Int }
struct B { p: Int, q: Int }
derive Show for A;
derive Show for B;

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, A { x: 1, y: 2 }.show(ctx)).ignore();
  let _ = io.println(ctx, B { p: 3, q: 4 }.show(ctx)).ignore();
  .Ok(())
}
"#;
    let (mut program, _) = compile(src);
    let _ = run(&mut program);
    // `joiner_of` asserts there is at most one, and both shapes render five
    // pieces, so finding it at all is the sharing.
    assert!(joiner_of(&program, 5).is_some(), "the arity-5 joiner is shared");
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
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;

enum Shape { Dot, Line(Int, Int) }
derive Equal, Show for Shape;

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let a = Shape.Line(1, 2);
  let _ = io.println(ctx, "${a == .Dot}").ignore();
  let _ = io.println(ctx, a.show(ctx)).ignore();
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
    let joiner = joiner_of(&program, 5).expect("an arity-5 joiner");
    assert_eq!(
        printed(&program, &out, Op::Show),
        vec![format!(
            "$derive$show$Shape(l0) = match l0 {{ .v0 => \".Dot\", .v1(l1, l2) => \
             f{joiner}(\".Line(\", derivePrimShow(l1), \", \", derivePrimShow(l2), \")\") }}"
        )]
    );
}

/// A list is one call to the runtime helper, handed a code pointer to the
/// element's own generated function.
#[test]
fn a_list_is_the_element_function_and_a_helper() {
    let src = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;

struct P { x: Int }
derive Equal, Show for P;

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let xs = [P { x: 1 }];
  let _ = io.println(ctx, "${xs == [P { x: 2 }]}").ignore();
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
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;

struct Meters { v: Int }
struct Seconds { v: Int }
derive Equal, Show for Meters;
derive Equal, Show for Seconds;

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let a = Meters { v: 1 };
  let b = Seconds { v: 1 };
  let _ = io.println(ctx, "${a == Meters { v: 2 }}").ignore();
  let _ = io.println(ctx, "${b == Seconds { v: 2 }}").ignore();
  let _ = io.println(ctx, a.show(ctx)).ignore();
  let _ = io.println(ctx, b.show(ctx)).ignore();
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
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;

enum Rose { Leaf(Int), Node([Rose]) }
derive Equal for Rose;

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let a = Rose.Node([Rose.Leaf(1)]);
  let _ = io.println(ctx, "${a == Rose.Leaf(2)}").ignore();
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
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/json" import { DecodeError, ToJson, FromJson };
from "core/json" import * as json;

struct P { x: Int }
derive Equal, ToJson, FromJson for P;

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let p = P { x: 1 };
  let back: Result<P, DecodeError> = json.decode(ctx, json.encode(ctx, p));
  let _ = io.println(ctx, "${back == .Ok(p)}").ignore();
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
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;

struct P { x: Int }
derive Equal, Show for P;

export fn seek(n: Int, needle: P): Int {
  if (n <= 0) {
0
  } else if (P { x: n } == needle) {
n
  } else {
seek(n - 1, needle)
  }
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "${seek(3, P { x: 2 })}").ignore();
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
