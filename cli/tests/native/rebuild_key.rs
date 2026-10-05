//! A region `ui.rebuild` keys on a heap value, natively, on every native
//! backend this toolchain has built in.
//!
//! The key is read from a signal inside a `ui.computed`, and a `fill` writes the
//! signal, so the region is walked again and its key compared with the one it
//! was built under. Each row is a real `buri test` over a repository, under the
//! heap check, and ends on a passing assert so the heap audit runs.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace(name: &str) -> PathBuf {
    crate::sweep::once();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-rebuild-key-{}", std::process::id()))
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

/// Builds a repository whose `page` is a search field above `region`, and runs
/// a test that renders it and then runs `steps` on every native backend, under
/// the heap check.
fn passes(name: &str, region: &str, steps: &str) {
    let repo = workspace(name);
    write(&repo.join("REPO.buri"), "");
    write(
        &repo.join("lib/page/BUILD.buri"),
        "library {\n    test {\n        sources: [\"test/page.buri\"]\n    }\n}\n",
    );
    write(
        &repo.join("lib/page/lib.buri"),
        &format!(
            r#"from "ui/node" import * as ui;
from "ui/node" import {{ Node }};
from "ui/signal" import {{ Signal }};

struct Typed {{
    text: Str,
}}

derive Show for Typed;

export fn page<C>(search: Signal<Str>): Node<C> {{
    ui.stack({{
        styles: [],
        children: [
            ui.field({{ label: .Const("Search"), kind: .Search, value: search, styles: [] }}),
            {region},
        ],
    }})
}}
"#
        ),
    );
    write(
        &repo.join("lib/page/test/page.buri"),
        &format!(
            r#"from "core/testing/assert" import * as assert;
from "platform/effect" import {{ Allocator, Ui, Watch }};
from "platform/effect/testing" import {{ alloc, headless, observer, render }};
from "ui/signal" import {{ signal }};
from "//lib/page" import {{ page }};

context Page {{
    Allocator: alloc(),
    Ui: headless(),
    Watch: observer(),
}}

test "the region follows the field" {{
    let ctx = Page();
    let search = signal(ctx, "");
    let shown = render(ctx, page(search));
{steps}
}}
"#
        ),
    );

    for (backend, flags) in crate::e2e::build_modes() {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
        cmd.current_dir(&repo).arg("test").args(flags).arg("//lib/page");
        let ran = crate::shared::ran_command(crate::shared::heap_checked(&mut cmd));
        assert!(
            ran.status == 0 && ran.stdout.contains("1 passed, 0 failed"),
            "{backend}: {name}:\n{}\n{}",
            ran.stdout,
            ran.stderr
        );
    }
}

/// The key is the string the field holds.
#[test]
fn a_region_keyed_on_a_string_survives_a_fill() {
    passes(
        "string",
        r#"ui.computed(fn(s) => {
                let typed = search.get(s);
                ui.rebuild(.Const(typed), fn(_c, _k) => ui.text({ content: .Const("x") }))
            })"#,
        r#"    shown.fill("Search", "row 1");
    assert.equal(shown.text(), "Search row 1 x");"#,
    );
}

/// The key is a record holding the string the field holds, and the region
/// shows that string.
#[test]
fn a_region_keyed_on_a_record_holding_a_string_survives_a_fill() {
    passes(
        "record",
        r#"ui.computed(fn(s) => {
                let typed = search.get(s);
                ui.rebuild(
                    .Const(Typed { text: typed }),
                    fn(_c, k) => ui.text({ content: .Const(k.text) }),
                )
            })"#,
        r#"    shown.fill("Search", "row 1");
    assert.equal(shown.text(), "Search row 1 row 1");"#,
    );
}

/// The key changes on every fill, so every fill tears the region down and
/// builds it again from the new key.
#[test]
fn a_region_keyed_on_a_string_follows_every_fill() {
    passes(
        "every-fill",
        r#"ui.computed(fn(s) => {
                let typed = search.get(s);
                ui.rebuild(.Const(typed), fn(_c, k) => ui.text({ content: .Const(k) }))
            })"#,
        r#"    shown.fill("Search", "a");
    shown.fill("Search", "ab");
    shown.fill("Search", "abc");
    shown.fill("Search", "");
    shown.fill("Search", "z");
    assert.equal(shown.text(), "Search z z");"#,
    );
}
