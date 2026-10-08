//! A type's own `Show` and `Equal` where a value used to be read by its shape:
//! a failing assertion's report, and a signal deciding whether a write changed
//! anything. Same root cause as #258.
//!
//! Each run is a real `buri test` over a repository, under the heap check, on
//! JavaScript and on every native backend this toolchain has built in.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace(name: &str) -> PathBuf {
    crate::sweep::once();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-hand-written-impls-{}", std::process::id()))
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(at: &Path, body: &str) {
    if let Some(dir) = at.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(at, body).unwrap();
}

/// A repository holding one library, `//lib`, whose test source is `test`.
fn repository(name: &str, test: &str) -> PathBuf {
    let repo = workspace(name);
    write(&repo.join("REPO.buri"), "");
    write(
        &repo.join("lib/BUILD.buri"),
        "library {\n    sources: [\"unused.buri\"]\n\n    test {\n        \
         sources: [\"test/lib_test.buri\"]\n    }\n}\n",
    );
    write(&repo.join("lib/lib.buri"), "from \"//lib/unused.buri\" export { unused };\n");
    write(&repo.join("lib/unused.buri"), "export fn unused(): Int {\n    0\n}\n");
    write(&repo.join("lib/test/lib_test.buri"), test);
    repo
}

/// `buri test //lib` on every backend, as `(backend, exit status, stdout,
/// stderr)`.
fn every_backend(repo: &Path) -> Vec<(&'static str, i32, String, String)> {
    let mut modes = crate::e2e::build_modes();
    modes.push(("js", &["--output=js"]));
    modes
        .into_iter()
        .map(|(backend, flags)| {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
            cmd.current_dir(repo).arg("test").args(flags).arg("//lib").arg("--force");
            let ran = crate::shared::ran_command(crate::shared::heap_checked(&mut cmd));
            (backend, ran.status, ran.stdout, ran.stderr)
        })
        .collect()
}

const REPORTS: &str = r#"from "core/testing/assert" import * as assert;
from "core/testing/check" import * as check;
from "platform/effect" import { Allocator };

struct Secret {
    value: Str,
}

impl Show for Secret {
    fn show<C: Allocator>(self, ctx: C): Str {
        "***"
    }
}

derive Equal for Secret;

struct Login {
    user: Str,
    secret: Secret,
}

derive Equal, Show for Login;

// No `Show` at all, so the report walks it and still reaches `Secret`'s.
struct Bare {
    secret: Secret,
    pair: (Int, Secret),
}

derive Equal for Bare;

// Equal by label alone.
struct Tag {
    label: Str,
    noise: Int,
}

impl Equal for Tag {
    fn equal(self, other: Tag): Bool {
        self.label == other.label
    }
}

struct Item {
    tag: Tag,
    count: Int,
}

derive Equal for Item;

fn one(): Secret {
    Secret { value: "hunter2" }
}

fn two(): Secret {
    Secret { value: "swordfish" }
}

test "a derived Show" {
    assert.equal(Login { user: "ann", secret: one() }, Login { user: "ann", secret: two() });
}

test "no Show" {
    assert.equal(Bare { secret: one(), pair: (1, one()) }, Bare { secret: two(), pair: (1, two()) });
}

test "a list" {
    assert.equal([one()], []);
}

test "contains" {
    assert.contains([one()], two());
}

test "ok" {
    let r: Result<Int, Secret> = .Err(one());
    let _ = assert.ok(r);
}

test "none" {
    assert.none(.Some(one()));
}

test "a property's counterexample" {
    check.forAll(fn(g) => (one(), g), fn(_s) => false);
}

test "a hand-written Equal decides" {
    assert.equal(Tag { label: "a", noise: 1 }, Tag { label: "a", noise: 2 });
    assert.notEqual(Tag { label: "a", noise: 1 }, Tag { label: "b", noise: 1 });
    assert.equal(Item { tag: Tag { label: "a", noise: 1 }, count: 3 }, Item { tag: Tag { label: "a", noise: 2 }, count: 3 });
    assert.contains([Tag { label: "a", noise: 1 }], Tag { label: "a", noise: 9 });
}
"#;

