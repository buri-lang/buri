//! `middle::lower` on whole snippets, through the real front end. Here rather
//! than beside the pass because a snippet compiles through `driver`, which is
//! in `buri`.

use crate::compiler::driver;
use crate::compiler::middle;
use crate::compiler::middle::ir::{self, Body, Inst, Ownership, ValueId};
use crate::compiler::middle::lower::*;
use crate::compiler::middle::monomorphize::{self, Program};
use crate::compiler::middle::rc;
use crate::compiler::modules::Role;
use crate::diagnostics::{Diagnostics, SourceMap};

/// Compiles a snippet through the real front end and the real middle end,
/// and lowers it.
///
/// Through the ordinary driver on purpose: a lowering tested against a
/// hand-built tree would be tested against the tree its author imagined,
/// and the shapes that break a lowering are the ones a checker produces
/// and nobody thought to write down.
fn lower(text: &str) -> ir::Program {
    lower_with(text, true)
}

/// The same, with the inliner off.
///
/// A golden print of a two-line function has to be a print of *that*
/// function, and with the inliner on a two-line function is inlined into
/// its caller and then dropped by `dce` — so the test would assert on an
/// abort. The pipeline is still the real one; only the budget differs, and
/// the tests that care about what the whole middle end produces run with
/// it on.
fn lower_plain(text: &str) -> ir::Program {
    lower_with(text, false)
}

fn lower_with(text: &str, inline: bool) -> ir::Program {
    let (program, analysis) = compiled(text, inline);
    checked(run(&program, &analysis.checked.tables))
}

/// The snippet, through the front end and the whole middle end, stopping
/// before the lowering — for the tests that need to hand `run_with` a plan
/// of their own.
fn compiled(text: &str, inline: bool) -> (Program, driver::Analysis) {
    let mut map = SourceMap::new();
    let analysis = driver::analyze_snippet(&mut map, "test", text, Role::Entry);
    let complaints: Vec<&str> =
        analysis.diagnostics.items.iter().map(|d| d.message.as_str()).collect();
    assert!(!analysis.diagnostics.has_errors(), "snippet did not compile: {complaints:?}");
    let entry = analysis.checked.entry.expect("the snippet exports `main`");
    let module_paths: Vec<String> =
        analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diags = Diagnostics::new();
    let mut program = monomorphize::run(
        &analysis.checked,
        module_paths,
        &mut diags,
        monomorphize::Roots::Main(entry),
    );
    assert!(!diags.has_errors(), "monomorphization failed");
    let opts = middle::Options {
        inline: crate::compiler::middle::inline::Options { inline },
        ..middle::Options::default()
    };
    middle::run(&mut program, &opts);
    middle::native(&mut program);
    (program, analysis)
}

/// Every test verifies what it lowered: a lowering that produced a
/// malformed CFG and an assertion about one instruction in it would pass.
fn checked(lowered: ir::Program) -> ir::Program {
    let problems = ir::verify(&lowered);
    if !problems.is_empty() && std::env::var("IR_DEBUG").is_ok() {
        println!("{lowered}");
    }
    assert!(problems.is_empty(), "the lowered IR is malformed:\n{}", problems.join("\n"));
    lowered
}

/// The function whose debug name ends with this, rendered.
fn render(p: &ir::Program, suffix: &str) -> String {
    let f = p
        .funcs
        .iter()
        .find(|f| f.debug_name.ends_with(suffix))
        .unwrap_or_else(|| panic!("no function named `{suffix}`"));
    p.render_func(f)
}

/// Wraps a body in a `main` the driver will accept.
fn program(extra: &str, body: &str) -> String {
    format!(
        "from \"native\" import {{ NativeHost }};\n{extra}\n\n\
         export fn main(host: NativeHost): Result<(), Str> {{\n{body}\n  .Ok(())\n}}\n"
    )
}

