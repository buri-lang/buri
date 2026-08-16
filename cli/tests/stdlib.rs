//! `core/*` has to typecheck against itself. It is the first program the
//! compiler ever sees, and every diagnostic it produces here is one a user
//! would otherwise hit through an import.

#[test]
fn the_standard_library_checks() {
    let mut map = buri::diag::SourceMap::new();
    let analysis = buri::driver::analyze_stdlib(&mut map);
    let mut out = String::new();
    for d in &analysis.diags.items {
        out.push_str(&map.render(d, false));
    }
    assert!(out.is_empty(), "the standard library does not check:\n{out}");
}

/// A module that loads lazily may not declare a method on a built-in type.
///
/// This is the rule that makes lazy loading safe, and it needs enforcing
/// because breaking it fails *silently*. A method needs no import: `xs.map(…)`
/// resolves in `[T]`'s defining module. If `core/bytes` declared `impl [U8]`
/// and `core/bytes` were not loaded — because the program never imported it —
/// the method would simply not be found, and the error would name the call
/// site rather than the cause.
///
/// The two ways to satisfy it: put the module in `stdlib::EAGER_MODULES`, or
/// write free functions instead. `core/bytes` takes the second, which is why
/// it is `bytes.toHex(ctx, b)` rather than `b.toHex(ctx)`.
#[test]
fn a_lazily_loaded_module_declares_no_method_on_a_built_in_type() {
    const BUILT_IN: &[&str] = &[
        "Str", "Char", "Bool", "Int", "Float", "I8", "I16", "I32", "I64", "I128", "U8", "U16",
        "U32", "U64", "U128", "F32", "F64",
    ];

    let mut offenders = Vec::new();
    let mut checked = 0;
    for path in buri::stdlib::MODULES {
        if buri::stdlib::EAGER_MODULES.contains(path) {
            continue;
        }
        let Some(source) = buri::stdlib::source(path) else { continue };
        checked += 1;
        let parsed = buri::parse::parse_stdlib(source, buri::diag::FileId(0));
        for item in &parsed.module.items {
            let buri::ast::Item::Impl(d) = item else { continue };
            // The self position is the type the methods belong to. With a
            // `for` clause it is the second one; that is a conformance, and a
            // conformance travels with the trait rather than with the module.
            if d.trait_ty.is_some() {
                continue;
            }
            let name = match &d.self_ty {
                buri::ast::TypeExpr::Array { .. } => "[T]".to_string(),
                buri::ast::TypeExpr::Named { path, .. } => {
                    path.last().map(|s| s.name.clone()).unwrap_or_default()
                }
                _ => continue,
            };
            if name == "[T]" || BUILT_IN.contains(&name.as_str()) {
                offenders.push(format!("{path} declares `impl {name}`"));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "a lazily loaded module declares a method on a built-in type, which will not \
         resolve in a program that does not import it:\n  {}\n\nEither add the module to \
         stdlib::EAGER_MODULES, or make these free functions.",
        offenders.join("\n  ")
    );
    assert!(checked > 5, "only {checked} lazily loaded module(s); the scan is not working");
}

/// Every module still loads, one at a time, on top of the eager set.
///
/// Lazy loading means a module is first seen in a compilation that holds only
/// the built-in types and whatever it imports for itself. `analyze_stdlib`
/// loads all of them together, so it cannot notice a module that only checks
/// because something else happened to be present.
#[test]
fn every_module_checks_on_its_own() {
    let mut broken = Vec::new();
    for path in buri::stdlib::MODULES {
        let mut map = buri::diag::SourceMap::new();
        let analysis = buri::driver::analyze_std_module(&mut map, path);
        let mut out = String::new();
        for d in &analysis.diags.items {
            out.push_str(&map.render(d, false));
        }
        if !out.is_empty() {
            broken.push(format!("--- {path}\n{out}"));
        }
    }
    assert!(
        broken.is_empty(),
        "module(s) that check only when something else is loaded too:\n{}",
        broken.join("\n")
    );
}
