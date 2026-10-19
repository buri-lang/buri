//! The standard library declarations the compiler names by id
//! (`TyConId::OPTION` and the rest) are the ones it says, in every way a
//! compilation opens.

use crate::compiler::backend::intrinsic_keys::JsonArm;
use crate::compiler::driver::{self, Analysis};
use crate::compiler::modules::Role;
use crate::compiler::semantics::resolve::Sym;
use crate::compiler::semantics::types::{MethodId, TraitId, TyConId};
use crate::compiler::standard_library;
use crate::diagnostics::SourceMap;

const TYPES: &[(TyConId, &str, &str)] = &[
    (TyConId::OPTION, "core/option", "Option"),
    (TyConId::REQUEST, "platform/effect", "Request"),
    (TyConId::RESPONSE, "platform/effect", "Response"),
    (TyConId::SCOPE, "platform/effect", "Scope"),
    (TyConId::ORDER, "core/order", "Order"),
    (TyConId::RESULT, "core/result", "Result"),
    (TyConId::RANGE_ERROR, "core/number", "RangeError"),
];

/// Each trait, and the method an operator calls, which is its first.
const TRAITS: &[(TraitId, &str, &str, Option<&str>)] = &[
    (TraitId::ALLOCATOR, "platform/effect", "Allocator", None),
    (TraitId::EQUAL, "core/order", "Equal", Some("equal")),
    (TraitId::ORDERED, "core/order", "Ordered", Some("compare")),
    (TraitId::SHOW, "core/order", "Show", None),
    (TraitId::HASH, "core/order", "Hash", None),
    (TraitId::ADD, "core/number", "Add", Some("add")),
    (
        TraitId::SUBTRACT,
        "core/number",
        "Subtract",
        Some("subtract"),
    ),
    (
        TraitId::MULTIPLY,
        "core/number",
        "Multiply",
        Some("multiply"),
    ),
    (TraitId::DIVIDE, "core/number", "Divide", Some("divide")),
    (
        TraitId::REMAINDER,
        "core/number",
        "Remainder",
        Some("remainder"),
    ),
    (TraitId::NEGATE, "core/number", "Negate", Some("negate")),
    (TraitId::BOUNDED, "core/number", "Bounded", None),
    (TraitId::CHECKED, "core/number", "Checked", None),
    (TraitId::WRAPPING, "core/number", "Wrapping", None),
    (TraitId::SATURATING, "core/number", "Saturating", None),
];

fn holds_the_known_ids(how: &str, analysis: &Analysis) {
    let tables = &analysis.checked.tables;
    let path = |m| analysis.loaded.module(m).path.as_str();
    for &(id, module, name) in TYPES {
        let found = tables.tycon(id);
        assert_eq!(
            (path(found.module), found.name.as_str()),
            (module, name),
            "{how}: `TyConId` {} is no longer `{name}`; renumber the constants in \
             `semantics/types.rs`",
            id.index(),
        );
    }
    for &(id, module, name, method) in TRAITS {
        let found = tables.trait_(id);
        assert_eq!(
            (path(found.module), found.name.as_str()),
            (module, name),
            "{how}: `TraitId` {} is no longer `{name}`; renumber the constants in \
             `semantics/types.rs`",
            id.index(),
        );
        if let Some(method) = method {
            assert_eq!(
                found.methods.get(MethodId::FIRST).name,
                method,
                "{how}: {name}"
            );
        }
    }
    for (module, name) in standard_library::prelude() {
        let m = analysis
            .loaded
            .find(module)
            .unwrap_or_else(|| panic!("{how}: `{module}` isn't loaded"));
        assert!(
            analysis.checked.scopes.get(m).exports.contains_key(name),
            "{how}: `{module}` doesn't export its prelude name `{name}`"
        );
    }
}

#[test]
fn a_program_opens_with_the_known_ids() {
    let mut map = SourceMap::new();
    let analysis = driver::analyze_std_module(&mut map, "core/json");
    holds_the_known_ids("a program", &analysis);
}

/// Each [`JsonArm`] a primitive encodes to is the `core/json` `Json` variant
/// its `variant` says, and carries the one field its `payload` says.
#[test]
fn each_json_arm_is_the_variant_it_names() {
    let mut map = SourceMap::new();
    let analysis = driver::analyze_std_module(&mut map, "core/json");
    let tables = &analysis.checked.tables;
    let m = analysis
        .loaded
        .find("core/json")
        .expect("`core/json` is loaded");
    let Some(&Sym::Ty(json)) = analysis.checked.scopes.get(m).exports.get("Json") else {
        panic!("`core/json` exports the type `Json`");
    };
    let variants = tables.tycon(json).variants();
    for (arm, name) in [
        (JsonArm::Bool, "Bool"),
        (JsonArm::Num, "Num"),
        (JsonArm::Str, "Str"),
    ] {
        let v = variants.get(arm.variant());
        assert_eq!(v.name, name, "`VariantId::JSON_*` are `Json`'s order");
        let fields: Vec<_> = v.fields.iter().map(|f| f.ty).collect();
        assert_eq!(
            fields,
            [tables.prim(arm.payload())],
            "`Json.{name}`'s field"
        );
    }
}

#[test]
fn a_snippet_opens_with_the_known_ids() {
    let mut map = SourceMap::new();
    let analysis = driver::analyze_snippet(&mut map, "known.buri", "", Role::Source);
    holds_the_known_ids("a snippet", &analysis);
}

#[test]
fn the_self_check_opens_with_the_known_ids() {
    let mut map = SourceMap::new();
    let analysis = driver::analyze_stdlib(&mut map);
    holds_the_known_ids("the self-check", &analysis);
}
