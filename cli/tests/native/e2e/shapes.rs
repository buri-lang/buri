//! **Large data shapes**, each a whole program printing what the generator here
//! works out it must print, on this toolchain's native backend under the heap
//! check and on JavaScript.
//!
//! How the compiler's work grows with each shape is held in `stencil.rs`,
//! `llvm.rs` and `build/profile.rs`. These rows hold what a user sees.

use super::{heap_checked, ready};

const HEAD: &str = r#"from "platform/effect" import { Allocator, Stdout };
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/map" import * as map;
from "core/map" import { Map };
from "core/str" import * as str;
"#;

const MAIN: &str = "export fn main(host: NativeHost): Result<(), Str> {\n    \
                    let ctx = context { Allocator: host.alloc, Stdout: host.stdout };\n";

fn say(expr: &str) -> String {
    format!("    let _ = io.println(ctx, {expr}).ignore();\n")
}

/// `source` prints `expected` natively, giving back every block, and on
/// JavaScript.
fn prints(name: &str, source: &str, expected: &[String]) {
    let (stdout, stderr) = heap_checked(name, source);
    assert_eq!(stdout, expected, "native, stderr:\n{stderr}");
    if crate::shared::js_engine().is_none() {
        crate::ci::skipped("e2e javascript", "no JavaScript engine on PATH");
        return;
    }
    let (checked, paths) = crate::agreement::analyze(name, source);
    let js = crate::agreement::run_js(name, &checked, &paths);
    assert_eq!(js.status, 0, "javascript exited {}:\n{}", js.status, js.stderr);
    let lines: Vec<String> = js.stdout.lines().map(str::to_string).collect();
    assert_eq!(lines, expected, "javascript, stderr:\n{}", js.stderr);
}

fn join<T>(items: impl Iterator<Item = T>, sep: &str) -> String
where
    T: std::fmt::Display,
{
    items.map(|i| i.to_string()).collect::<Vec<_>>().join(sep)
}

/// `nth(i)` as an `n`-arm match from an `Int`.
fn nth(n: usize) -> String {
    let arms: String = (0..n - 1).map(|i| format!("        {i} => .V{i},\n")).collect();
    format!("fn nth(i: Int): E {{\n    match (i) {{\n{arms}        _ => .V{},\n    }}\n}}\n", n - 1)
}

/// An enum of `n` unit variants with every derive, built from an index and
/// matched in full.
fn unit_enum(n: usize) -> (String, Vec<String>) {
    let variants: String = (0..n).map(|i| format!("    V{i},\n")).collect();
    let codes: String = (0..n).map(|i| format!("        .V{i} => {},\n", i * 7 % 1000 + 3)).collect();
    let source = format!(
        "{HEAD}\nenum E {{\n{variants}}}\n\nderive Equal, Hash, Show, Ordered for E;\n\n{}\n\
         fn code(e: E): Int {{\n    match (e) {{\n{codes}    }}\n}}\n\n{MAIN}\
         \x20   let all = list.range(ctx, 0, {n}).map(ctx, fn(i) => nth(i));\n\
         \x20   let again = list.range(ctx, 0, {n}).map(ctx, fn(i) => nth(i));\n\
         \x20   let total = all.fold(fn(acc, e) => acc + code(e), 0);\n\
         \x20   let shown = all.mapCtx(ctx, fn(c, e) => e.show(c)).join(ctx, \",\");\n\
         \x20   let sorted = all.isSortedBy(fn(x, y) => x.compare(y));\n\
         \x20   let resorted = all.reverse(ctx).sort(ctx);\n{}{}{}    .Ok(())\n}}\n",
        nth(n),
        say("\"${total} ${all == again} ${sorted} ${resorted == all}\""),
        say(&format!(
            "\"${{nth(3) < nth(4)}} ${{nth({}) > nth(0)}} ${{nth(5).hash() == nth(5).hash()}}\"",
            n - 1
        )),
        say(&format!("\"${{shown.length()}} ${{nth({})}}\"", n / 2)),
    );
    let total: usize = (0..n).map(|i| i * 7 % 1000 + 3).sum();
    let shown = join((0..n).map(|i| format!(".V{i}")), ",");
    let expected = vec![
        format!("{total} true true true"),
        String::from("true true true"),
        format!("{} .V{}", shown.len(), n / 2),
    ];
    (source, expected)
}

