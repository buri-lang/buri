//! `semantics::resolve` on whole snippets, through the real front end. Here
//! rather than beside the pass because a snippet compiles through `driver`,
//! which is in `buri`.

use crate::compiler::semantics::types::*;
use crate::diagnostics::SourceMap;

/// One snippet through the real front end, with its `Tables`.
fn tables_of(src: &str) -> Tables {
    let mut map = SourceMap::new();
    let analysis = crate::compiler::driver::analyze_snippet(
        &mut map,
        "resolve_test.buri",
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
    analysis.checked.tables
}

/// The method named `name` on the type named `ty`.
fn method<'t>(tables: &'t Tables, ty: &str, name: &str) -> &'t FnInfo {
    let con =
        tables.tycons.iter().position(|c| c.name == ty).expect("the snippet declares the type");
    tables
        .fns
        .iter()
        .find(|f| f.name == name && f.self_ty.map(TyConId::index) == Some(con))
        .expect("the snippet declares the method")
}

const RELAY: &str = r#"
struct Wrap { value: Int }

trait Relay {
  fn relay(self, done: fn(Self, Int) => Int): Int;
}

impl Relay for Wrap {
  fn relay(self, done: fn(Self, Int) => Int): Int {
done(self, self.value)
  }
}

fn main(): () {}
"#;

/// A written `Self` in an `impl` method's parameter is the `impl` head's
/// type by the time it reaches `FnInfo.params`. Left as `Ty::SelfTy` it is
/// substituted by nothing downstream — `middle::monomorphize` passes `None`
/// for the self type at every `substitute` — and reaches `middle::layout`
/// as a type it has no size for.
#[test]
fn a_written_self_in_an_impl_method_parameter_is_the_impl_heads_type() {
    let tables = tables_of(RELAY);
    let f = method(&tables, "Wrap", "relay");
    let TyKind::Fn(params, _) = f.params[1].ty.kind() else {
        panic!("the parameter is a function type: {:?}", f.params[1].ty)
    };
    let wrap = tables.tycons.iter().position(|c| c.name == "Wrap").unwrap_or_default();
    assert!(
        matches!(params[0].kind(), TyKind::Con(id, args) if id.index() == wrap && args.is_empty()),
        "`Self` stayed unresolved in an `impl` method's parameter: {:?}",
        params[0],
    );
}

/// The implicit type of a `self` parameter and a written `Self` come from
/// the same scope, so the two spellings of the receiver agree.
#[test]
fn the_self_parameter_and_a_written_self_agree() {
    let tables = tables_of(RELAY);
    let f = method(&tables, "Wrap", "relay");
    let TyKind::Fn(params, _) = f.params[1].ty.kind() else { panic!("a function type") };
    assert_eq!(f.params[0].ty, params[0]);
    assert_eq!(f.params[0].role, ParamRole::SelfParam);
}

/// A `trait`'s own signature is the other half: there is no implementing
/// type yet, so `Self` stays abstract and an `impl` is what supplies one.
#[test]
fn a_written_self_in_a_trait_signature_stays_abstract() {
    let tables = tables_of(RELAY);
    let tr = tables.traits.iter().find(|t| t.name == "Relay").expect("the trait");
    let m = tr.methods.first().expect("the method");
    assert_eq!(m.params[0].ty, Ty::SELF);
    let TyKind::Fn(params, _) = m.params[1].ty.kind() else { panic!("a function type") };
    assert_eq!(params[0], Ty::SELF);
}

/// The head's own type parameters are in scope in what `Self` expands to,
/// so a generic `impl` gets `Crate<T>` and not a bare constructor — in the
/// return type as well as in a parameter.
#[test]
fn a_written_self_carries_the_impl_heads_type_arguments() {
    let tables = tables_of(
        r#"
struct Crate<T> { value: T }

trait Copyable {
  fn copy(self, other: Self): Self;
}

impl<T> Copyable for Crate<T> {
  fn copy(self, other: Self): Self {
other
  }
}

fn main(): () {}
"#,
    );
    let f = method(&tables, "Crate", "copy");
    let head = |ty: &Ty| matches!(ty.kind(), TyKind::Con(_, args) if **args == [Ty::param(0)]);
    assert!(head(&f.params[1].ty), "a written `Self` parameter: {:?}", f.params[1].ty);
    assert!(head(&f.ret), "a written `Self` return: {:?}", f.ret);
}

/// An inherent `impl` resolves `Self` the same way a trait `impl` does:
/// the two share `register_impl`'s scope and differ only in what they
/// register.
#[test]
fn an_inherent_impl_resolves_a_written_self_too() {
    let tables = tables_of(
        r#"
struct Knob { value: Int }

impl Knob {
  fn pick(self, other: Self): Self {
if (self.value > other.value) { self } else { other }
  }
}

fn main(): () {}
"#,
    );
    let f = method(&tables, "Knob", "pick");
    assert_eq!(f.params[1].ty, f.params[0].ty);
    assert_eq!(f.ret, f.params[0].ty);
    assert!(!matches!(f.params[1].ty.kind(), TyKind::SelfTy));
}

