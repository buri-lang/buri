//! **The sticky shared bit**: the two claims that make writing into a list
//! legal, and the one that makes it worth doing.
//!
//! `design/native/MEMORY.md` §5.5 is the design. What is asserted here is what
//! the design has to be true for:
//!
//!  * **Provenance.** A JavaScript array this backend did not allocate carries
//!    no bit, and absence reads as *shared*, so it is copied and never written
//!    to — and no mark a host can read by name is ever written onto it either.
//!    A frozen or sealed one is marked without throwing. The `core/list` surface
//!    is asked this directly, because the host boundary is a property of the
//!    runtime rather than of any program.
//!  * **Aliasing.** Every case where two names reach one list is in the
//!    conformance corpus (`conformance/lib/data/test/lists.buri`, "aliasing"),
//!    where both backends run it and the answers have to match.
//!  * **Cost.** Growing a list in a loop is linear — whether the list is the
//!    last field its record's literal writes or not, and `core/buri/ast`'s
//!    printer, which the build runs for every `.proto` in a repository, is
//!    linear because of it. Asserted as a ratio between the same *total*
//!    work done in small runs and in large ones: linear work makes those
//!    equal, and the copying they replaced makes the second several times the
//!    first. The work is counted, never timed, so load can't move it: the
//!    instructions the JavaScript runtime retires, where macOS counts them,
//!    and everywhere the elements every `Array.prototype.slice` copied.
//!
//! ```text
//! cargo test -p buri --test language sharing::
//! ```

use std::process::{Command, Stdio};

use crate::harness::{JS_BINARY, Scratch, js_runtime};

/// The runtime, plus a script that asks it the questions a Buri program
/// cannot: what a host array is, and what a mark is written on.
///
/// Every claim is a `throw`, so the assertion is the exit code and the message
/// is the thrown one.
const HOST_ARRAY_PROBE: &str = r#"
function check(what, ok) {
  if (!ok) throw new Error(what);
}

// An array from the host: no `$u`, so `$list_push` must copy it.
const host = [1, 2, 3];
const grown = $list_push(host, null, 4);
check("a host array was written through", host.length === 3);
check("a host array's copy is wrong", grown.length === 4 && grown[3] === 4);
check("a host array was marked", !Object.prototype.hasOwnProperty.call(host, "$u"));

// The copy is ours, so the next push writes into it.
const again = $list_push(grown, null, 5);
check("the copy of a host array was copied again", again === grown);

// Marking a host array must write nothing a host can read by name onto it.
const before = Object.getOwnPropertyNames(host).join(",");
const json = JSON.stringify(host);
$share(host);
check("$share wrote onto a host array", Object.getOwnPropertyNames(host).join(",") === before);
check("$share changed a host array's JSON", JSON.stringify(host) === json);
check("a marked host array became writable", $list_push(host, null, 9) !== host);
check("a marked host array was written through", host.length === 3);

// A host object, the same.
const object = { a: 1 };
$share(object);
check("$share wrote onto a host object", Object.keys(object).join(",") === "a");
check("$share changed a host object's JSON", JSON.stringify(object) === '{"a":1}');

// An aggregate passes its mark to a field read out of it, and only then.
const unmarkedParent = [0, $list_push($list_empty(), null, 1)];
check(
  "a field of an unmarked aggregate was marked",
  $fromShared(unmarkedParent, unmarkedParent[1]).$u === true,
);
const markedParent = $share([0, $list_push($list_empty(), null, 1)]);
check(
  "a field of a marked aggregate was not marked",
  $fromShared(markedParent, markedParent[1]).$u === false,
);

// A frozen or sealed host value cannot take a property, and marking one must
// neither throw nor lose the mark.
for (const [name, seal] of [["frozen", Object.freeze], ["sealed", Object.seal]]) {
  const plain = seal([0, $list_push($list_empty(), null, 1)]);
  check(name + ": a field of an unmarked aggregate was marked", $fromShared(plain, plain[1]).$u === true);
  const kept = seal([0, $list_push($list_empty(), null, 1)]);
  check(name + ": $share answers its argument", $share(kept) === kept);
  check(name + ": a field of a marked aggregate was not marked", $fromShared(kept, kept[1]).$u === false);
  const frozenHost = seal([1, 2, 3]);
  $share(frozenHost);
  check(name + ": a marked host array became writable", $list_push(frozenHost, null, 4) !== frozenHost);
}