/// An enum of `n` variants whose payloads cycle through a number, a string,
/// both, a record holding an option, and nothing.
fn payload_enum(n: usize) -> (String, Vec<String>) {
    let (mut decls, mut builds, mut codes) = (String::new(), String::new(), String::new());
    let (mut total, mut shown) = (0usize, Vec::new());
    for i in 0..n {
        let len = i % 3 + 1;
        let s = "s".repeat(len);
        let (decl, build, code, value, show) = match i % 5 {
            0 => (
                format!("V{i}(Int)"),
                format!(".V{i}(i * 2)"),
                format!(".V{i}(x) => x + {i}"),
                3 * i,
                format!(".V{i}({})", i * 2),
            ),
            1 => (
                format!("V{i}(Str)"),
                format!(".V{i}(s)"),
                format!(".V{i}(t) => t.length() + {i}"),
                len + i,
                format!(".V{i}(\"{s}\")"),
            ),
            2 => (
                format!("V{i}(Int, Str)"),
                format!(".V{i}(i, s)"),
                format!(".V{i}(x, t) => x + t.length()"),
                i + len,
                format!(".V{i}({i}, \"{s}\")"),
            ),
            3 => (
                format!("V{i} {{ a: Int, b: Option<Str> }}"),
                format!(".V{i} {{ a: i, b: .Some(s) }}"),
                format!(".V{i} {{ a, b: .Some(t) }} => a + t.length(),\n        .V{i} {{ a, b: .None }} => a"),
                i + len,
                format!(".V{i} {{ a: {i}, b: .Some(\"{s}\") }}"),
            ),
            _ => (format!("V{i}"), format!(".V{i}"), format!(".V{i} => {i}"), i, format!(".V{i}")),
        };
        decls.push_str(&format!("    {decl},\n"));
        builds.push_str(&format!("        {i} => {build},\n"));
        codes.push_str(&format!("        {code},\n"));
        total += value;
        shown.push(show);
    }
    let source = format!(
        "{HEAD}\nenum E {{\n{decls}}}\n\nderive Equal, Hash, Show, Ordered for E;\n\n\
         fn nth<C: Allocator>(ctx: C, i: Int): E {{\n    let s = \"s\".repeat(ctx, i % 3 + 1);\n    \
         match (i) {{\n{builds}        _ => .V0(0),\n    }}\n}}\n\n\
         fn code(e: E): Int {{\n    match (e) {{\n{codes}    }}\n}}\n\n{MAIN}\
         \x20   let all = list.range(ctx, 0, {n}).mapCtx(ctx, fn(c, i) => nth(c, i));\n\
         \x20   let again = list.range(ctx, 0, {n}).mapCtx(ctx, fn(c, i) => nth(c, i));\n\
         \x20   let total = all.fold(fn(acc, e) => acc + code(e), 0);\n\
         \x20   let shown = all.mapCtx(ctx, fn(c, e) => e.show(c)).join(ctx, \",\");\n\
         \x20   let sorted = all.isSortedBy(fn(x, y) => x.compare(y));\n\
         \x20   let resorted = all.reverse(ctx).sort(ctx);\n\
         \x20   let hashed = all.fold(fn(acc, e) => acc && e.hash() == e.hash(), true);\n{}{}    .Ok(())\n}}\n",
        say("\"${total} ${all == again} ${sorted} ${resorted == all} ${hashed}\""),
        say("\"${shown.length()} ${nth(ctx, 2)} ${nth(ctx, 3)}\""),
    );
    let joined = shown.join(",");
    let expected = vec![
        format!("{total} true true true true"),
        format!("{} {} {}", joined.len(), shown[2], shown[3]),
    ];
    (source, expected)
}