/// Lowering runs one function per core and folds the per-worker type tables
/// back together afterwards. This pins that the whole is a pure function of
/// the program: two lowerings of one source render the same IR, byte for
/// byte, whatever order the workers finished in — so a `TypeId` cannot move
/// with the scheduler. A merge that read the workers' results in completion
/// order rather than index order, or a remap that dropped a function's ids,
/// would part these two strings.
///
/// The snippet pulls in `core/list` and `core/str` and names four types the
/// interner must agree on — a tuple, a list, a `Str` and a struct — so the
/// program it lowers is hundreds of functions wide, past the point the pass
/// spreads over the cores.
#[test]
fn lowering_is_a_pure_function_of_the_program() {
    let src = program(
        "struct Point { x: Int, y: Int }\n\n\
         fn label(p: Point): Str { \"pt\" }\n\n\
         fn pair(a: Int, b: Str): (Int, Str) { (a, b) }\n\n\
         fn each(xs: [Int]): Int { xs.length() }\n",
        "  let _ = label(Point { x: 1, y: 2 });\n  \
         let _ = pair(3, \"a\");\n  let _ = each([4, 5, 6]);",
    );
    let once = format!("{}", lower(&src));
    let twice = format!("{}", lower(&src));
    assert_eq!(once, twice, "two lowerings of one program rendered differently");
}

/// The reproduction the release-then-retain class was found on, lowered.
///
/// Four lines of Buri: a struct holding a list whose elements are counted,
/// a field of it bound by a `let` **nothing reads**, and the struct read
/// afterwards so the binding is not the last thing alive
/// (`reports/llvm-parallel-listen-fix.md` §1). The report's `let hs = two.b;`
/// is a field path, which `middle::forward` replaces by the path, so the
/// binding here takes the field apart with a pattern instead. The list is freshly
/// allocated, so its count at the drop is one and a release there is a
/// free.
///
/// Asserted over the **emitted instructions** rather than over the plan, so
/// this holds in a build with no `debug_assertions` in it — which the
/// tripwire in `FnLower::rc` does not. It is also the wider claim: a run of
/// reference operations with no instruction between them is one run
/// whatever keys it came from, and a release followed by a retain of one
/// value inside such a run is a write through a header the allocator may
/// already have taken back.
#[test]
fn a_counted_field_bound_and_never_read_is_not_released_then_retained() {
    let p = lower(&program(
        "struct Two { a: Int, b: [Str] }\n\n\
         export fn two(): Int {\n\
         \x20 let two = Two { a: 200, b: [\"h\"] };\n\
         \x20 let Two { a: _, b: hs } = two;\n\
         \x20 two.a + two.b.length()\n\
         }",
        "  let _ = two();",
    ));
    let mut pairs = 0usize;
    for f in &p.funcs {
        let Some(code) = f.code() else { continue };
        for block in &code.blocks {
            // A maximal run of reference operations: nothing between them
            // runs, so the whole run is one moment.
            let mut released: Vec<ValueId> = Vec::new();
            for inst in &block.insts {
                match inst {
                    Inst::DecRef { value, .. } => released.push(*value),
                    Inst::IncRef { value } => {
                        assert!(
                            !released.contains(value),
                            "{}: v{} is released and then retained with nothing between \
                             — a free, and then a write through the freed header\n{}",
                            f.debug_name,
                            value.0,
                            p.render_func(f)
                        );
                        pairs += 1;
                    }
                    _ => released.clear(),
                }
            }
        }
    }
    // Not vacuous: this program really does emit the operations whose order
    // is the claim. On the planner that produced the crash, one of the
    // increments counted here stood in a run that had already released its
    // value — so a lowering that stopped emitting reference operations
    // would satisfy the loop above and is refused here instead.
    assert!(pairs > 0, "no increment was emitted at all, so nothing was ordered");
}