// Two names for one of ours: the mark is what separates them.
const ours = $list_empty();
const marked = $share(ours);
check("$share answers its argument", marked === ours);
const left = $list_push(ours, null, 1);
const right = $list_push(ours, null, 2);
check("a marked list was written through", ours.length === 0);
check("two pushes onto a marked list agree", left[0] === 1 && right[0] === 2);

// Sticky: nothing puts a list back.
$list_push(left, null, 9);
check("a copy that was marked came back unmarked", left.$u === true);
$share(left);
$share(left);
check("a mark was undone", left.$u === false);

// The other five all copy a host array and write into one of ours.
for (const [name, call] of [
  ["concat", (xs) => $list_concat(xs, null, [9])],
  ["reverse", (xs) => $list_reverse(xs, null)],
  ["take", (xs) => $list_take(xs, null, 2)],
  ["drop", (xs) => $list_drop(xs, null, 1)],
  ["slice", (xs) => $list_slice(xs, null, 0, 2)],
]) {
  const from_host = [1, 2, 3];
  const out = call(from_host);
  check(name + " wrote through a host array", from_host.length === 3);
  check(name + " wrote through a host array", from_host[0] === 1 && from_host[2] === 3);
  check(name + " did not take ownership of its answer", out.$u === true);
}

console.log("ok");
"#;

/// A host array is copied, never written to, and never marked by name.
#[test]
fn a_host_array_is_never_written_through() {
    let scratch = Scratch::empty("js-sharing-host-array");
    let probe = scratch.write(
        "probe.mjs",
        &format!("{}\n{HOST_ARRAY_PROBE}", buri::compiler::backend::js::runtime_source()),
    );
    let out = Command::new(js_runtime()).arg(&probe).output().expect("the javascript runtime runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.trim() == "ok",
        "the runtime's list surface does not hold the host boundary:\n{stdout}{stderr}"
    );
}