// -----------------------------------------------------------------------
// Rule 26 inside a `trait` body and an `impl` block
// -----------------------------------------------------------------------

/// How many times a snippet reports one diagnostic code.
///
/// The count rather than the presence: what this covers is a set of
/// signatures nobody read, so a test that merely saw *a* diagnostic would
/// pass on the one signature that was already being checked.
fn reported(src: &str, code: &str) -> usize {
    let mut map = SourceMap::new();
    let analysis = crate::compiler::driver::analyze_snippet(
        &mut map,
        "resolve_test.buri",
        src,
        crate::compiler::modules::Role::Entry,
    );
    analysis
        .diagnostics
        .items
        .iter()
        .filter(|d| d.code.as_deref() == Some(code))
        .count()
}

/// A `trait` declaration, the `impl` that supplies it, and one of the
/// type's own methods — the three shapes a method signature comes in, each
/// taking a context under a name that is not `ctx`.
const SINKS: &str = r#"
from "platform/effect" import { Stdout };

struct Ledger { lines: Int }

trait Report {
  fn write<C: Stdout>(self, sink: C): ();
}

impl Report for Ledger {
  fn write<C: Stdout>(self, sink: C): () {
()
  }
}

impl Ledger {
  fn dump<C: Stdout>(self, sink: C): () {
()
  }
}

fn main(): () {}
"#;

/// All three, not none. `pending_ctx_rules` used to be pushed from the
/// `Item::Fn` path alone, so a method — which is where a context is most
/// often taken — was exempt from the rule its own callers read.
#[test]
fn an_effect_carrying_parameter_is_refused_in_every_kind_of_method() {
    assert_eq!(reported(SINKS, "effect-parameter-not-ctx"), 3);
}

/// The position half of the rule reaches a method too: receiver first,
/// context second, everything else after.
#[test]
fn a_ctx_out_of_position_is_refused_in_every_kind_of_method() {
    let src = r#"
from "platform/effect" import { Stdout };

struct Bell { tone: Str }

trait Ring {
  fn ring<C: Stdout>(self, times: Int, ctx: C): ();
}

impl Ring for Bell {
  fn ring<C: Stdout>(self, times: Int, ctx: C): () {
()
  }
}

impl Bell {
  fn peal<C: Stdout>(self, times: Int, ctx: C): () {
()
  }
}

fn main(): () {}
"#;
    assert_eq!(reported(src, "ctx-not-first"), 3);
}

/// A misplaced receiver, in the two places one survives to rule 26: an
/// inherent `impl` refuses a method that does not open with `self` before
/// it is ever registered — that is `impl-missing-self` — so what is
/// left is a `trait` body and the `impl` that supplies it.
#[test]
fn a_misplaced_self_is_refused_in_a_trait_body_and_in_its_impl() {
    let src = r#"
struct Square { side: Int }

trait Scale {
  fn scaled(factor: Int, self): Int;
}

impl Scale for Square {
  fn scaled(factor: Int, self): Int {
self.side * factor
  }
}

fn main(): () {}
"#;
    // The parser reports the same code at each site on its own, so the
    // semantic half is the second of each pair.
    assert_eq!(reported(src, "self-not-first"), 4);
}

/// A legitimate `ctx` does not license a second capability beside it: the
/// loop reads on past the first two parameters, in a method as anywhere.
#[test]
fn an_effect_carrying_parameter_after_a_ctx_is_still_refused() {
    let src = r#"
from "platform/effect" import { Stdout };

struct Twice { n: Int }

impl Twice {
  fn both<C: Stdout, D: Stdout>(self, ctx: C, also: D): () {
()
  }
}

fn main(): () {}
"#;
    assert_eq!(reported(src, "effect-parameter-not-ctx"), 1);
}

/// The other direction, which is what keeps this from being a rule against
/// methods: one that follows the convention says nothing at all.
#[test]
fn a_method_that_follows_the_convention_is_admitted() {
    let src = r#"
from "platform/effect" import { Stdout };

struct Quiet { n: Int }

trait Speak {
  fn speak<C: Stdout>(self, ctx: C, times: Int): ();
}

impl Speak for Quiet {
  fn speak<C: Stdout>(self, ctx: C, times: Int): () {
()
  }
}

impl Quiet {
  fn hush<C: Stdout>(self, ctx: C): () {
()
  }
}

fn main(): () {}
"#;
    for code in ["effect-parameter-not-ctx", "ctx-not-first", "self-not-first"] {
        assert_eq!(reported(src, code), 0, "a well-formed method reported `{code}`");
    }
}

/// End to end, and the direction that matters most: an `impl` that agrees
/// with its trait says nothing. This is the shape the whole standard
/// library is written in.
#[test]
fn an_impl_that_agrees_with_its_trait_is_admitted() {
    let src = r#"
struct Knob { value: Int }

trait Pick {
  fn pick(self, other: Self): Self;
  fn label<T>(self, tag: T): Int;
}

impl Pick for Knob {
  fn pick(self, other: Knob): Knob {
if (self.value > other.value) { self } else { other }
  }
  fn label<T>(self, tag: T): Int { self.value }
}

fn main(): () {}
"#;
    assert_eq!(reported(src, "impl-signature-mismatch"), 0);
}