/// The `Int`s and `Str`s of a wide shape: every other one a string.
fn kinds(n: usize) -> Vec<bool> {
    (0..n).map(|i| i % 2 == 1).collect()
}

/// An enum whose `Tuple` and `Record` variants carry `n` fields each.
fn wide_payload(n: usize) -> (String, Vec<String>) {
    let strs = kinds(n);
    let types = join(strs.iter().map(|s| if *s { "Str" } else { "Int" }), ", ");
    let fields = join(strs.iter().enumerate().map(|(i, s)| format!("f{i}: {}", if *s { "Str" } else { "Int" })), ", ");
    let values = join(strs.iter().enumerate().map(|(i, s)| if *s { String::from("s") } else { format!("k + {i}") }), ", ");
    let named = join(strs.iter().enumerate().map(|(i, s)| if *s { format!("f{i}: s") } else { format!("f{i}: k + {i}") }), ", ");
    let binds = join((0..n).map(|i| format!("a{i}")), ", ");
    let summed = join(strs.iter().enumerate().map(|(i, s)| if *s { format!("a{i}.length()") } else { format!("a{i}") }), " + ");
    let last = n - 1;
    let read_last = if strs[last] { format!("f{last}.length()") } else { format!("f{last}") };
    // Equal up to the last `Int`, which is `x`.
    let late = strs.iter().rposition(|s| !*s).unwrap_or(0);
    let lates = join(
        strs.iter().enumerate().map(|(i, s)| match (*s, i == late) {
            (true, _) => String::from("\"w\""),
            (false, true) => String::from("x"),
            (false, false) => i.to_string(),
        }),
        ", ",
    );
    let source = format!(
        "{HEAD}\nenum W {{\n    Tuple({types}),\n    Record {{ {fields} }},\n    Empty,\n}}\n\n\
         derive Equal, Hash, Show, Ordered for W;\n\n\
         fn make<C: Allocator>(ctx: C, k: Int, which: Int): W {{\n    \
         let s = \"w\".repeat(ctx, k % 3 + 1);\n    match (which) {{\n        \
         0 => .Tuple({values}),\n        1 => .Record {{ {named} }},\n        _ => .Empty,\n    }}\n}}\n\n\
         fn sum(w: W): Int {{\n    match (w) {{\n        .Tuple({binds}) => {summed},\n        \
         .Record {{ f{last}, .. }} => {read_last},\n        .Empty => 0 - 1,\n    }}\n}}\n\n\
         fn late(x: Int): W {{\n    .Tuple({lates})\n}}\n\n{MAIN}\
         \x20   let ws = list.range(ctx, 0, 12).mapCtx(ctx, fn(c, i) => make(c, i, i % 3));\n\
         \x20   let again = list.range(ctx, 0, 12).mapCtx(ctx, fn(c, i) => make(c, i, i % 3));\n\
         \x20   let total = ws.fold(fn(acc, w) => acc + sum(w), 0);\n\
         \x20   let hashed = ws.fold(fn(acc, w) => acc && w.hash() == w.hash(), true);\n{}{}{}    .Ok(())\n}}\n",
        say("\"${total} ${ws == again} ${hashed} ${ws.sort(ctx).isSortedBy(fn(x, y) => x.compare(y))}\""),
        say("\"${make(ctx, 1, 0) < make(ctx, 2, 0)} ${make(ctx, 1, 1) > make(ctx, 1, 0)} ${make(ctx, 1, 0)}\""),
        say("\"${late(1) == late(1)} ${late(1) == late(2)} ${late(1) < late(2)} ${late(2) > late(1)} ${late(1).hash() == late(1).hash()} ${late(1).hash() == late(2).hash()}\""),
    );
    let total: i64 = (0..12usize)
        .map(|k| {
            let len = (k % 3 + 1) as i64;
            match k % 3 {
                0 => strs.iter().enumerate().map(|(i, s)| if *s { len } else { (k + i) as i64 }).sum(),
                1 => if strs[last] { len } else { (k + last) as i64 },
                _ => -1,
            }
        })
        .sum();
    let shown = format!(
        ".Tuple({})",
        join(strs.iter().enumerate().map(|(i, s)| if *s { String::from("\"ww\"") } else { (1 + i).to_string() }), ", ")
    );
    (source, vec![format!("{total} true true true"), format!("true true {shown}"), String::from("true false true true true false")])
}