/// A failing assertion renders both values through their `Show`, hand-written
/// or derived, and a type with no `Show` still reaches a field's own. The
/// comparison goes through the type's `Equal`.
#[test]
fn a_failing_assert_renders_values_through_their_show_on_every_backend() {
    let repo = repository("reports", REPORTS);
    let expected = [
        "    actual:   Login { user: \"ann\", secret: *** }\n    expected: Login { user: \"ann\", secret: *** }\n",
        "    actual:   Bare { secret: ***, pair: (1, ***) }\n    expected: Bare { secret: ***, pair: (1, ***) }\n",
        "    actual:   [***]\n    expected: []\n",
        "  assert.contains failed\n    actual:   [***]\n    expected: [***]\n",
        "  assert.ok failed\n    actual:   ***\n",
        "  assert.none failed\n    actual:   .Some(***)\n",
        ", case: *** })\n",
    ];
    for (backend, status, stdout, stderr) in every_backend(&repo) {
        let context = format!("{backend}:\n{stdout}\n{stderr}");
        assert_eq!(status, 1, "{context}");
        assert!(stdout.contains("1 passed, 7 failed"), "{context}");
        assert!(!stdout.contains("hunter2") && !stdout.contains("swordfish"), "{context}");
        for line in expected {
            assert!(stdout.contains(line), "{backend}: missing\n{line}\nin\n{stdout}\n{stderr}");
        }
    }
}

const SIGNALS: &str = r#"from "core/testing/assert" import * as assert;
from "platform/effect" import { Ui, Watch };
from "platform/effect/testing" import { headless, observer, recorder };
from "ui/signal" import { signal, watch };

// Equal by label alone.
struct Tag {
    label: Str,
    noise: Int,
}

impl Equal for Tag {
    fn equal(self, other: Tag): Bool {
        self.label == other.label
    }
}

// Never equal, not even to itself.
struct Never {
    value: Int,
}

impl Equal for Never {
    fn equal(self, other: Never): Bool {
        false
    }
}

struct Item {
    tag: Tag,
    count: Int,
}

derive Equal for Item;

// No `Equal` at all.
struct Loose {
    tag: Tag,
}

struct Plain {
    value: Int,
}

// A function has no shape, so this keeps the comparison it always had.
struct Held {
    tag: Tag,
    run: fn(Int) => Int,
}

enum Tree {
    Leaf(Tag),
    Node([Tree]),
}

test "a write its Equal calls equal fires nothing" {
    let ctx = context { Ui: headless(), Watch: observer() };
    let cell = signal(ctx, Tag { label: "a", noise: 1 });
    let seen = recorder();
    watch(ctx, fn(s) => {
        let _ = seen.note(cell.get(s).noise);
    });
    cell.set(ctx, Tag { label: "a", noise: 2 });
    assert.equal(seen.noted(), [1]);
    cell.set(ctx, Tag { label: "b", noise: 3 });
    assert.equal(seen.noted(), [1, 3]);
}

test "a write its Equal calls different fires" {
    let ctx = context { Ui: headless(), Watch: observer() };
    let cell = signal(ctx, Never { value: 1 });
    let seen = recorder();
    watch(ctx, fn(s) => {
        let _ = seen.note(cell.get(s).value);
    });
    cell.set(ctx, Never { value: 1 });
    assert.equal(seen.noted(), [1, 1]);
}

test "a derived Equal over a hand-written one" {
    let ctx = context { Ui: headless(), Watch: observer() };
    let cell = signal(ctx, Item { tag: Tag { label: "a", noise: 1 }, count: 1 });
    let seen = recorder();
    watch(ctx, fn(s) => {
        let _ = seen.note(cell.get(s).tag.noise);
    });
    cell.set(ctx, Item { tag: Tag { label: "a", noise: 2 }, count: 1 });
    cell.set(ctx, Item { tag: Tag { label: "b", noise: 3 }, count: 1 });
    assert.equal(seen.noted(), [1, 3]);
}

test "a list and a tuple" {
    let ctx = context { Ui: headless(), Watch: observer() };
    let list = signal(ctx, [Tag { label: "a", noise: 1 }]);
    let pair = signal(ctx, (Tag { label: "a", noise: 1 }, 1));
    let seen = recorder();
    watch(ctx, fn(s) => {
        let _ = seen.note(list.get(s).length() + pair.get(s).1);
    });
    list.set(ctx, [Tag { label: "a", noise: 2 }]);
    pair.set(ctx, (Tag { label: "a", noise: 2 }, 1));
    assert.equal(seen.noted(), [2]);
    pair.set(ctx, (Tag { label: "a", noise: 2 }, 5));
    assert.equal(seen.noted(), [2, 6]);
}

