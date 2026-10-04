//! `semantics::expressions` on whole snippets, through the real front end.
//! Here rather than beside the pass because a snippet compiles through
//! `driver`, which is in `buri`.

use crate::diagnostics::SourceMap;

/// Every error one snippet reports, through the real front end.
///
/// **`Role::Platform` rather than `Role::Entry`**, and it buys these tests
/// two things at once. What they are about is `implementing_ty` — which
/// type `Self` stands for when an effect method is reached *through a
/// context value* — and the only way to write that call down is the method
/// form, which is legal in the standard library and in an `impl` that
/// supplies an effect, and nowhere else (`report_effect_method`). A
/// platform module is one of those two, and it may build a context
/// (SPEC 11.3), so the claim is written where it is still writable.
///
/// The second thing is that a platform module may **declare an effect**,
/// which is why the snippet mints its own `Accept` rather than borrowing
/// one from `platform/effect`. No standard-library effect spells `Self` in a
/// callback any more — `Listen` used to, and its request handler moved
/// into `core/net/server`'s own loop — so the rule outlived its last
/// in-tree instance, and a test that depended on one would have died with
/// it. The rule is about `Self`, not about servers.
fn errors_of(src: &str) -> Vec<String> {
    let mut map = SourceMap::new();
    let analysis = crate::compiler::driver::analyze_snippet(
        &mut map,
        "self_through_a_context.buri",
        src,
        crate::compiler::modules::Role::Platform,
    );
    analysis
        .diagnostics
        .items
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.message.clone())
        .collect()
}

/// A `Listen` and a `Network`, each an ordinary struct, and a context binding
/// both. `{handler}` is the `status` of the handler's `Response`, which is
/// the one thing the tests below disagree about.
///
/// The context is built in `main` because that is one of the three places
/// authority may enter (`misplaced-context`), and a snippet is a whole
/// module rather than a body.
fn snippet(handler: &str) -> String {
    format!(
        r#"
from "platform/effect" import {{ Network, NetError, Request, Response }};

effect Accept {{
  fn accept(self, address: Str, onRequest: fn(Self, Request) => Response): Bool;
}}

struct Server {{ mark: Int }}

impl Accept for Server {{
  fn accept(self, address: Str, onRequest: fn(Self, Request) => Response): Bool {{
true
  }}
}}

struct Caller {{}}

impl Network for Caller {{
  fn fetch(self, request: Request): Result<Response, NetError> {{
.Err(.Refused)
  }}
}}

from "node" import {{ NodeHost }};

export fn main(host: NodeHost): Result<(), Str> {{
  let ctx = context {{ Accept: Server {{ mark: 7 }}, Network: Caller {{}} }};
  match (ctx.accept("a", fn(acceptor, request) => Response {{
status: {handler},
headers: [],
body: [],
  }})) {{
true => .Ok(()),
false => .Err("refused"),
  }}
}}
"#
    )
}

/// The handler's first parameter is the **implementation**, so a field of
/// it is readable.
///
/// A context has no fields at all, so this is the whole claim in one line:
/// before `implementing_ty`, `Self` was the receiver and `acceptor.mark`
/// reported `no-field` on a type with no name to print.
#[test]
fn a_self_parameter_through_a_context_is_the_implementing_type() {
    let errors = errors_of(&snippet("acceptor.mark"));
    assert!(errors.is_empty(), "the snippet did not compile: {errors:?}");
}

/// And it is **not** the context, which is the same claim from the other
/// side.
///
/// `fetch` is `Network`'s, the context binds `Network`, and `Server` — which is
/// what implements `Listen` — does not. So a `Self` that was still the
/// receiver would resolve this call and hand the handler a context at
/// runtime; the implementation arrives instead, and the front end says so.
#[test]
fn a_self_parameter_through_a_context_is_not_the_context() {
    let errors = errors_of(&snippet(
        "match (acceptor.fetch(request)) { .Ok(r) => r.status, .Err(_) => 0 }",
    ));
    assert!(
        errors.iter().any(|m| m.contains("fetch")),
        "expected the handler's parameter to have no `fetch`, got {errors:?}"
    );
}

/// A written `Self` in the `impl` head's own signature and the one the
/// call site substitutes are the same type — A1 fixed the first, and this
/// is the pair agreeing.
///
/// The `impl` above writes `fn(Self, Request) => Response` rather than
/// `fn(Server, Request) => Response`, so the passing test above is already
/// that agreement; this is the other spelling, which must compile too.
#[test]
fn the_impl_may_spell_the_handler_with_the_concrete_type_instead() {
    let src = snippet("acceptor.mark")
        .replace("onRequest: fn(Self, Request) => Response", "onRequest: fn(Server, Request) => Response");
    let errors = errors_of(&src);
    assert!(errors.is_empty(), "the snippet did not compile: {errors:?}");
}