/// `Option<Result<Option<…Int…>, Str>>`, `depth` levels, in a struct, matched
/// in one pattern.
fn nested_generic(depth: usize) -> (String, Vec<String>) {
    let mut ty = String::from("Int");
    let wrap = |inner: &str| {
        let mut v = inner.to_string();
        for j in 0..depth {
            v = if j % 2 == 0 { format!(".Some({v})") } else { format!(".Ok({v})") };
        }
        v
    };
    for j in 0..depth {
        ty = if j % 2 == 0 { format!("Option<{ty}>") } else { format!("Result<{ty}, Str>") };
    }
    let source = format!(
        "{HEAD}\nstruct Deep {{\n    v: {ty},\n    tag: Str,\n}}\n\n\
         derive Equal, Hash, Show, Ordered for Deep;\n\n\
         fn wrap<C: Allocator>(ctx: C, x: Int): Deep {{\n    Deep {{ v: {}, tag: \"t\".repeat(ctx, 2) }}\n}}\n\n\
         fn open(d: Deep): Int {{\n    match (d.v) {{\n        {} => x,\n        _ => 0 - 1,\n    }}\n}}\n\n{MAIN}\
         \x20   let a = wrap(ctx, 7);\n    let b = wrap(ctx, 7);\n    let c = wrap(ctx, 9);\n{}{}    .Ok(())\n}}\n",
        wrap("x"),
        wrap("x"),
        say("\"${open(a)} ${open(c)} ${a == b} ${a == c} ${a < c} ${a.hash() == b.hash()}\""),
        say("\"${[c, a, b].sort(ctx).first() == .Some(a)} ${a}\""),
    );
    let shown = format!("Deep {{ v: {}, tag: \"tt\" }}", wrap("7"));
    (source, vec![String::from("7 9 true false true true"), format!("true {shown}")])
}

/// `n` enums, each wrapping the one before, with every derive on each.
fn enum_chain(n: usize) -> (String, Vec<String>) {
    let mut decls = String::from("enum L0 {\n    Leaf(Int),\n    Gone(Str),\n}\n\nderive Equal, Hash, Show, Ordered for L0;\n");
    for i in 1..n {
        decls.push_str(&format!(
            "\nenum L{i} {{\n    Wrap(L{}),\n    Stop(Str),\n}}\n\nderive Equal, Hash, Show, Ordered for L{i};\n",
            i - 1
        ));
    }
    let wrap = |x: &str| (1..n).fold(format!(".Leaf({x})"), |v, _| format!(".Wrap({v})"));
    let top = n - 1;
    let source = format!(
        "{HEAD}\n{decls}\nfn build(x: Int): L{top} {{\n    {}\n}}\n\n\
         fn open(l: L{top}): Int {{\n    match (l) {{\n        {} => x,\n        _ => 0 - 1,\n    }}\n}}\n\n{MAIN}\
         \x20   let a = build(3);\n    let b = build(3);\n    let c = build(4);\n\
         \x20   let stopped: L{top} = .Stop(\"s\".repeat(ctx, 3));\n{}{}    .Ok(())\n}}\n",
        wrap("x"),
        wrap("x"),
        say("\"${open(a)} ${open(stopped)} ${a == b} ${a == c} ${a < c} ${a.hash() == b.hash()} ${stopped < a}\""),
        say("\"${a} ${stopped}\""),
    );
    (source, vec![String::from("3 -1 true false true true false"), format!("{} .Stop(\"sss\")", wrap("3"))])
}