test "a type with no Equal is compared field by field" {
    let ctx = context { Ui: headless(), Watch: observer() };
    let loose = signal(ctx, Loose { tag: Tag { label: "a", noise: 1 } });
    let plain = signal(ctx, Plain { value: 1 });
    let seen = recorder();
    watch(ctx, fn(s) => {
        let _ = seen.note(loose.get(s).tag.noise + plain.get(s).value);
    });
    loose.set(ctx, Loose { tag: Tag { label: "a", noise: 2 } });
    plain.set(ctx, Plain { value: 1 });
    assert.equal(seen.noted(), [2]);
    plain.set(ctx, Plain { value: 4 });
    assert.equal(seen.noted(), [2, 5]);
}

test "a recursive type with no Equal" {
    let ctx = context { Ui: headless(), Watch: observer() };
    let cell = signal(ctx, Tree.Node([.Leaf(Tag { label: "a", noise: 1 })]));
    let seen = recorder();
    watch(ctx, fn(s) => {
        let _ = cell.get(s);
        let _ = seen.note(1);
    });
    cell.set(ctx, Tree.Node([.Leaf(Tag { label: "a", noise: 7 })]));
    assert.equal(seen.noted(), [1]);
    cell.set(ctx, Tree.Node([.Leaf(Tag { label: "b", noise: 7 }), .Node([])]));
    assert.equal(seen.noted(), [1, 1]);
}

test "a value holding a function" {
    let ctx = context { Ui: headless(), Watch: observer() };
    let f = fn(x: Int) => x + 1;
    let cell = signal(ctx, Held { tag: Tag { label: "a", noise: 1 }, run: f });
    let seen = recorder();
    watch(ctx, fn(s) => {
        let _ = seen.note(cell.get(s).tag.noise);
    });
    cell.set(ctx, Held { tag: Tag { label: "b", noise: 2 }, run: f });
    assert.equal(seen.noted(), [1, 2]);
}
"#;

/// A signal decides whether a write changed anything with the type's `Equal`,
/// hand-written or derived. A type with no `Equal` is compared field by field,
/// as before, and a field's own `Equal` still decides that field.
#[test]
fn a_signal_compares_with_the_types_equal_on_every_backend() {
    let repo = repository("signals", SIGNALS);
    for (backend, status, stdout, stderr) in every_backend(&repo) {
        assert!(
            status == 0 && stdout.contains("7 passed, 0 failed"),
            "{backend}:\n{stdout}\n{stderr}"
        );
    }
}

const MAPS: &str = r#"from "core/testing/assert" import * as assert;
from "core/orderedmap" import * as ordmap;
from "core/orderedmap" import { OrderedMap };
from "platform/effect" import { Allocator, Ui, Watch };
from "platform/effect/testing" import { alloc, headless, observer, recorder };
from "ui/signal" import { signal, watch };

// No `Show` and no `Equal`, so neither of the map's own applies to a map of it.
struct Item {
    weight: Int,
}

struct Bag {
    items: OrderedMap<Int, Item>,
}

struct Secret {
    value: Str,
}

impl Show for Secret {
    fn show<C: Allocator>(self, ctx: C): Str {
        "***"
    }
}

struct Vault {
    secrets: OrderedMap<Int, Secret>,
}

fn weight(bag: Bag, key: Int): Int {
    bag.items.get(key).map(fn(item) => item.weight).withDefault(0)
}

test "some unwraps a bag" {
    let held: Option<Bag> = .Some(Bag { items: ordmap.empty() });
    assert.equal(weight(assert.some(held), 1), 0);
}

test "a bag in a signal" {
    let ctx = context { Ui: headless(), Watch: observer() };
    let cell = signal(ctx, Bag { items: ordmap.empty() });
    let seen = recorder();
    watch(ctx, fn(s) => {
        let _ = seen.note(weight(cell.get(s), 1));
    });
    cell.set(ctx, Bag { items: ordmap.empty().insert(alloc(), 1, Item { weight: 4 }) });
    assert.equal(seen.noted(), [0, 4]);
}

