//! `buri test --coverage` counts a call of a branching built-in as a decision
//! (`coverage::BUILTINS`). That list is written by hand, so this reads the
//! signatures of `core/*` and fails when a function that branches is missing
//! from it, or when it names one that is gone.
//!
//! A function branches, by this rule, when its receiver (its first parameter)
//! is an `Option` or a `Result`, or its last parameter is a function that
//! answers `Bool` or `Option`. Each such function is either listed in
//! `BUILTINS`, or in [`DOES_NOT_BRANCH`] with the reason a call of it isn't a
//! choice worth a branch.
use buri::compiler::middle::coverage::{Builtin, BUILTINS};
use buri::compiler::standard_library::MODULES;
use buri::diagnostics::FileId;
use buri::parsing::flat::{Tree, TypeView};
use buri::parsing::parser::parse_stdlib;
use buri::parsing::tree::{FnDecl, Item, ParamKind};
use std::collections::{BTreeMap, BTreeSet};

/// The functions the rule finds that a call of doesn't split, by module, name
/// and why.
const DOES_NOT_BRANCH: &[(&str, &str, &str)] = &[
    (
        "core/option",
        "isSome",
        "answers a Bool, and the `if` on it is the branch",
    ),
    (
        "core/option",
        "isNone",
        "answers a Bool, and the `if` on it is the branch",
    ),
    (
        "core/option",
        "flatten",
        "its outcome depends on two variants, so no one receiver splits it",
    ),
    (
        "core/option",
        "zip",
        "its outcome depends on two variants, so no one receiver splits it",
    ),
    (
        "core/result",
        "isOk",
        "answers a Bool, and the `if` on it is the branch",
    ),
    (
        "core/result",
        "isErr",
        "answers a Bool, and the `if` on it is the branch",
    ),
    (
        "core/result",
        "ignore",
        "drops the value without choosing between its variants",
    ),
    (
        "core/testing/assert",
        "ok",
        "an assertion: the other variant fails the test rather than taking a path",
    ),
    (
        "core/testing/assert",
        "err",
        "an assertion: the other variant fails the test rather than taking a path",
    ),
    (
        "core/testing/assert",
        "some",
        "an assertion: the other variant fails the test rather than taking a path",
    ),
    (
        "core/testing/assert",
        "none",
        "an assertion: the other variant fails the test rather than taking a path",
    ),
    (
        "core/testing/assert",
        "equalWith",
        "an equivalence the test states, whose `false` fails the test",
    ),
    (
        "core/testing/check",
        "forAll",
        "a property, whose `false` fails the test",
    ),
    (
        "core/testing/check",
        "forAllCtx",
        "a property, whose `false` fails the test",
    ),
];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Why {
    Receiver,
    Predicate,
}

fn head(tree: &Tree, ty: buri::parsing::flat::TypeId) -> Option<String> {
    tree.type_head(ty).map(str::to_string)
}

/// Why `d` branches, if it does. `owner` is the type of the `impl` it's in.
fn why(tree: &Tree, d: &FnDecl, owner: Option<&str>) -> BTreeSet<Why> {
    let mut found = BTreeSet::new();
    let params = tree.list(d.params);
    let receiver = match params.first() {
        Some(p) if p.kind == ParamKind::SelfParam => owner.map(str::to_string),
        Some(p) => p.written_type().and_then(|t| head(tree, t)),
        None => None,
    };
    if matches!(receiver.as_deref(), Some("Option" | "Result")) {
        found.insert(Why::Receiver);
    }
    let last = params.last().and_then(|p| p.written_type());
    if let Some(TypeView::Fn { ret, .. }) = last.map(|t| tree.ty(t)) {
        if matches!(head(tree, ret).as_deref(), Some("Bool" | "Option")) {
            found.insert(Why::Predicate);
        }
    }
    found
}

/// Every exported function of `core/*` the rule picks, by module and name.
fn candidates() -> BTreeMap<(String, String), BTreeSet<Why>> {
    let mut out: BTreeMap<(String, String), BTreeSet<Why>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for module in MODULES.iter().filter(|m| m.path.starts_with("core/")) {
        let parsed = parse_stdlib(module.source, FileId(0));
        let tree = &parsed.module.tree;
        let mut note = |d: &FnDecl, owner: Option<&str>| {
            if !d.exported {
                return;
            }
            let key = (module.path.to_string(), tree.name(d.name).to_string());
            seen.insert(key.clone());
            let reasons = why(tree, d, owner);
            let entry = out.entry(key).or_default();
            entry.extend(reasons);
        };
        for item in &parsed.module.items {
            match item {
                Item::Fn(d) => note(d, None),
                Item::Impl(i) if i.trait_ty.is_none() => {
                    let owner = head(tree, i.self_ty);
                    for d in tree.list(i.methods) {
                        note(d, owner.as_deref());
                    }
                }
                _ => {}
            }
        }
    }
    assert!(seen.len() > 500, "only {} functions read", seen.len());
    out.retain(|_, why| !why.is_empty());
    out
}

fn listed(module: &str, name: &str) -> Option<Builtin> {
    BUILTINS
        .iter()
        .find(|(m, _, names)| *m == module && names.contains(&name))
        .map(|(_, how, _)| *how)
}

#[test]
fn every_stdlib_function_that_branches_is_a_builtin_or_says_why_not() {
    let missing: Vec<String> = candidates()
        .keys()
        .filter(|(m, n)| {
            listed(m, n).is_none() && !DOES_NOT_BRANCH.iter().any(|(dm, dn, _)| dm == m && dn == n)
        })
        .map(|(m, n)| format!("{m}.{n}"))
        .collect();
    assert!(
        missing.is_empty(),
        "these branch but are not counted: add each to `coverage::BUILTINS`, or to \
         DOES_NOT_BRANCH with a reason:\n  {}",
        missing.join("\n  ")
    );
}

#[test]
fn every_builtin_exists_and_branches_the_way_it_is_listed() {
    let found = candidates();
    let mut wrong = Vec::new();
    for (module, how, names) in BUILTINS {
        for name in *names {
            let key = (module.to_string(), name.to_string());
            let need = match how {
                Builtin::Variant => Why::Receiver,
                Builtin::Predicate => Why::Predicate,
            };
            if !found.get(&key).is_some_and(|w| w.contains(&need)) {
                wrong.push(format!("{module}.{name} is listed as {how:?}"));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "these are in `coverage::BUILTINS` but are not exported functions of that kind:\n  {}",
        wrong.join("\n  ")
    );
}

#[test]
fn nothing_that_does_not_branch_is_stale_or_also_a_builtin() {
    let found = candidates();
    let mut wrong = Vec::new();
    for (module, name, _) in DOES_NOT_BRANCH {
        let key = (module.to_string(), name.to_string());
        if !found.contains_key(&key) {
            wrong.push(format!("{module}.{name} is not a function the rule picks"));
        }
        if listed(module, name).is_some() {
            wrong.push(format!("{module}.{name} is also in `BUILTINS`"));
        }
    }
    assert!(
        wrong.is_empty(),
        "DOES_NOT_BRANCH is stale:\n  {}",
        wrong.join("\n  ")
    );
}