/// Every value a block projects a field out of is still alive when it does
/// so.
///
/// The rule the two shapes below broke, stated once over the emitted
/// instructions: within one basic block, a `DecRef v` may not stand before
/// an instruction that reads `v` as the **base** of a projection, unless
/// something between them retained `v` again. A projection copies words out
/// of the base's block with no count of its own, so a base released first is
/// a read of a block the allocator may already have handed to somebody else
/// — and, as with the release-then-retain class above, the counts balance
/// either way, which is why `check_balance` never saw it.
///
/// The two shapes, both in the corpus below:
///
///  * **A projection off a call the inliner pasted in** (issue #33).
///    `middle::inline` replaces `identity(make(ctx))` with the callee's
///    body, which is a `Block` whose tail is that block's own binding.
///    `middle::rc` scanned a projection's base as a *borrow* whatever shape
///    it was, so the block dropped the binding on its way out and the field
///    read that followed copied words out of it afterwards.
///  * **A `match` whose arm reads a sibling field of the scrutinee's base**
///    (issue #39). `match (s.outcome)` binds payloads that point into `s`,
///    and the arm's own read of `s.manager` is `s`'s last use — so the drop
///    landed inside the arm, before the payloads had been read.
///
/// Asserted over instructions rather than over the plan for the same reason
/// the test above is: this is what the backends are handed.
#[test]
fn a_projection_never_reads_a_base_this_block_has_already_released() {
    let p = lower(&program(
        "from \"platform/effect\" import { Allocator };\n\
         \n\
         struct Inner { export items: [Str] }\n\
         struct Outer { export inner: Inner, export tag: Str }\n\
         enum Held { One { name: Str, rest: [Str] }, Two { name: Str } }\n\
         struct Holder { export held: Held, export label: Str }\n\n\
         fn make<C: Allocator>(ctx: C): Outer {\n\
         \x20 Outer { inner: Inner { items: [\"x\".repeat(ctx, 8)] }, tag: \"y\".repeat(ctx, 8) }\n\
         }\n\n\
         fn identity<T>(value: T): T { value }\n\n\
         fn holder<C: Allocator>(ctx: C): Holder {\n\
         \x20 Holder {\n\
         \x20   held: .One { name: \"n\".repeat(ctx, 8), rest: [\"r\".repeat(ctx, 8)] },\n\
         \x20   label: \"l\".repeat(ctx, 8),\n\
         \x20 }\n\
         }\n\n\
         fn used<C: Allocator>(ctx: C, label: Str, names: [Str]): Int {\n\
         \x20 label.length() + names.map(ctx, fn(s) => s.length()).length()\n\
         }\n\n\
         export fn projected<C: Allocator>(ctx: C): Int {\n\
         \x20 let inner = identity(make(ctx)).inner;\n\
         \x20 inner.items.length()\n\
         }\n\n\
         export fn armed<C: Allocator>(ctx: C): Int {\n\
         \x20 let h = holder(ctx);\n\
         \x20 match (h.held) {\n\
         \x20   .One { name, rest } => used(ctx, h.label, [name].concat(ctx, rest)),\n\
         \x20   .Two { name } => used(ctx, h.label, [name]),\n\
         \x20 }\n\
         }",
        "  let ctx = context { Allocator: host.alloc };\n\
         \x20 let _ = projected(ctx) + armed(ctx);",
    ));
    let mut bases = 0usize;
    for f in &p.funcs {
        let Some(code) = f.code() else { continue };
        for block in &code.blocks {
            let mut released: Vec<ValueId> = Vec::new();
            for inst in &block.insts {
                match inst {
                    Inst::DecRef { value, .. } => released.push(*value),
                    Inst::IncRef { value } => released.retain(|v| v != value),
                    Inst::GetField { agg, .. } | Inst::GetPayload { agg, .. } => {
                        assert!(
                            !released.contains(agg),
                            "{}: v{} is projected out of after this block released it\n{}",
                            f.debug_name,
                            agg.0,
                            p.render_func(f)
                        );
                        bases += 1;
                    }
                    _ => {}
                }
            }
        }
    }
    // Not vacuous: the corpus really does project out of counted bases the
    // same blocks release, which is the situation the rule is about.
    assert!(bases > 0, "no projection was emitted at all, so nothing was ordered");
}

