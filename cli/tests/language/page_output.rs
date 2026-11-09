//! **What a page prints reaches the reader, whatever ran it.**
//!
//! A page's handlers, rows, timers and tasks run from the browser's event
//! loop, long after `main` has returned. Each test here builds one page for
//! `web`, imports it into happy-dom's document through `harness::browser`,
//! drives one of those entry points the way a reader would, and asserts what
//! the page printed. The driver prints nothing of its own, so the output is
//! the page's alone. The page's mount prints `row a` and `typed ` first.

use crate::harness::browser::browser;
use crate::harness::hang::Capped;
use crate::harness::{js_runtime, Scratch};
use std::process::Command;

/// One page with a way to print from every entry point a reader can reach.
const PAGE: &str = r#"from "core/io" import * as io;
from "core/lazy" import * as lazy;
from "core/tasks" import * as tasks;
from "core/time" import * as time;
from "platform/effect" import { after, Allocator, Clock, Stderr, Stdout, Tasks, Ui };
from "ui/node" import * as ui;
from "ui/signal" import { signal };
from "web" import { WebHost };

fn loaded<C: Stdout>(ctx: C): () {
    io.println(ctx, "loaded").ignore()
}

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Clock: host.clock,
        Stderr: host.stderr,
        Stdout: host.stdout,
        Tasks: host.tasks,
        Ui: host.ui,
    };
    let items = signal(ctx, ["a"]);
    let typed = signal(ctx, "");
    ui.mount(
        ctx,
        ui.stack({
            styles: [],
            children: [
                ui.button({
                    label: .Const("Press"),
                    styles: [],
                    onPress: .Some(fn(c) => io.println(c, "pressed").ignore()),
                }),
                ui.button({
                    label: .Const("Warn"),
                    styles: [],
                    onPress: .Some(fn(c) => io.eprintln(c, "warned").ignore()),
                }),
                ui.button({
                    label: .Const("Wait"),
                    styles: [],
                    onPress: .Some(fn(c) => {
                        let _ = io.println(c, "before").ignore();
                        let _ = time.sleep(c, time.milliseconds(1));
                        io.println(c, "after").ignore()
                    }),
                }),
                ui.button({
                    label: .Const("Later"),
                    styles: [],
                    onPress: .Some(fn(c) => {
                        let _ = after(c, time.milliseconds(1), fn(d) => io.println(d, "fired").ignore());
                    }),
                }),
                ui.button({
                    label: .Const("Tick"),
                    styles: [],
                    onPress: .Some(fn(c) => {
                        let _ = tasks.after(c, time.milliseconds(1), fn(d) => io.println(d, "ticked").ignore());
                    }),
                }),
                ui.button({
                    label: .Const("Spawn"),
                    styles: [],
                    onPress: .Some(fn(c) => {
                        tasks.scope(c, fn(d, here) => {
                            tasks.spawn(d, here, fn(e) => io.println(e, "spawned").ignore())
                        })
                    }),
                }),
                ui.button({
                    label: .Const("Lazy"),
                    styles: [],
                    onPress: .Some(fn(c) => {
                        let run = lazy.load(loaded);
                        run(c)
                    }),
                }),
                ui.button({
                    label: .Const("Add"),
                    styles: [],
                    onPress: .Some(fn(c) => items.set(c, ["a", "b"])),
                }),
                ui.each(.Cell(items), fn(s) => s, fn(c, s, _i) => {
                    let _ = io.println(c, "row ${s}").ignore();
                    ui.text({ content: .Const(s) })
                }),
                ui.field({ label: .Const("Name"), kind: .Text, value: typed, styles: [] }),
                ui.rebuild(.Cell(typed), fn(c, t) => {
                    let _ = io.println(c, "typed ${t}").ignore();
                    ui.empty()
                }),
                ui.stack({
                    styles: [],
                    children: [ui.button({ label: .Const("Keyed"), styles: [], onPress: .None })],
                    onKey: .Some(fn(c, key) => {
                        let _ = io.println(c, "key ${key}").ignore();
                        true
                    }),
                }),
                ui.form({
                    onSubmit: fn(c) => io.println(c, "submitted").ignore(),
                    children: [ui.button({ label: .Const("Send"), styles: [], onPress: .None })],
                    styles: [],
                }),
                ui.button({
                    label: .Const("Hover"),
                    styles: [],
                    onPress: .None,
                    onHover: .Some(fn(c, on) => io.println(c, "hovered ${on}").ignore()),
                }),
                ui.button({
                    label: .Const("Outside"),
                    styles: [],
                    onPress: .None,
                    onPressOutside: .Some(fn(c) => io.println(c, "outside").ignore()),
                }),
            ],
        }),
        [],
    )
}
"#;