/// And the same `impl` written with `Self` throughout, which A1 made
/// legal: `Self` and the head type are the same type inside the block, so
/// the comparison must accept either spelling.
#[test]
fn an_impl_that_writes_self_for_the_head_type_is_admitted() {
    let src = r#"
struct Knob { value: Int }

trait Pick {
  fn pick(self, other: Self): Self;
}

impl Pick for Knob {
  fn pick(self, other: Self): Self {
if (self.value > other.value) { self } else { other }
  }
}

fn main(): () {}
"#;
    assert_eq!(reported(src, "impl-signature-mismatch"), 0);
}

/// End to end in the other direction: three methods, one disagreement
/// each, one report each.
#[test]
fn every_half_of_a_signature_is_compared() {
    let src = r#"
struct Knob { value: Int }

trait Pick {
  fn pick(self, other: Self): Self;
  fn count(self): Int;
  fn label<T>(self, tag: T): Int;
}

impl Pick for Knob {
  fn pick(self, other: Int): Self { self }
  fn count(self): Str { "one" }
  fn label(self, tag: Int): Int { self.value }
}

fn main(): () {}
"#;
    assert_eq!(reported(src, "impl-signature-mismatch"), 3);
}

/// And `Self` outside both is still the mistake it was: the scope is
/// entered and left around one declaration, so the next one in the same
/// module does not inherit it.
#[test]
fn self_outside_an_impl_is_still_refused() {
    let mut map = SourceMap::new();
    let analysis = crate::compiler::driver::analyze_snippet(
        &mut map,
        "resolve_test.buri",
        r#"
struct Knob { value: Int }

impl Knob {
  fn pick(self): Self {
self
  }
}

fn free(x: Int): Self { x }

fn main(): () {}
"#,
        crate::compiler::modules::Role::Entry,
    );
    assert!(
        analysis
            .diagnostics
            .items
            .iter()
            .any(|d| d.code.as_deref() == Some("self-type-outside-impl")),
        "a free function's `Self` was admitted",
    );
}

/// A `context` declaration built from one written *after* it.
///
/// `..Base()` reads the type checking recorded for `Base`, and the
/// declarations used to be checked strictly in the order their ids were
/// minted: item order inside a module, and module-*discovery* order across
/// them. So a declaration whose base came later kept only the bindings it
/// wrote itself, silently — no diagnostic where the mistake was, and an
/// `missing-impl` at every use for an effect that is right there in
/// the source.
///
/// It is not a contrived order. `cli/tests/conformance`'s
/// `lib/semantics/test/effects.buri` spreads a declaration written above
/// its own, and the day a migration made that file the first in its package
/// to import the module the base came from, the module was discovered
/// *through* it — so the base was minted second and eleven tests started
/// failing on a file nothing had edited.
///
/// [`Checker::ctx_decls_reached`] is the fix: a use checks its declaration
/// if checking has not reached it yet.
const SPREAD_BEFORE_ITS_BASE: &str = r#"
from "platform/effect" import { Clock };
from "core/time" import * as time;

struct Frozen { at: I64 }

impl Clock for Frozen {
  fn nowMilliseconds(self): I64 { self.at }
  fn sleepMilliseconds(self, milliseconds: Int): () { () }
  fn monotonicNanoseconds(self): I64 { self.at }
}

context Deep {
  ..Base(),
}

context Base {
  Clock: Frozen { at: 7 },
}

fn reading<C: Clock>(ctx: C): I64 { time.now(ctx).0 }

test "the spread carries the base's binding" {
  let ctx = Deep();
  let _ = reading(ctx);
}
"#;

#[test]
fn a_context_may_spread_one_declared_after_it() {
    // A test source, because that is where a context may be built — and
    // where every one this found is written.
    let mut map = SourceMap::new();
    let analysis = crate::compiler::driver::analyze_snippet(
        &mut map,
        "resolve_test.buri",
        SPREAD_BEFORE_ITS_BASE,
        crate::compiler::modules::Role::TestSource,
    );
    let errors: Vec<String> = analysis
        .diagnostics
        .items
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.message.clone())
        .collect();
    // The whole claim: the use of `Deep` type-checks. The binding is
    // asserted below as well, so a `Deep` that compiled by binding nothing
    // would still fail here.
    assert!(errors.is_empty(), "the snippet did not compile: {errors:?}");
    let tables = analysis.checked.tables;
    let deep = tables
        .ctx_decls
        .iter()
        .find(|d| d.name == "Deep")
        .expect("the snippet declares `Deep`");
    let checked = deep.checked.expect("`Deep` was checked");
    assert_eq!(
        tables.ctx_type(checked.ty).bindings.len(),
        1,
        "`Deep` binds what the spread gave it",
    );
}
