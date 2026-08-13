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