#[test]
fn arithmetic_lowers_to_one_block() {
    let p = lower_plain(&program(
        "export fn add(a: Int, b: Int): Int { a + b }",
        "  let _ = add(1, 2);",
    ));
    assert_eq!(
        render(&p, ":add"),
        "; test:add [unit test]\n\
         fn test$add(i64, i64) -> i64 {\n\
         \x20 b0(v0: i64, v1: i64):\n\
         \x20   v2 = add.I64 v0, v1\n\
         \x20   return v2\n\
         }\n"
    );
}

#[test]
fn an_if_becomes_a_branch_and_a_join_with_a_parameter() {
    let p = lower_plain(&program(
        "export fn pick(c: Bool): Int { if (c) { 1 } else { 2 } }",
        "  let _ = pick(true);",
    ));
    assert_eq!(
        render(&p, ":pick"),
        "; test:pick [unit test]\n\
         fn test$pick(i1) -> i64 {\n\
         \x20 b0(v0: i1):\n\
         \x20   branch v0, b1(), b2()\n\
         \x20 b1():\n\
         \x20   v2 = const 1\n\
         \x20   jump b3(v2)\n\
         \x20 b2():\n\
         \x20   v3 = const 2\n\
         \x20   jump b3(v3)\n\
         \x20 b3(v1: i64):\n\
         \x20   return v1\n\
         }\n"
    );
}

/// The property the whole design rests on: a value that differs per
/// predecessor arrives as a block parameter, which is a phi in the other
/// notation (CODEGEN-LLVM.md §2.1).
#[test]
fn every_join_takes_its_value_as_a_parameter() {
    let p = lower_plain(&program(
        "export fn pick(c: Bool): Int { if (c) { 1 } else { 2 } }",
        "  let _ = pick(false);",
    ));
    for f in &p.funcs {
        let Some(code) = f.code() else { continue };
        let preds = code.preds();
        for (i, b) in code.blocks.iter().enumerate() {
            let n = preds.get(i).map(Vec::len).unwrap_or(0);
            if n > 1 {
                // Every predecessor supplies an argument list of exactly
                // this block's arity — checked by `verify`, and this is
                // the statement of *why* it matters.
                for p in preds.get(i).map(Vec::as_slice).unwrap_or_default() {
                    let t = code
                        .get(*p)
                        .term
                        .targets()
                        .find(|t| t.block.index() == i)
                        .expect("a predecessor names the block");
                    assert_eq!(t.args.len(), b.params.len());
                }
            }
        }
    }
}

#[test]
fn an_enum_match_becomes_one_switch() {
    let src = "
export enum Colour { Red, Green, Blue }

export fn code(c: Colour): Int {
  match (c) {
.Red => 1,
.Green => 2,
.Blue => 3,
  }
}
";
    let p = lower_plain(&program(src, "  let _ = code(.Red);"));
    let text = render(&p, ":code");
    assert!(text.contains("v2 = tag v0"), "{text}");
    assert!(text.contains("switch v2, [0 -> b2(), 1 -> b3(), 2 -> b4()]"), "{text}");
    // Total, so no default: an enum's table always is (VALUE-MODEL.md §6).
    assert!(!text.contains("default"), "{text}");
}

#[test]
fn a_match_with_a_guard_falls_back_to_tests_in_order() {
    let src = "
export fn classify(n: Int): Int {
  match (n) {
0 => 10,
x if (x > 100) => 20,
_ => 30,
  }
}
";
    let p = lower_plain(&program(src, "  let _ = classify(1);"));
    let text = render(&p, ":classify");
    assert!(text.contains("eq.I64"), "{text}");
    assert!(text.contains("gt.I64"), "{text}");
    assert!(!text.contains("switch"), "{text}");
}

