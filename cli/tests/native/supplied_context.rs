//! A context the runtime supplies rather than the caller, natively, on every
//! native backend this toolchain has built in.
//!
//! A test's `render` walks its tree with the runtime driving the walk. A region
//! the tree rebuilds after a write is walked again from a watcher, and a press
//! fires a handler, and neither has a context to hand over. The context each is
//! handed must still be a value, whatever the test's context holds: one whose
//! effect keeps a heap value must not crash the run.
//!
//! Each row is a real `buri test` over a repository, under the heap check.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace(name: &str) -> PathBuf {
    crate::sweep::once();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-supplied-context-{}", std::process::id()))
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

/// A field whose signal feeds a `ui.each`, filled, and a button pressed, with a
/// test context that also holds a repository effect whose test implementation
/// keeps a `State` (buri-lang/buri#241). The effect is never called.
#[test]
fn a_fill_and_a_press_run_with_a_context_holding_a_state() {
    let repo = workspace("fill-with-state");
    write(&repo.join("REPO.buri"), "");
    write(
        &repo.join("platform/effect/note/BUILD.buri"),
        "library {\n    visibility: [\"//visibility:public\"]\n\n    testing {}\n}\n",
    );
    write(
        &repo.join("platform/effect/note/lib.buri"),
        r#"export effect Note {
    fn note(self, text: Str): ();
}

export fn note<C: Note>(ctx: C, text: Str): () {
    ctx.note(text)
}
"#,
    );
    write(
        &repo.join("platform/effect/note/testing/lib.buri"),
        r#"from "core/platforms/testing/state" import * as state;
from "//platform/effect/note" import { Note };

export struct TestNote {
    notes: state.State<[Str]>,
}

impl Note for TestNote {
    fn note(self, text: Str): () {
        state.update(self.notes, fn(c, notes) => (notes.push(c, text), ()))
    }
}

export fn note(): TestNote {
    TestNote { notes: state.new([]) }
}
"#,
    );
    write(
        &repo.join("lib/page/BUILD.buri"),
        "library {\n    test {\n        sources: [\"test/page.buri\"]\n        \
         dependencies: [\"//platform/effect/note\", \"//platform/effect/note/testing\"]\n    \
         }\n}\n",
    );
    write(
        &repo.join("lib/page/lib.buri"),
        r#"from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/signal" import { Signal };

export fn page<C>(search: Signal<Str>): Node<C> {
    let rows = ["row 0", "row 1", "row 2"];
    ui.stack({
        styles: [],
        children: [
            ui.field({ label: .Const("Search"), kind: .Search, value: search, styles: [] }),
            ui.each(
                .Computed(fn(s) => {
                    let typed = search.get(s);
                    rows.filter(s, fn(row) => row.contains(typed))
                }),
                fn(row) => row,
                fn(_c, row, _i) => ui.text({ content: .Const(row) }),
            ),
        ],
    })
}
"#,
    );
    write(
        &repo.join("lib/page/test/page.buri"),
        r#"from "core/testing/assert" import * as assert;
from "platform/effect" import { Allocator, Ui, Watch };
from "platform/effect/testing" import { alloc, headless, observer, render };
from "ui/node" import * as ui;
from "ui/signal" import { signal };
from "//lib/page" import { page };
from "//platform/effect/note" import { Note };
from "//platform/effect/note/testing" import { note };

context Noted {
    Allocator: alloc(),
    Ui: headless(),
    Watch: observer(),
    Note: note(),
}

test "filtering with the repository effect in the context" {
    let ctx = Noted();
    let search = signal(ctx, "");
    let shown = render(ctx, page(search));
    shown.fill("Search", "row 1");
    assert.equal(search.get(ctx), "row 1");
    assert.equal(shown.text(), "Search row 1 row 1");
}

test "pressing with the repository effect in the context" {
    let ctx = Noted();
    let pressed = signal(ctx, 0);
    let shown = render(
        ctx,
        ui.button({
            label: .Const("Go"),
            styles: [],
            onPress: .Some(fn(c) => {
                let _ = pressed.set(c, pressed.get(c) + 1);
            }),
        }),
    );
    shown.press("Go");
    shown.press("Go");
    assert.equal(pressed.get(ctx), 2);
}
"#,
    );

    for (backend, flags) in crate::e2e::build_modes() {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
        cmd.current_dir(&repo).arg("test").args(flags).arg("//lib/page");
        let ran = crate::shared::ran_command(crate::shared::heap_checked(&mut cmd));
        assert!(
            ran.status == 0 && ran.stdout.contains("2 passed, 0 failed"),
            "{backend}: a fill and a press with a State in the context:\n{}\n{}",
            ran.stdout,
            ran.stderr
        );
    }
}
