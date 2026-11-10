//! Two rows with one key, in `platform/effect/testing`'s `render`, on
//! JavaScript and on every native backend this toolchain has built in.
//!
//! A browser refuses the list where the keys are read and names the key
//! (`cli/tests/crash/ui_duplicate_row_key.buri`). A document with no browser
//! refuses it the same way, for `ui.each` and for `shape.each` alike.

use std::path::Path;
use std::process::Command;

fn write(at: &Path, body: &str) {
    if let Some(dir) = at.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(at, body).unwrap();
}

/// A repository whose one test renders `node`, a list keyed by its items, from
/// `["milk", "eggs", "milk"]`.
fn rendering(name: &str, node: &str) -> crate::sweep::Owned {
    let repo = crate::sweep::owned("native-duplicate-keys", name);
    write(&repo.join("REPO.buri"), "");
    write(
        &repo.join("list/BUILD.buri"),
        "library {\n    sources: [\"list.buri\"]\n\n    test {\n        \
         sources: [\"test/list_test.buri\"]\n    }\n}\n",
    );
    write(
        &repo.join("list/lib.buri"),
        "from \"//list/list.buri\" export { itself };\n",
    );
    write(
        &repo.join("list/list.buri"),
        "export fn itself(item: Str): Str {\n    item\n}\n",
    );
    write(
        &repo.join("list/test/list_test.buri"),
        &format!(
            r#"from "platform/effect" import {{ Allocator, Ui, Watch }};
from "platform/effect/testing" import {{ alloc, headless, observer, render }};
from "ui/node" import * as ui;
from "ui/shape" import * as shape;
from "//list" import {{ itself }};

test "two rows with one key" {{
    let ctx = context {{ Allocator: alloc(), Ui: headless(), Watch: observer() }};
    let shopping = ["milk", "eggs", "milk"];
    let _ = render(ctx, {node});
}}
"#
        ),
    );
    repo
}

fn refused(repo: &Path) {
    let mut modes = crate::e2e::build_modes();
    modes.push(("js", &["--output=js"]));
    for (backend, flags) in modes {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
        cmd.current_dir(repo).arg("test").args(flags).arg("//list");
        let ran = crate::shared::ran_command(&mut cmd);
        assert!(
            ran.status != 0
                && format!("{}{}", ran.stdout, ran.stderr)
                    .contains("two rows share the key \"milk\""),
            "{backend}: two rows with one key:\n{}\n{}",
            ran.stdout,
            ran.stderr
        );
    }
}

#[test]
fn a_list_with_two_rows_of_one_key_is_refused_naming_the_key() {
    let repo = rendering(
        "nodes",
        "ui.each(.Const(shopping), itself, fn(_c, item, _i) => ui.text({ content: .Const(item) }))",
    );
    refused(&repo);
}

#[test]
fn a_drawing_with_two_rows_of_one_key_is_refused_naming_the_key() {
    let repo = rendering(
        "shapes",
        "ui.drawing({ alt: .Decorative, size: .Const((10.0, 10.0)), styles: [], shapes: [\
         shape.each(.Const(shopping), itself, fn(_c, item, _i) => shape.text({ at: .Const((0.0, 0.0)), content: .Const(item) }))] })",
    );
    refused(&repo);
}