/// A self-recursive tail call is a loop before this pass runs
/// (`middle::tail_calls`), so lowering sees a `Loop` and produces a header
/// block with the parameters as block parameters and a back edge to it.
#[test]
fn a_tail_recursive_function_becomes_a_back_edge_to_a_header() {
    let src = "
export fn count(n: Int, acc: Int): Int {
  if (n <= 0) { acc } else { count(n - 1, acc + n) }
}
";
    let p = lower_plain(&program(src, "  let _ = count(10, 0);"));
    let text = render(&p, ":count");
    // The entry falls into a header, and the header is the back edge's
    // destination: the entry block is never a branch target, which both
    // backends require.
    assert!(text.contains("b0(v0: i64, v1: i64):\n    jump b1(v0, v1)"), "{text}");
    let f = p
        .funcs
        .iter()
        .find(|f| f.debug_name.ends_with(":count"))
        .expect("the function is in the program");
    let code = f.code().expect("it has a body");
    let preds = code.preds();
    let header = preds.get(1).cloned().unwrap_or_default();
    assert!(header.len() >= 2, "the header has the entry and a back edge: {header:?}");
    assert!(
        header.iter().any(|b| b.index() > 1),
        "one predecessor comes from later in the function: {header:?}"
    );
    // Nothing jumps to the entry.
    assert!(preds.first().map(Vec::is_empty).unwrap_or(false), "{text}");
}

#[test]
fn a_question_mark_branches_and_returns_early() {
    let src = "
export fn half(n: Int): Option<Int> { if (n % 2 == 0) { .Some(n / 2) } else { .None } }

export fn quarter(n: Int): Option<Int> {
  let h = half(n)?;
  half(h)
}
";
    let p = lower_plain(&program(src, "  let _ = quarter(8);"));
    let text = render(&p, ":quarter");
    // The failure arm builds `.None` at *this* function's return type and
    // returns it, rather than passing the matched value through.
    assert!(text.contains("make #1 ()"), "{text}");
    let returns = text.matches("return").count();
    assert!(returns >= 2, "one return for the early exit and one for the value: {text}");
}

#[test]
fn a_lowered_program_names_one_unit_per_module() {
    let p = lower_plain(&program(
        "export fn id(n: Int): Int { n }",
        "  let _ = id([1, 2].length());",
    ));
    assert!(p.units.iter().any(|u| u == "test"), "{:?}", p.units);
    assert!(p.units.iter().any(|u| u.starts_with("core_")), "{:?}", p.units);
    // Every function's unit is one this program declares.
    for f in &p.funcs {
        assert!((f.unit as usize) < p.units.len());
    }
}

#[test]
fn an_intrinsic_is_a_runtime_symbol_and_not_a_body() {
    let p = lower(&program("", "  let _ = [1, 2].length();"));
    let runtime: Vec<&str> = p.funcs.iter().filter_map(|f| f.intrinsic_key()).collect();
    assert!(
        runtime.contains(&"list.length"),
        "`len` is supplied by the runtime, by key: {runtime:?}"
    );
    for f in &p.funcs {
        match &f.body {
            Body::Runtime(_) => assert!(f.code().is_none()),
            Body::Code(c) => assert!(!c.blocks.is_empty(), "{}", f.debug_name),
        }
    }
}

/// Every pattern form the checker can produce, lowered and verified.
///
/// The value of this one is not the assertions at the bottom; it is that
/// `lower` runs over an array pattern with a rest binding, an
/// alternative pattern that binds nothing, a payload-carrying enum, a
/// struct update and a tuple, and that the CFG it produces passes the
/// verifier — which is the property every backend written after this
/// depends on.
#[test]
fn every_pattern_form_lowers_and_verifies() {
    let src = "
export struct Point { export x: Int, export y: Int }

export enum Shape { Circle(Int), Rect(Int, Int), Empty }

export fn area(s: Shape): Int {
  match (s) {
.Circle(r) => r * r * 3,
.Rect(w, h) => w * h,
.Empty => 0,
  }
}

export fn head(xs: [Int]): Int {
  match (xs) {
[] => 0,
[a] => a,
[a, b, ..rest] => a + b + rest.length(),
  }
}

export fn label(n: Int): Str {
  match (n) {
0 | 1 => \"small\",
_ => \"big\",
  }
}

export fn moved(p: Point): Point { Point { ..p, x: p.x + 1 } }

export fn both(t: (Int, Bool)): Int { match (t) { (n, true) => n, (n, false) => 0 - n } }
";
    let p = lower_plain(&program(
        src,
        "
  let _a = area(.Rect(2, 3));
  let _h = head([1, 2, 3]);
  let _l = label(2);
  let _m = moved(Point { x: 1, y: 2 });
  let _b = both((1, true));
",
    ));
    assert!(ir::verify(&p).is_empty());
    let all: String = p.to_string();
    assert!(all.contains("payload."), "an enum arm projects its payload: {all}");
    assert!(all.contains("= len "), "an array pattern tests the length");
    assert!(all.contains("= slice "), "a rest binding slices");
    assert!(all.contains("= field."), "a struct update reads the fields it keeps");
}

