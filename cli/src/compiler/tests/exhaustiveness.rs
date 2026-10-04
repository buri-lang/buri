//! `semantics::exhaustiveness` on whole snippets, through the real front end.
//! Here rather than beside the pass because a snippet compiles through
//! `driver`, which is in `buri`.

use crate::diagnostics::SourceMap;

/// Every finding one snippet reports: the code, and the text the caret is
/// under.
///
/// The span is half the point of this refinement — a diagnostic that says
/// an alternative is dead and then underlines the whole arm has not
/// answered the question — so it is what the assertions below are written
/// against.
fn findings(src: &str) -> Vec<(String, String)> {
    let mut map = SourceMap::new();
    let analysis = crate::compiler::driver::analyze_snippet(
        &mut map,
        "reachability.buri",
        src,
        crate::compiler::modules::Role::Source,
    );
    let mut out = Vec::new();
    for d in &analysis.diagnostics.items {
        let code = d.code.clone().unwrap_or_default();
        let (start, end) = (d.span.start as usize, d.span.end as usize);
        let text = src.get(start..end).unwrap_or("").to_string();
        out.push((code, text));
    }
    out
}

/// Codes only, for the cases that are about what is and is not reported.
fn codes(src: &str) -> Vec<String> {
    findings(src).into_iter().map(|(c, _)| c).collect()
}

const HELLO: &str = "enum Hello { World, Now(Bool) }\n";

/// The report this refinement was written for. The arm is live —
/// `Hello.World` reaches it — and the alternative beside it is not.
#[test]
fn a_dead_alternative_beside_a_live_one_is_reported() {
    let src = format!(
        "{HELLO}fn greeting(hi: Hello): Str {{\n  \
         match (hi) {{\n    \
         Hello.Now(_) => \"hello world\",\n    \
         Hello.Now(_) | Hello.World => \"now\",\n  }}\n}}\n"
    );
    assert_eq!(
        findings(&src),
        vec![("unreachable-alternative".to_string(), "Hello.Now(_)".to_string())],
        "the second arm's first alternative is dead and the arm is not"
    );
}

/// The dead one in the middle, so that a report cannot pass by naming the
/// first alternative or the last.
#[test]
fn a_dead_middle_alternative_is_reported() {
    let src = format!(
        "{HELLO}fn greeting(hi: Hello): Str {{\n  \
         match (hi) {{\n    \
         Hello.Now(true) => \"a\",\n    \
         Hello.World | Hello.Now(true) | Hello.Now(false) => \"b\",\n  }}\n}}\n"
    );
    assert_eq!(
        findings(&src),
        vec![("unreachable-alternative".to_string(), "Hello.Now(true)".to_string())]
    );
}

/// Every alternative dead is the arm dead, and that is the older report,
/// against the whole arm. Both would be two names for one mistake.
#[test]
fn an_arm_whose_alternatives_are_all_dead_is_one_unreachable_arm() {
    let src = format!(
        "{HELLO}fn greeting(hi: Hello): Str {{\n  \
         match (hi) {{\n    \
         _ => \"a\",\n    \
         Hello.World | Hello.Now(_) => \"b\",\n  }}\n}}\n"
    );
    assert_eq!(
        findings(&src),
        vec![(
            "unreachable-arm".to_string(),
            "Hello.World | Hello.Now(_) => \"b\"".to_string()
        )]
    );
}

/// An alternative may overlap the arms above it and still be worth
/// writing. `Hello.Now(_)` covers `Hello.Now(true)`, which is taken, and
/// `Hello.Now(false)`, which is not.
#[test]
fn an_alternative_that_still_adds_coverage_is_left_alone() {
    let src = format!(
        "{HELLO}fn greeting(hi: Hello): Str {{\n  \
         match (hi) {{\n    \
         Hello.Now(true) => \"a\",\n    \
         Hello.Now(_) | Hello.World => \"b\",\n  }}\n}}\n"
    );
    assert_eq!(codes(&src), Vec::<String>::new());
}