/// A tuple of `n` elements, built, taken apart, compared and shown.
fn long_tuple(n: usize) -> (String, Vec<String>) {
    let strs = kinds(n);
    let types = join(strs.iter().map(|s| if *s { "Str" } else { "Int" }), ", ");
    let values = join(strs.iter().enumerate().map(|(i, s)| if *s { String::from("s") } else { format!("k + {i}") }), ", ");
    let binds = join((0..n).map(|i| format!("a{i}")), ", ");
    let summed = join(strs.iter().enumerate().map(|(i, s)| if *s { format!("a{i}.length()") } else { format!("a{i}") }), " + ");
    let source = format!(
        "{HEAD}\nfn make<C: Allocator>(ctx: C, k: Int): ({types}) {{\n    \
         let s = \"u\".repeat(ctx, k % 3 + 1);\n    ({values})\n}}\n\n\
         fn sum(t: ({types})): Int {{\n    let ({binds}) = t;\n    {summed}\n}}\n\n{MAIN}\
         \x20   let a = make(ctx, 1);\n    let b = make(ctx, 1);\n    let c = make(ctx, 2);\n{}{}    .Ok(())\n}}\n",
        say(&format!("\"${{sum(a)}} ${{a == b}} ${{a == c}} ${{a < c}} ${{a.{}}}\"", n - 2)),
        say("\"${[c, a, b].sort(ctx).first() == .Some(a)} ${a}\""),
    );
    let parts: Vec<String> =
        strs.iter().enumerate().map(|(i, s)| if *s { String::from("\"uu\"") } else { (1 + i).to_string() }).collect();
    let total: usize = strs.iter().enumerate().map(|(i, s)| if *s { 2 } else { 1 + i }).sum();
    (
        source,
        vec![format!("{total} true false true {}", parts[n - 2]), format!("true ({})", parts.join(", "))],
    )
}

/// A struct of `fields` fields, `count` of them in a list, a map by a field and
/// a map by the whole record.
fn records(fields: usize, count: usize) -> (String, Vec<String>) {
    let strs: Vec<bool> = (0..fields).map(|i| i % 3 == 1).collect();
    let decl: String =
        strs.iter().enumerate().map(|(i, s)| format!("    f{i}: {},\n", if *s { "Str" } else { "Int" })).collect();
    let values = join(
        strs.iter().enumerate().map(|(i, s)| if *s { format!("f{i}: s") } else { format!("f{i}: k * {}", i + 1) }),
        ", ",
    );
    let last = if strs[fields - 1] { fields - 2 } else { fields - 1 };
    let source = format!(
        "{HEAD}\nstruct R {{\n{decl}}}\n\nderive Equal, Hash, Show, Ordered for R;\n\n\
         fn make<C: Allocator>(ctx: C, k: Int): R {{\n    let s = str.format(ctx, \"r${{k}}\");\n    R {{ {values} }}\n}}\n\n{MAIN}\
         \x20   let rs = list.range(ctx, 0, {count}).mapCtx(ctx, fn(c, i) => make(c, ({count} - i) * 7 % {count}));\n\
         \x20   let again = list.range(ctx, 0, {count}).mapCtx(ctx, fn(c, i) => make(c, ({count} - i) * 7 % {count}));\n\
         \x20   let sorted = rs.sort(ctx);\n\
         \x20   let byKey = rs.foldCtx(ctx, fn(c, m: Map<Str, R>, r) => m.insert(c, r.f1, r), map.empty<Str, R>());\n\
         \x20   let byValue = rs.foldCtx(ctx, fn(c, m: Map<R, Int>, r) => m.insert(c, r, r.f0), map.empty<R, Int>());\n\
         \x20   let found = list.range(ctx, 0, {count}).foldCtx(ctx, fn(c, acc, k) => acc + byValue.get(make(c, k)).withDefault(0 - 1000), 0);\n\
         \x20   let total = sorted.fold(fn(acc, r) => acc + r.f{last}, 0);\n{}{}    .Ok(())\n}}\n",
        say("\"${rs == again} ${sorted.isSortedBy(fn(x, y) => x.compare(y))} ${byKey.length()} ${byValue.length()} ${found} ${total}\""),
        say("\"${make(ctx, 3).hash() == make(ctx, 3).hash()} ${make(ctx, 3) < make(ctx, 4)} ${make(ctx, 3)}\""),
    );
    let keys: Vec<usize> = (0..count).map(|i| (count - i) * 7 % count).collect();
    let distinct = keys.iter().collect::<std::collections::BTreeSet<_>>().len();
    let found: usize = (0..count).sum();
    let total: usize = keys.iter().map(|k| k * (last + 1)).sum();
    let shown = format!(
        "R {{ {} }}",
        join(
            strs.iter().enumerate().map(|(i, s)| if *s { format!("f{i}: \"r3\"") } else { format!("f{i}: {}", 3 * (i + 1)) }),
            ", "
        )
    );
    (
        source,
        vec![format!("true true {distinct} {distinct} {found} {total}"), format!("true true {shown}")],
    )
}