/// A mutually tail-recursive group is one function with a dispatch
/// parameter, and the dispatch parameter is this pass's to materialise.
///
/// `middle::tail_calls` merges the group and leaves the entry index as a
/// number on `ExprKind::Continue`, deliberately not smuggled in as an
/// argument at a type it would have had to invent. So the merged
/// function's signature grows a leading `i32` here, its header switches on
/// it, and a member with fewer parameters than the widest one pads the
/// slots it has nothing for.
#[test]
fn a_merged_tail_call_group_switches_on_a_dispatch_parameter() {
    let src = "
export fn walk(n: Int, acc: Int): Int {
  if (n <= 0) { acc } else { step(n - 1) }
}

export fn step(n: Int): Int {
  if (n <= 0) { 0 } else { walk(n - 1, n) }
}
";
    let p = lower_plain(&program(src, "  let _ = walk(4, 0);"));
    let merged = p
        .funcs
        .iter()
        .find(|f| f.debug_name.starts_with("tail group"))
        .expect("the group was merged into one function");
    let text = p.render_func(merged);
    assert_eq!(merged.sig.params.first(), Some(&ir::Type::I32), "{text}");
    // The entry falls into a header carrying every parameter, including
    // the index, and the header is what switches — the entry block is
    // never a branch target on either backend.
    assert!(text.contains("b0(v0: i32, v1: i64, v2: i64):\n    jump b1(v0, v1, v2)"), "{text}");
    assert!(text.contains("switch v3, [0 -> b3(), 1 -> b4()]"), "{text}");
    assert!(text.contains("const undef"), "the narrower member pads: {text}");
    // And the members forward into it rather than being deleted: an
    // `FnRef` or a non-tail call to one still has to work.
    for name in [":walk", ":step"] {
        let f = p
            .funcs
            .iter()
            .find(|f| f.debug_name.ends_with(name))
            .expect("the member kept its name");
        assert!(p.render_func(f).contains("call f"), "{name} forwards");
    }
}

/// `middle::rc`'s plan is placed, not ignored.
///
/// The plan is built here rather than taken from `rc::analyze`, and that
/// is the point: a test that waited for the analysis to emit a site at
/// this program would be testing that pass rather than this contract. What
/// is asserted is the half that lives here: a site at a node names a value,
/// and the instruction lands at that node, before or after, in plan order.
#[test]
fn a_plan_site_becomes_an_instruction_at_its_node() {
    let (program, analysis) = compiled(
        &program("export fn keep(s: Str): Str { s }", "  let _ = keep(\"hi\");"),
        false,
    );
    let target = program
        .funcs
        .iter()
        .position(|f| f.debug_name.ends_with(":keep"))
        .expect("the function is in the program");

    let mut plan = rc::Plan { funcs: Vec::new(), crosses_tasks: false };
    for (i, f) in program.funcs.iter().enumerate() {
        let params = vec![Ownership::Own; f.params.len()];
        let sites = if i == target {
            let local = *f.params.first().expect("`keep` takes one parameter");
            vec![
                rc::Site {
                    node: rc::NodeId(0),
                    at: rc::Position::Before,
                    op: rc::RcOp::IncRef,
                    target: rc::Target::Local(local),
                },
                rc::Site {
                    node: rc::NodeId(0),
                    at: rc::Position::After,
                    op: rc::RcOp::DecRef,
                    target: rc::Target::Node(rc::NodeId(0)),
                },
            ]
        } else {
            Vec::new()
        };
        plan.funcs.push(rc::FuncPlan {
            params,
            purity: ir::Purity::Pure,
            can_abort: false,
            sites,
            reuse: Vec::new(),
            unclassified: Vec::new(),
            inherits: Vec::new(),
            moved: Vec::new(),
        });
    }

    let lowered = checked(run_with(&program, &analysis.checked.tables, &plan));
    let f = lowered
        .funcs
        .iter()
        .find(|f| f.debug_name.ends_with(":keep"))
        .expect("the function survived lowering");
    assert_eq!(
        lowered.render_func(f),
        "; test:keep [unit test]\n\
         fn test$keep(Str) -> Str {\n\
         \x20 b0(v0: Str):\n\
         \x20   incref v0\n\
         \x20   decref v0\n\
         \x20   return v0\n\
         }\n"
    );
    // And the facts come from the plan rather than from the conservative
    // default `ir::Facts` documents.
    assert_eq!(f.facts.purity, ir::Purity::Pure);
    assert!(!f.facts.can_abort);
}