/// A list grown in a loop: `<runs>` runs of `<size>` pushes through each of
/// two shapes, printing how many elements that pushed.
const GROW: &str = r#"
from "core/env" import * as env;
from "platform/effect" import { Allocator, Environment, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/list" import * as list;

struct State { total: Int, items: [Int] }

fn build<C: Allocator>(ctx: C, i: Int, n: Int, acc: [Int]): [Int] {
  if (i >= n) { acc } else { build(ctx, i + 1, n, acc.push(ctx, i)) }
}

fn fold<C: Allocator>(ctx: C, i: Int, n: Int, s: State): State {
  if (i >= n) {
    s
  } else {
    fold(ctx, i + 1, n, State { ..s, total: s.total + i, items: s.items.push(ctx, i) })
  }
}

fn buildRuns<C: Allocator>(ctx: C, k: Int, runs: Int, n: Int, acc: Int): Int {
  if (k >= runs) {
    acc
  } else {
    buildRuns(ctx, k + 1, runs, n, acc + build(ctx, 0, n, list.empty<Int>()).length())
  }
}

fn foldRuns<C: Allocator>(ctx: C, k: Int, runs: Int, n: Int, acc: Int): Int {
  if (k >= runs) {
    acc
  } else {
    foldRuns(
      ctx,
      k + 1,
      runs,
      n,
      acc + fold(ctx, 0, n, State { total: 0, items: list.empty<Int>() }).items.length(),
    )
  }
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let runs = args.get(0).andThen(fn(s) => s.toInt()).withDefault(0);
  let n = args.get(1).andThen(fn(s) => s.toInt()).withDefault(0);
  let pushed = buildRuns(ctx, 0, runs, n, 0) + foldRuns(ctx, 0, runs, n, 0);
  io.println(ctx, "${pushed}").mapErr(fn(_e) => "stdout")
}
"#;

/// Both sizes push two hundred thousand elements: runs of a thousand and runs
/// of ten thousand. Linear growth makes them cost the same; the copy they
/// replaced makes each push of the larger copy ten times as much.
///
/// Counted rather than timed, so a loaded machine reads what an idle one does.
/// In instructions, linear growth scores 1.0 and the copy, with
/// `$list_push`'s in-place branch deleted, 8.7. The bound of 4 is
/// between them. The copies are counted too, which Linux can do: none, against
/// a billion.
#[test]
fn growing_a_list_in_a_loop_is_linear() {
    let scratch = Scratch::repo("js-sharing-linearity");
    scratch.write("cmd/grow/BUILD.buri", JS_BINARY);
    scratch.write("cmd/grow/main.buri", GROW);
    scratch.run(&["build", "//cmd/grow", "--force"]).ok();

    let small = added(&scratch, "cmd/grow", 100, 1_000);
    let large = added(&scratch, "cmd/grow", 10, 10_000);
    if let Some(ratio) = ratio(&small, &large) {
        assert!(
            ratio <= 4.0,
            "growing a list is not linear: two hundred thousand pushes cost {ratio:.1} \
             times as many instructions in runs of ten thousand as in runs of a \
             thousand, where linear growth scores 1.0 and copying 8.7"
        );
    }
    for (cost, size) in [(&small, "a thousand"), (&large, "ten thousand")] {
        assert!(
            cost.copied < 20_000,
            "two hundred thousand pushes in runs of {size} copied {} elements",
            cost.copied
        );
    }
}

/// The same list, grown in a loop, held in a record whose **other** field is
/// written in the same literal.
///
/// `raw` out of `core/buri/ast`'s printer, with the names changed: a record
/// carrying the pieces written so far and the offset the next one starts at,
/// and one functional update that pushes a piece and advances the offset,
/// run the way [`GROW`] runs.
///
/// `total` is what the shape is for. It is written beside the push, it is read
/// out of the same record, and what reading it produces is an `Int` rather
/// than a reference to anything.
const GROW_BESIDE: &str = r#"
from "core/env" import * as env;
from "platform/effect" import { Allocator, Environment, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/list" import * as list;

struct Out { items: [Int], total: Int }

fn write<C: Allocator>(ctx: C, i: Int, n: Int, out: Out): Out {
  if (i >= n) {
    out
  } else {
    write(ctx, i + 1, n, Out { ..out, items: out.items.push(ctx, i), total: out.total + i })
  }
}

fn writeRuns<C: Allocator>(ctx: C, k: Int, count: Int, n: Int, acc: Int): Int {
  if (k >= count) {
    acc
  } else {
    writeRuns(
      ctx,
      k + 1,
      count,
      n,
      acc + write(ctx, 0, n, Out { items: list.empty<Int>(), total: 0 }).items.length(),
    )
  }
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let runs = args.get(0).andThen(fn(s) => s.toInt()).withDefault(0);
  let n = args.get(1).andThen(fn(s) => s.toInt()).withDefault(0);
  io.println(ctx, "${writeRuns(ctx, 0, runs, n, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// A push into a record's list stays in place when the same literal writes a
/// field declared after it.
///
/// [`growing_a_list_in_a_loop_is_linear`] asserts the curve for the shape
/// where the list is the last field the literal writes. Move one `Int` field
/// past it — the same program, the same data, two words of the struct
/// declaration swapped — and the push used to copy. The analysis behind the
/// in-place write asks whether anything still reads the record after the push;
/// `out.total` answered yes; and a record whose list has another reader is a
/// record whose list has to be copied. Reading an `Int` out of a record is not
/// another reader of its list, which is what `middle/rc.rs`'s
/// `Scan::no_reference_path` says.
///
/// Counted the same way and against the same bound, because it is the same
/// claim about the same curve: linear growth scores 1.0 and copying
/// 8.0. `core/buri/ast`'s printer is what found it: every token it
/// writes goes through this shape, and printing a four-hundred-field message
/// took 56 seconds.
#[test]
fn growing_a_list_beside_another_field_is_linear() {
    let scratch = Scratch::repo("js-sharing-linearity-beside");
    scratch.write("cmd/grow/BUILD.buri", JS_BINARY);
    scratch.write("cmd/grow/main.buri", GROW_BESIDE);
    scratch.run(&["build", "//cmd/grow", "--force"]).ok();

    let small = added(&scratch, "cmd/grow", 200, 1_000);
    let large = added(&scratch, "cmd/grow", 20, 10_000);
    if let Some(ratio) = ratio(&small, &large) {
        assert!(
            ratio <= 4.0,
            "a push beside another field is not linear: two hundred thousand pushes \
             cost {ratio:.1} times as many instructions in runs of ten thousand as in \
             runs of a thousand, where linear growth scores 1.0 and copying \
             8.0"
        );
    }
    for (cost, size) in [(&small, "a thousand"), (&large, "ten thousand")] {
        assert!(
            cost.copied < 20_000,
            "two hundred thousand pushes beside another field in runs of {size} \
             copied {} elements",
            cost.copied
        );
    }
}

/// The printer the build runs for every `.proto` in a repository is linear in
/// the module it prints.
///
/// This is the claim the two above are worth having. `core/buri/ast`'s printer
/// threads an `Out` — the pieces written so far, and the offset the next one
/// starts at — through every function of the walk, and every token it writes
/// goes through `raw`, whose one functional update is the shape
/// [`growing_a_list_beside_another_field_is_linear`] is about. Printing a
/// four-hundred-field message took 56 seconds and an eight-hundred-field one
/// five minutes; it is 26 milliseconds for twenty-five thousand fields now.
///
/// Counted rather than timed, against the same bound: ten thousand fields
/// printed in modules of a hundred and in modules of two thousand. In
/// instructions, linear printing scores 1.5 and a printer whose `raw` copies
/// scores 12. The copies themselves are counted too, which Linux can do: a
/// printed field copies about one element, and the copying printer over a
/// billion.
#[test]
fn printing_a_module_is_linear_in_its_size() {
    let scratch = Scratch::repo("js-sharing-printer");
    scratch.write("cmd/print/BUILD.buri", JS_BINARY);
    scratch.write("cmd/print/main.buri", &PRINT.replace("SHAPE", "wide"));
    scratch.run(&["build", "//cmd/print", "--force"]).ok();

    let small = added(&scratch, "cmd/print", 100, 100);
    let large = added(&scratch, "cmd/print", 5, 2_000);
    for (cost, size) in [(&small, "a hundred"), (&large, "two thousand")] {
        assert!(
            cost.copied < 20_000,
            "printing ten thousand fields in modules of {size} copied {} elements",
            cost.copied
        );
    }
    if let Some(ratio) = ratio(&small, &large) {
        assert!(
            ratio <= 4.0,
            "the printer is not linear: ten thousand fields cost {ratio:.1} times as \
             many instructions printed in modules of two thousand as in modules of a \
             hundred, where linear printing scores 1.5 and copying 12"
        );
    }
}

/// `core/buri/ast`'s printer, over the module `SHAPE` builds `<size>` wide,
/// printed `<runs>` times. It prints how many characters that wrote.
///
/// The tree is built before the first print, and the same tree is printed
/// each time, so a second batch of prints is the printer's work alone.
const PRINT: &str = r#"
from "core/buri/ast" import * as ast;
from "core/env" import * as env;
from "platform/effect" import { Allocator, Environment, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

fn fields<C: Allocator>(ctx: C, i: Int, n: Int, acc: [ast.FieldDecl]): [ast.FieldDecl] {
  if (i >= n) {
    acc
  } else {
    fields(
      ctx,
      i + 1,
      n,
      acc.push(
        ctx,
        ast.FieldDecl {
          name: ast.Name { text: "field", origin: ast.nowhere() },
          ty: ast.Type {
            kind: .Named(ast.Name { text: "Int", origin: ast.nowhere() }, []),
            origin: ast.nowhere(),
          },
          exported: true,
          docs: [],
          origin: ast.origin("wide.proto", i, i + 1),
        },
      ),
    )
  }
}

/// `export struct Wide { export field: Int, ... }`, `n` fields wide.
fn wide<C: Allocator>(ctx: C, n: Int): ast.Module {
  ast.Module {
    items: [
      ast.Item {
        kind: .Struct(ast.StructDecl {
          name: ast.Name { text: "Wide", origin: ast.nowhere() },
          generics: [],
          body: .Record(fields(ctx, 0, n, list.empty<ast.FieldDecl>())),
          exported: true,
          docs: [],
        }),
        origin: ast.origin("wide.proto", 0, 4),
      },
    ],
    docs: [],
  }
}

/// `export struct S0 { export field: Int }` and `derive Equal, Show for S0;`,
/// and the same for each of `n` names: a schema of `n` messages. The derives
/// come after every struct, so each is printed above a declaration it names.
fn many<C: Allocator>(ctx: C, n: Int): ast.Module {
  let names = list.range(ctx, 0, n).mapCtx(ctx, fn(c, i) => str.format(c, "S${i}"));
  let structs = names.mapCtx(ctx, fn(c, name) => declared(c, name));
  let derives = names.map(ctx, fn(name) => derived(name));
  ast.Module { items: structs.concat(ctx, derives), docs: [] }
}

fn named(text: Str): ast.Type {
  ast.Type { kind: .Named(ast.Name { text: text, origin: ast.nowhere() }, []), origin: ast.nowhere() }
}

fn declared<C: Allocator>(ctx: C, name: Str): ast.Item {
  ast.Item {
    kind: .Struct(ast.StructDecl {
      name: ast.Name { text: name, origin: ast.nowhere() },
      generics: [],
      body: .Record(fields(ctx, 0, 1, list.empty<ast.FieldDecl>())),
      exported: true,
      docs: [],
    }),
    origin: ast.nowhere(),
  }
}

fn derived(name: Str): ast.Item {
  ast.Item {
    kind: .Derive(ast.DeriveDecl { traits: [named("Equal"), named("Show")], selfTy: named(name) }),
    origin: ast.nowhere(),
  }
}

fn runs<C: Allocator>(ctx: C, k: Int, count: Int, tree: ast.Module, acc: Int): Int {
  if (k >= count) {
    acc
  } else {
    runs(ctx, k + 1, count, tree, acc + ast.print(ctx, tree).text.length())
  }
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let args = env.arguments(ctx);
  let count = args.get(0).andThen(fn(s) => s.toInt()).withDefault(0);
  let size = args.get(1).andThen(fn(s) => s.toInt()).withDefault(0);
  let tree = SHAPE(ctx, size);
  io.println(ctx, "${runs(ctx, 0, count, tree, 0)}").mapErr(fn(_e) => "stdout")
}
"#;

/// The same printer over a module of many declarations, each with a `derive`
/// that names it: a schema of many messages rather than one wide one.
///
/// A `derive` is printed above the declaration it names. Finding it was a
/// search of every item for each `derive`, and then a pass over every item for
/// each declaration, so printing a module was quadratic in how many
/// declarations it held — most of the time `std/codegen/proto` spent on a
/// schema of a thousand messages.
///
/// Four thousand declarations printed in modules of fifty and in one module of
/// four thousand, counted in instructions: linear printing scores 1.1, and the
/// quadratic printer 16. There is no such count off macOS, and no copy to
/// count instead, so elsewhere this checks only that the module prints.
#[test]
fn printing_many_declarations_is_linear_in_their_number() {
    let scratch = Scratch::repo("js-sharing-printer-many");
    scratch.write("cmd/print/BUILD.buri", JS_BINARY);
    scratch.write("cmd/print/main.buri", &PRINT.replace("SHAPE", "many"));
    scratch.run(&["build", "//cmd/print", "--force"]).ok();

    let small = added(&scratch, "cmd/print", 80, 50);
    let large = added(&scratch, "cmd/print", 1, 4_000);
    if let Some(ratio) = ratio(&small, &large) {
        assert!(
            ratio <= 4.0,
            "the printer is not linear in declarations: four thousand declarations \
             cost {ratio:.1} times as many instructions printed as one module as in \
             modules of fifty, where linear printing scores 1.1 and quadratic 16"
        );
    }
}

/// What running an artifact cost: the elements every `Array.prototype.slice`
/// copied, and the instructions it retired where the kernel counts them.
struct Cost {
    copied: u64,
    instructions: Option<u64>,
}

/// What `runs` more runs of `size` cost `package`: its run with `2 × runs`
/// less its run with `runs`, so that starting the runtime and compiling the
/// code are on neither side. Each is the fewer instructions of two launches,
/// since a launch only ever gains noise.
///
/// A count rather than a time, so a loaded machine reads what an idle one
/// does. A clock was what made these flaky: under load one pair read 402 ms
/// against 20, and the next 10 against 105.
fn added(scratch: &Scratch, package: &str, runs: u64, size: u64) -> Cost {
    let cost = |runs: u64| {
        let args = [runs.to_string(), size.to_string()];
        let (first, said) = launched(scratch, package, &args);
        let (second, again) = launched(scratch, package, &args);
        assert_eq!((first.copied, &said), (second.copied, &again), "two launches of `{package} {args:?}` differ");
        let fewer = first.instructions.zip(second.instructions).map(|(a, b)| a.min(b));
        (Cost { copied: first.copied, instructions: fewer }, said)
    };
    let (once, said_once) = cost(runs);
    let (twice, said_twice) = cost(2 * runs);
    let wrote = |said: &str| said.trim().parse::<u64>().unwrap_or_else(|_| panic!("`{package}` printed {said:?}"));
    assert_eq!(wrote(&said_twice), 2 * wrote(&said_once), "`{package}`'s second batch did different work");
    Cost {
        copied: twice.copied.saturating_sub(once.copied),
        instructions: twice.instructions.zip(once.instructions).map(|(t, o)| t.saturating_sub(o)),
    }
}

/// How many times the instructions `small` cost `large` does, where the two
/// did the same number of elements in different sizes: about 1 for linear
/// work. `None` where nothing counts instructions.
fn ratio(small: &Cost, large: &Cost) -> Option<f64> {
    let (small, large) = (small.instructions?, large.instructions?);
    eprintln!("instructions: {small} small, {large} large");
    Some(large as f64 / small.max(1) as f64)
}

/// Runs a built artifact with every `Array.prototype.slice` counted, and
/// answers the elements those calls copied, beside what the program printed.
///
/// `$list_push`'s copy of a shared list is a `slice`, so this is the number
/// that grows with the square of a list's length when a loop copies it on
/// every push, and with its length when the loop writes in place. It is a
/// count rather than a time, so it reads the same on a loaded machine as on an
/// idle one. What is counted is what each call answers rather than the length
/// of the list it was called on, so a reader that slices one word out of a long
/// list of characters pays for the word and not for the list. The wrapper
/// replaces the method and then imports the artifact, which runs `main`; the
/// count is written synchronously on the way out, because an asynchronous write
/// to a pipe may not survive the exit.
fn copied_by_slice(scratch: &Scratch, package: &str) -> (u64, String) {
    let (cost, stdout) = launched(scratch, package, &[]);
    (cost.copied, stdout)
}

/// One launch of `package` with `args` under the wrapper [`copied_by_slice`]
/// describes, and what it printed.
///
/// JavaScriptCore compiles on the program's own thread here, so the
/// instructions don't depend on when a compiler thread finished.
fn launched(scratch: &Scratch, package: &str, args: &[String]) -> (Cost, String) {
    let artifact = scratch.artifact(package);
    let wrapper = scratch.write(
        "count-slices.mjs",
        r#"
import { writeSync } from "node:fs";
import { pathToFileURL } from "node:url";
let copied = 0;
const slice = Array.prototype.slice;
Array.prototype.slice = function (...args) {
  const out = slice.apply(this, args);
  copied += out.length;
  return out;
};
process.on("exit", () => writeSync(2, `copied=${copied}\n`));
// The program reads its own arguments after the artifact's path.
const [artifact] = process.argv.splice(2, 1);
if (typeof Bun !== "undefined" && Bun.argv !== process.argv) Bun.argv.splice(2, 1);
await import(pathToFileURL(artifact).href);
"#,
    );
    let what = format!("{} {} {} {}", js_runtime(), wrapper.display(), artifact.display(), args.join(" "));
    let child = Command::new(js_runtime())
        .arg(&wrapper)
        .arg(&artifact)
        .args(args)
        .env("BUN_JSC_useConcurrentJIT", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("`{what}` did not run: {e}"));
    let instructions = buri::profile::exited_instructions(child.id());
    let out = child.wait_with_output().unwrap_or_else(|e| panic!("`{what}` did not finish: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "`{what}` failed:\n{stdout}{stderr}");
    let copied = stderr
        .lines()
        .find_map(|l| l.strip_prefix("copied="))
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or_else(|| panic!("`{what}` reported no count:\n{stdout}{stderr}"));
    (Cost { copied, instructions }, stdout)
}

/// A fold whose record grows **two** lists in one functional update, and reads
/// a third field into one of them.
///
/// `prepare` out of `core/buri/ast`'s parser, with the names changed: each
/// token is pushed onto `tokens`, the doc lines waiting for it are pushed onto
/// `docs`, and `pending` starts again empty. Every field the update reads out
/// of `acc` is one it also replaces, so `acc` has no reader left afterwards and
/// neither list is shared. The analysis read the first projection as a second
/// reference, because `acc` was still read by the second one, and each push
/// copied its whole list: parsing a module was quadratic in its tokens.
const GROW_TWO: &str = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/list" import * as list;

struct Prep { tokens: [Int], docs: [[Int]], pending: [Int] }

fn prepare<C: Allocator>(ctx: C, raw: [Int]): Prep {
  raw.foldCtx(
    ctx,
    fn(c, acc, t) => {
      if (t % 4 == 0) {
        Prep { ..acc, pending: acc.pending.push(c, t) }
      } else {
        Prep {
          ..acc,
          tokens: acc.tokens.push(c, t),
          docs: acc.docs.push(c, acc.pending),
          pending: [],
        }
      }
    },
    Prep { tokens: [], docs: [], pending: [] },
  )
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let p = prepare(ctx, list.range(ctx, 0, 4000));
  let _ = io.println(ctx, "${p.tokens.length()} ${p.docs.length()}").ignore();
  .Ok(())
}
"#;

/// Four thousand elements folded into two lists copies a handful of elements,
/// not the several million a copy per push costs.
#[test]
fn growing_two_lists_in_one_update_copies_neither() {
    let scratch = Scratch::repo("js-sharing-two-lists");
    scratch.write("cmd/grow/BUILD.buri", JS_BINARY);
    scratch.write("cmd/grow/main.buri", GROW_TWO);
    scratch.run(&["build", "//cmd/grow", "--force"]).ok();

    let (copied, stdout) = copied_by_slice(&scratch, "cmd/grow");
    assert_eq!(stdout, "3000 3000\n");
    // A copy per push is about nine million elements here: three thousand
    // pushes onto each of two lists growing to three thousand. Writing in
    // place copies none of them; the bound leaves room for whatever a
    // runtime copies on its own account.
    assert!(
        copied < 40_000,
        "folding four thousand elements into two lists copied {copied} elements"
    );
}

/// A fold that pushes onto its accumulator a value read out of that same
/// accumulator: `acc.push(c, x + acc.last()...)`.
///
/// The second argument only reads `acc`, and it is evaluated in full before
/// the push takes the list. The analysis counted the read as a second
/// reference, so it shared `acc` for the push and every push copied the whole
/// list: the fold was quadratic in its length.
const PUSH_READING_ITSELF: &str = r#"
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/list" import * as list;

fn sums<C: Allocator>(ctx: C, raw: [Int]): [Int] {
  raw.foldCtx(ctx, fn(c, acc, t) => acc.push(c, t + acc.last().withDefault(0)), [])
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let s = sums(ctx, list.range(ctx, 0, 4000));
  let _ = io.println(ctx, "${s.length()} ${s.last().withDefault(0)}").ignore();
  .Ok(())
}
"#;

/// Four thousand pushes, each reading the list it pushes onto, copy a handful
/// of elements, not the eight million a copy per push costs.
#[test]
fn a_push_that_reads_its_own_list_copies_nothing() {
    let scratch = Scratch::repo("js-sharing-push-reads-itself");
    scratch.write("cmd/sums/BUILD.buri", JS_BINARY);
    scratch.write("cmd/sums/main.buri", PUSH_READING_ITSELF);
    scratch.run(&["build", "//cmd/sums", "--force"]).ok();

    let (copied, stdout) = copied_by_slice(&scratch, "cmd/sums");
    assert_eq!(stdout, "4000 7998000\n");
    assert!(
        copied < 40_000,
        "four thousand pushes onto a list each reading it copied {copied} elements"
    );
}

/// `core/buri/ast`'s lexer, run over a source of a few thousand tokens that
/// reaches every scanner: words, numbers in each base, strings, templates,
/// characters, every kind of comment, and punctuation.
///
/// The lexer threads one record through the scan and pushes each token onto a
/// list in it. A scanner that kept the record it started from alive beside the
/// one it advanced left two records holding the same list, so every push copied
/// the whole list so far: tokenizing was quadratic in the number of tokens.
const TOKENIZE: &str = r#"
from "core/buri/ast" import * as ast;
from "platform/effect" import { Allocator, Stdout };
from "node" import { NodeHost };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

fn line<C: Allocator>(ctx: C, i: Int): Str {
  let n = i.show(ctx);
  str.format(ctx, "/// doc ${n}\nlet x${n} = f(${n}, 0x1f, 2.5e3, \"s\\n\", \"a\${n}b\${n}c\", 'c', '\\u{41}') <= y; // ${n}\n/* a /* ${n} */ b */\n")
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let source = list.range(ctx, 0, 250).mapCtx(ctx, fn(c, i) => line(c, i)).join(ctx, "");
  let tokens = ast.tokenize(ctx, source);
  let _ = io.println(ctx, tokens.length().show(ctx)).ignore();
  .Ok(())
}
"#;

/// Tokenizing about seven thousand tokens copies a few of them, not the tens of
/// millions a copy per push costs.
#[test]
fn tokenizing_a_source_copies_no_token_list() {
    let scratch = Scratch::repo("js-sharing-tokenize");
    scratch.write("cmd/lex/BUILD.buri", JS_BINARY);
    scratch.write("cmd/lex/main.buri", TOKENIZE);
    scratch.run(&["build", "//cmd/lex", "--force"]).ok();

    let (copied, stdout) = copied_by_slice(&scratch, "cmd/lex");
    assert_eq!(stdout, "7250\n");
    // A copy per push is about twenty-five million elements here: seven
    // thousand pushes onto a list growing to seven thousand. Writing in place
    // copies none of them; the bound leaves room for whatever a runtime copies
    // on its own account.
    assert!(
        copied < 70_000,
        "tokenizing seven thousand tokens copied {copied} elements"
    );
}

/// `std/codegen/proto/schema` and `std/textproto/read`, each run over an
/// ordinary document of about ten thousand characters.
///
/// Both readers start by recording the byte offset of every character, folding
/// over the characters and pushing each offset onto a list. The step read the
/// list's last offset and pushed onto the same list, and that read is a second
/// reference: every push copied the list so far. Reading a schema was
/// quadratic in its size before any of it was parsed, which is what made a
/// hostile schema of ten thousand nested messages take two minutes to refuse.
const READ_DOCUMENTS: &str = r#"
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "node" import { NodeHost };
from "platform/effect" import { Allocator, Stdout };
from "std/codegen/proto/schema" import * as schema;
from "std/textproto/read" import * as textproto;

fn message<C: Allocator>(ctx: C, i: Int): Str {
  let n = i.show(ctx);
  str.format(ctx, "message M${n} {\n  string name = 1;\n  int32 count = 2;\n  message Inner { repeated string tags = 1; }\n  Inner inner = 3;\n}\n")
}

fn entry<C: Allocator>(ctx: C, i: Int): Str {
  let n = i.show(ctx);
  str.format(ctx, "name: \"m${n}\"\ncount: ${n}\n")
}

export fn main(host: NodeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let messages = list.range(ctx, 0, 100).mapCtx(ctx, fn(c, i) => message(c, i)).join(ctx, "");
  let proto = str.format(ctx, "edition = \"2026\";\npackage p;\n${messages}");
  let parsed = schema.parse(ctx, proto, "lib/p/p.proto");
  let text = list.range(ctx, 0, 400).mapCtx(ctx, fn(c, i) => entry(c, i)).join(ctx, "");
  let fields = match (textproto.parse(ctx, text, "lib/p/p.txtpb")) {
    .Ok(doc) => doc.fields.length(),
    .Err(_) => -1,
  };
  let said = parsed.errors.length();
  let read = parsed.schema.messages.length();
  let _ = io.println(ctx, "${read} ${said} ${fields}").ignore();
  .Ok(())
}
"#;

/// Reading two ten-thousand-character documents copies about as many
/// elements as they have characters, not the tens of millions a copy per
/// character costs.
#[test]
fn reading_a_schema_copies_no_offset_list() {
    let scratch = Scratch::repo("js-sharing-schema-offsets");
    scratch.write("cmd/read/BUILD.buri", JS_BINARY);
    scratch.write("cmd/read/main.buri", READ_DOCUMENTS);
    scratch.run(&["build", "//cmd/read", "--force"]).ok();

    let (copied, stdout) = copied_by_slice(&scratch, "cmd/read");
    assert_eq!(stdout, "100 0 800\n");
    // A copy per character is about a hundred million elements here: two lists
    // of offsets growing to about eleven and ten thousand. Writing in place
    // copies none of them; the bound leaves room for the words each reader
    // slices out of its characters, which add up to less than the documents.
    assert!(
        copied < 100_000,
        "reading two ten-thousand-character documents copied {copied} elements"
    );
}