test "a map whose values show renders through the map's Show" {
    assert.none(.Some(Vault { secrets: ordmap.empty().insert(alloc(), 1, Secret { value: "hunter2" }) }));
}
"#;

/// A map's `Show` and `Equal` need its values' (#269). Where the values have
/// none, a report or a signal walks the map as it walks any type with no
/// `impl`, rather than calling one that does not apply. Where they have one,
/// the map's own renders it.
#[test]
fn a_map_whose_values_lack_show_or_equal_is_walked_on_every_backend() {
    let repo = repository("maps", MAPS);
    for (backend, status, stdout, stderr) in every_backend(&repo) {
        let context = format!("{backend}:\n{stdout}\n{stderr}");
        assert_eq!(status, 1, "{context}");
        assert!(stdout.contains("2 passed, 1 failed"), "{context}");
        assert!(stdout.contains("actual:   .Some(Vault { secrets: {1: ***} })\n"), "{context}");
        assert!(!stdout.contains("hunter2"), "{context}");
    }
}

const UNMET_BOUNDS: &str = r#"from "core/actor" import { Address };
from "core/orderedmap" import * as ordmap;
from "core/orderedmap" import { OrderedMap };
from "core/testing/assert" import * as assert;
from "platform/effect" import { Ui, Watch };
from "platform/effect/testing" import { alloc, headless, observer, recorder };
from "ui/signal" import { signal, watch };

derive Equal, Show for Wrap;
struct Wrap<T> {
    weight: Int,
    inner: T,
}

// No `Show` or `Equal`, and an address holds functions.
struct Plain<C> {
    to: Address<C, Int, Int, Int>,
}

struct Bag<C> {
    items: OrderedMap<Int, Wrap<Plain<C>>>,
}

fn empty<C>(): Bag<C> {
    Bag { items: ordmap.empty() }
}

fn weight<C>(bag: Bag<C>, key: Int): Int {
    bag.items.get(key).map(fn(item) => item.weight).withDefault(0)
}

fn weighOne<C>(ctx: C): Int {
    let bag: Option<Bag<C>> = .Some(empty());
    weight(assert.some(bag), 1)
}

// No `Show` either, and nothing in it is a function.
struct Count {
    n: Int,
}

struct Tally {
    counts: OrderedMap<Str, Count>,
}

test "a bag of addresses weighs nothing" {
    assert.equal(weighOne(alloc()), 0);
}

test "a signal of addresses" {
    let ctx = context { Ui: headless(), Watch: observer() };
    let cell = signal(ctx, empty());
    let seen = recorder();
    watch(ctx, fn(s) => {
        let _ = seen.note(weight(cell.get(s), 1));
    });
    cell.set(ctx, empty());
    assert.equal(seen.noted().length() > 0, true);
}

test "a map whose values have no Show" {
    let tally: Option<Tally> = .None;
    let _ = assert.some(tally);
}

test "a map of them that fails" {
    let tally = Tally { counts: ordmap.empty().insert(alloc(), "a", Count { n: 1 }) };
    assert.none(.Some(tally));
}
"#;

/// A failure report and a signal reach a hand-written `impl` whose bounds the
/// type does not meet, such as `OrderedMap`'s `Show` over values with none.
/// Each compiles on every backend and agrees on what it prints (#270).
#[test]
fn a_hand_written_impl_whose_bounds_fail_is_walked_by_shape_on_every_backend() {
    let repo = repository("unmet-bounds", UNMET_BOUNDS);
    let mut printed = Vec::new();
    for (backend, status, stdout, stderr) in every_backend(&repo) {
        let context = format!("{backend}:\n{stdout}\n{stderr}");
        assert_eq!(status, 1, "{context}");
        assert!(stdout.contains("2 passed, 2 failed"), "{context}");
        assert!(stdout.contains("assert.some failed\n    actual:   .None\n"), "{context}");
        let failure = stdout.split("assert.none failed").nth(1).unwrap_or_default();
        let shown = failure.lines().nth(1).unwrap_or_default().to_string();
        assert!(shown.contains("Count { n: 1 }"), "{context}");
        printed.push((backend, shown));
    }
    for (backend, shown) in &printed {
        assert_eq!(shown, &printed[0].1, "{backend} disagrees with {}", printed[0].0);
    }
}