/// A `let` whose binding nothing reads is dropped where it was bound, and
/// **after** the value's own reference operations.
///
/// `middle::rc` keys that drop on the *value's* node and [`FnLower::rc`]
/// skips a site naming a local nothing has bound yet, so emitting the
/// node's `After` operations before `pattern` ran threw it away — one
/// leaked block per unread binding, and `rc`'s own balance checker had
/// said all along that the binding exists first.
///
/// The order is the other half of the same question, and it is the half
/// that was a use-after-free rather than a leak: the value's own
/// operations land at this drop's key, so a plan that pushed the drop
/// first released a value before the `incref` that paid for it
/// (`middle/rc.rs`'s `Stmt::Let` case, and
/// `reports/llvm-parallel-listen-fix.md` for what that cost). The two
/// drops below are independent — `v2` is the literal the call borrowed,
/// `v3` is the block it returned — so what this string pins is which side
/// of the initializer the binding's drop comes out on.
#[test]
fn a_binding_nothing_reads_is_still_dropped() {
    let p = lower_plain(&program(
        "
from \"platform/effect\" import { Allocator };

export fn junk<C: Allocator>(ctx: C, n: Int): Int {
  let s = \"z\".repeat(ctx, n);
  n
}
",
        "  let _ = junk(context { Allocator: host.alloc }, 4);",
    ));
    assert_eq!(
        render(&p, ":junk"),
        "; test:junk [unit test]\n\
         fn test$junk$ujc6dt(a context, i64) -> i64 {\n\
         \x20 b0(v0: a context, v1: i64):\n\
         \x20   v2 = const \"z\"\n\
         \x20   v3 = call fn core_str$Str_repeat$ujc6dt(v2, v0, v1)\n\
         \x20   decref v2\n\
         \x20   decref v3\n\
         \x20   return v1\n\
         }\n"
    );
}

/// The whole standard library a program touches, lowered and verified.
/// This is the test that finds the expression shape nobody thought of.
#[test]
fn the_standard_library_a_program_reaches_lowers_and_verifies() {
    let p = lower(&program(
        "
export struct Point { export x: Int, export y: Int }

export fn sum(xs: [Int]): Int { xs.fold(fn(a, b) => a + b, 0) }
",
        "
  let pt = Point { x: 1, y: 2 };
  let _s = \"${pt.x}-${pt.y}\";
  let _n = sum([1, 2, 3]);
  let _o = [1, 2, 3].get(1);
  let _l = [1, 2, 3].length();
",
    ));
    assert!(ir::verify(&p).is_empty());
    // Both kinds of body reach the backend: generated code, and a
    // runtime symbol. `fold` and `get` are code now (`lower/lists.rs`);
    // `length` is the runtime symbol.
    assert!(p.funcs.iter().any(|f| f.code().is_some()));
    assert!(p.funcs.iter().any(|f| f.intrinsic_key().is_some()));
}