/// What the page wrote to stdout and stderr once `driver` has driven it and
/// everything the page started has finished.
fn printed(name: &str, driver: &str) -> (String, String) {
    let scratch = Scratch::repo(name);
    scratch.write(
        "cmd/page/BUILD.buri",
        "binary {\n    outputs: [\n        { platform: \"web\" },\n    ]\n}\n",
    );
    scratch.write("cmd/page/main.buri", PAGE);
    scratch.run(&["build", "//cmd/page"]).ok();
    let script = format!(
        "{}\nawait import(\"./.buri/out/web/cmd/page/main.mjs\");\n{driver}",
        browser()
    );
    let path = scratch.write("drive.mjs", &script);
    let out = Command::new(js_runtime())
        .arg(&path)
        .capped_output()
        .expect("the javascript runtime runs");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "the page did not run:\n{stdout}{stderr}"
    );
    (stdout, stderr)
}

const MOUNTED: &str = "row a\ntyped \n";

fn stdout_after(name: &str, driver: &str) -> String {
    let (stdout, _) = printed(name, driver);
    stdout
        .strip_prefix(MOUNTED)
        .unwrap_or_else(|| panic!("the mount did not print first:\n{stdout}"))
        .to_string()
}

#[test]
fn a_press_handler_that_does_not_wait_prints() {
    let out = stdout_after("print-press", r#"labelled("Press").click();"#);
    assert_eq!(out, "pressed\n");
}

#[test]
fn a_press_handler_that_does_not_wait_writes_stderr() {
    let (_, stderr) = printed("print-press-stderr", r#"labelled("Warn").click();"#);
    assert_eq!(stderr, "warned\n");
}

#[test]
fn a_press_handler_prints_after_it_waits() {
    let out = stdout_after("print-wait", r#"labelled("Wait").click();"#);
    assert_eq!(out, "before\nafter\n");
}

#[test]
fn a_page_timer_prints() {
    let out = stdout_after("print-page-timer", r#"labelled("Later").click();"#);
    assert_eq!(out, "fired\n");
}

#[test]
fn a_task_timer_prints() {
    let out = stdout_after("print-task-timer", r#"labelled("Tick").click();"#);
    assert_eq!(out, "ticked\n");
}

#[test]
fn a_spawned_task_prints() {
    let out = stdout_after("print-spawn", r#"labelled("Spawn").click();"#);
    assert_eq!(out, "spawned\n");
}

#[test]
fn a_lazily_loaded_function_prints() {
    let out = stdout_after("print-lazy", r#"labelled("Lazy").click();"#);
    assert_eq!(out, "loaded\n");
}

#[test]
fn rows_built_after_a_press_print() {
    let out = stdout_after("print-rows", r#"labelled("Add").click();"#);
    assert_eq!(out, "row b\n");
}

#[test]
fn a_subtree_rebuilt_as_the_reader_types_prints() {
    let out = stdout_after(
        "print-typed",
        r#"const field = document.querySelector("input"); field.value = "x"; fire(field, "input");"#,
    );
    assert_eq!(out, "typed x\n");
}

#[test]
fn a_key_handler_prints() {
    let out = stdout_after("print-key", r#"labelled("Keyed").focus(); press("Enter");"#);
    assert_eq!(out, "key Enter\n");
}

#[test]
fn a_submit_handler_prints() {
    let out = stdout_after(
        "print-submit",
        r#"labelled("Send").parentNode.requestSubmit();"#,
    );
    assert_eq!(out, "submitted\n");
}

#[test]
fn a_hover_handler_prints() {
    let out = stdout_after("print-hover", r#"fire(labelled("Hover"), "mouseenter");"#);
    assert_eq!(out, "hovered true\n");
}

#[test]
fn a_press_outside_handler_prints() {
    let out = stdout_after(
        "print-outside",
        r#"fire(labelled("Press"), "pointerdown");"#,
    );
    assert_eq!(out, "outside\n");
}