/// Matches of `n` arms over an `n`-variant enum: a diagonal over pairs,
/// or-groups of three that leave the last three variants to `_`, string
/// literals and guards.
fn long_match(n: usize) -> (String, Vec<String>) {
    let variants: String = (0..n).map(|i| format!("    V{i},\n")).collect();
    let pairs: String = (0..n).map(|i| format!("        (.V{i}, .V{i}) => {i},\n")).collect();
    let groups: String = (0..n.saturating_sub(3))
        .step_by(3)
        .map(|i| format!("        .V{i} | .V{} | .V{} => {},\n", i + 1, i + 2, i / 3))
        .collect();
    let words: String = (0..n).map(|i| format!("        \"w{i}\" => {i},\n")).collect();
    let guards: String = (0..n).map(|i| format!("        .V{i} if k > {i} => {i},\n")).collect();
    let source = format!(
        "{HEAD}\nenum E {{\n{variants}}}\n\n{}\n\
         fn same(a: E, b: E): Int {{\n    match ((a, b)) {{\n{pairs}        _ => 0 - 1,\n    }}\n}}\n\n\
         fn group(e: E): Int {{\n    match (e) {{\n{groups}        _ => 0 - 1,\n    }}\n}}\n\n\
         fn word(w: Str): Int {{\n    match (w) {{\n{words}        _ => 0 - 1,\n    }}\n}}\n\n\
         fn guard(e: E, k: Int): Int {{\n    match (e) {{\n{guards}        _ => 0 - 1,\n    }}\n}}\n\n{MAIN}\
         \x20   let idx = list.range(ctx, 0, {n});\n\
         \x20   let diagonal = idx.fold(fn(acc, i) => acc + same(nth(i), nth(i)), 0);\n\
         \x20   let off = idx.fold(fn(acc, i) => acc + same(nth(i), nth((i + 1) % {n})), 0);\n\
         \x20   let groups = idx.fold(fn(acc, i) => acc + group(nth(i)), 0);\n\
         \x20   let words = idx.foldCtx(ctx, fn(c, acc, i) => acc + word(str.format(c, \"w${{i}}\")), 0);\n\
         \x20   let guards = idx.fold(fn(acc, i) => acc + guard(nth(i), {}), 0);\n{}    .Ok(())\n}}\n",
        nth(n),
        n / 2,
        say("\"${diagonal} ${off} ${groups} ${words} ${guards} ${word(\"nope\")}\""),
    );
    let all: i64 = (0..n as i64).sum();
    let mut group = vec![-1i64; n];
    for i in (0..n.saturating_sub(3)).step_by(3) {
        for slot in group.iter_mut().skip(i).take(3) {
            *slot = (i / 3) as i64;
        }
    }
    let groups: i64 = group.iter().sum();
    let guards: i64 = (0..n).map(|i| if n / 2 > i { i as i64 } else { -1 }).sum();
    (source, vec![format!("{all} {} {groups} {all} {guards} -1", -(n as i64))])
}