/// The arm's own alternatives are part of the matrix too, so `A | A` is
/// caught with nothing above it at all.
#[test]
fn an_alternative_repeated_within_one_arm_is_reported() {
    let src = format!(
        "{HELLO}fn greeting(hi: Hello): Str {{\n  \
         match (hi) {{\n    \
         Hello.World | Hello.World | Hello.Now(_) => \"a\",\n  }}\n}}\n"
    );
    assert_eq!(
        findings(&src),
        vec![("unreachable-alternative".to_string(), "Hello.World".to_string())]
    );
}

/// `a | (b | c)` is one flat list of three. The parser reads a run of `|`
/// as one node, so the only nesting is the parentheses, and a report has to
/// reach through them to the alternative that is actually dead.
#[test]
fn a_parenthesised_alternation_is_flattened() {
    let src = format!(
        "{HELLO}fn greeting(hi: Hello): Str {{\n  \
         match (hi) {{\n    \
         Hello.Now(true) => \"a\",\n    \
         Hello.World | (Hello.Now(true) | Hello.Now(false)) => \"b\",\n  }}\n}}\n"
    );
    assert_eq!(
        findings(&src),
        vec![("unreachable-alternative".to_string(), "Hello.Now(true)".to_string())]
    );
}

/// An alternation *inside* a constructor is not an alternative of the arm.
/// It counts toward coverage — that is what makes this `match` exhaustive
/// and its second arm live — and it is not reported one branch at a time.
#[test]
fn an_alternation_inside_a_constructor_is_not_an_arm_alternative() {
    let src = format!(
        "{HELLO}fn greeting(hi: Hello): Str {{\n  \
         match (hi) {{\n    \
         Hello.World => \"a\",\n    \
         Hello.Now(true | false) => \"b\",\n  }}\n}}\n"
    );
    assert_eq!(codes(&src), Vec::<String>::new());
}

/// A guarded arm covers nothing below it, so an alternative that repeats
/// one of its patterns is live — the guard may have failed.
#[test]
fn a_guarded_arm_does_not_kill_the_alternatives_below_it() {
    let src = format!(
        "{HELLO}fn greeting(hi: Hello, ok: Bool): Str {{\n  \
         match (hi) {{\n    \
         Hello.Now(_) if ok => \"a\",\n    \
         Hello.Now(_) | Hello.World => \"b\",\n  }}\n}}\n"
    );
    assert_eq!(codes(&src), Vec::<String>::new());
}

/// The guard does not save an alternative from the one beside it, though:
/// both halves of `A | A if g` are behind the same guard, so the second
/// still never decides anything.
#[test]
fn a_guard_does_not_save_an_alternative_from_its_own_arm() {
    let src = format!(
        "{HELLO}fn greeting(hi: Hello, ok: Bool): Str {{\n  \
         match (hi) {{\n    \
         Hello.World | Hello.World if ok => \"a\",\n    \
         _ => \"b\",\n  }}\n}}\n"
    );
    assert_eq!(
        findings(&src),
        vec![("unreachable-alternative".to_string(), "Hello.World".to_string())]
    );
}

/// A rest pattern is expanded into one row per length it can match, and
/// those rows are not alternatives anybody wrote. `[_, ..rest]` covers
/// length one, which `[_]` above it already took, and it is still the
/// pattern that covers every longer array.
#[test]
fn a_rest_patterns_expansion_is_not_an_alternative() {
    let src = "fn n(xs: [Bool]): Int {\n  \
               match (xs) {\n    \
               [] => 0,\n    \
               [_] => 1,\n    \
               [_, ..rest] => 2,\n  }\n}\n";
    assert_eq!(codes(src), Vec::<String>::new());
}

/// A pattern the checker could not build is a wildcard to this pass, and a
/// wildcard would make everything after it look dead. The finer question is
/// not asked at all of a `match` that already has an error in it.
#[test]
fn a_broken_pattern_does_not_cascade_into_dead_alternatives() {
    let src = format!(
        "{HELLO}fn greeting(hi: Hello): Str {{\n  \
         match (hi) {{\n    \
         Hello.Nope => \"a\",\n    \
         Hello.Now(_) | Hello.World => \"b\",\n    \
         _ => \"c\",\n  }}\n}}\n"
    );
    let reported = codes(&src);
    assert!(
        !reported.iter().any(|c| c == "unreachable-alternative"),
        "an unresolved variant should not make the arms after it look dead: {reported:?}"
    );
}