/// A template of `n` holes, half numbers and half strings built at run time,
/// formatted and printed.
fn long_template(n: usize) -> (String, Vec<String>) {
    let lets: String = (0..n)
        .map(|i| {
            if i % 2 == 0 {
                format!("    let a{i} = {i} * 3;\n")
            } else {
                format!("    let a{i} = \"p{i}\".repeat(ctx, 2);\n")
            }
        })
        .collect();
    let holes = join((0..n).map(|i| format!("${{a{i}}}")), "|");
    let source = format!(
        "{HEAD}\n{MAIN}{lets}    let line = str.format(ctx, \"<{holes}>\");\n{}{}{}    .Ok(())\n}}\n",
        say("line"),
        say("\"${line.length()}\""),
        say(&format!("\"{holes}\"")),
    );
    let parts = join((0..n).map(|i| if i % 2 == 0 { (i * 3).to_string() } else { format!("p{i}p{i}") }), "|");
    let line = format!("<{parts}>");
    (source, vec![line.clone(), line.len().to_string(), parts])
}

/// **An enum of three hundred unit variants derives, sorts and matches.**
#[test]
fn an_enum_of_hundreds_of_unit_variants_derives_and_matches() {
    unless_ready!();
    let (source, expected) = unit_enum(300);
    prints("e2e-shape-unit-enum", &source, &expected);
}

/// **An enum of a hundred and twenty variants with payloads of every kind.**
#[test]
fn an_enum_of_hundreds_of_payload_variants_derives_and_matches() {
    unless_ready!();
    let (source, expected) = payload_enum(120);
    prints("e2e-shape-payload-enum", &source, &expected);
}

/// **Variants carrying twenty-four fields each, as a tuple and as a record.**
#[test]
fn an_enum_whose_variants_carry_wide_payloads_derives_and_matches() {
    unless_ready!();
    let (source, expected) = wide_payload(24);
    prints("e2e-shape-wide-payload", &source, &expected);
}

/// **Options and results nested sixteen deep, matched in one pattern.**
#[test]
fn options_and_results_nested_deep_derive_and_match() {
    unless_ready!();
    let (source, expected) = nested_generic(16);
    prints("e2e-shape-nested-generic", &source, &expected);
}

/// **A chain of sixteen enums, each wrapping the one before.**
#[test]
fn a_chain_of_enums_each_wrapping_the_last_derives_and_matches() {
    unless_ready!();
    let (source, expected) = enum_chain(16);
    prints("e2e-shape-enum-chain", &source, &expected);
}

/// **A tuple of twenty-four elements, taken apart, compared, sorted and shown.**
#[test]
fn a_long_tuple_is_taken_apart_compared_and_shown() {
    unless_ready!();
    let (source, expected) = long_tuple(24);
    prints("e2e-shape-long-tuple", &source, &expected);
}

/// **Records of twenty-four fields in a list, a map by key and a map by
/// value.**
#[test]
fn large_records_in_lists_and_maps_sort_hash_and_leak_nothing() {
    unless_ready!();
    let (source, expected) = records(24, 60);
    prints("e2e-shape-records", &source, &expected);
}

/// **Matches of three hundred arms take the arm they should.**
#[test]
fn long_matches_over_a_many_variant_enum_take_the_right_arm() {
    unless_ready!();
    let (source, expected) = long_match(300);
    prints("e2e-shape-long-match", &source, &expected);
}

/// **A template of two hundred holes reads the same as it was written.**
#[test]
fn a_template_of_hundreds_of_holes_reads_the_same_and_leaks_nothing() {
    unless_ready!();
    let (source, expected) = long_template(200);
    prints("e2e-shape-long-template", &source, &expected);
}
