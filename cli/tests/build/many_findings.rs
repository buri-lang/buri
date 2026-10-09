//! A file with a finding on every one of its thousands of functions, linted,
//! fixed and published the way a user and an editor meet it.
//!
//! `profile` holds each of these shapes linear in its findings. These hold what
//! the answers are: every finding once, where it is, after edits, and a fix
//! that leaves the program printing what it printed.

use crate::harness::*;

/// One `buri lsp` on a pipe, answering each message completely before reading
/// the next, so the server goes idle between requests as it does behind an
/// editor.
struct Server {
    process: std::process::Child,
    next_id: u64,
}

impl Server {
    fn open(root: &std::path::Path) -> Server {
        let process = buri_command()
            .arg("lsp")
            .env("BURI_LSP_ANALYSIS", "synchronous")
            .current_dir(root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the language server did not start");
        let mut server = Server { process, next_id: 0 };
        let root = buri::json::Value::str(format!("file://{}", root.display())).to_string();
        server.ask("initialize", &format!(r#"{{"rootUri":{root},"capabilities":{{}}}}"#));
        server.notify("initialized", "{}");
        server
    }

    /// One request, and its own answer. Whatever the server sends meanwhile is
    /// dropped.
    fn ask(&mut self, method: &str, params: &str) -> buri::json::Value {
        self.next_id += 1;
        let id = self.next_id;
        self.write(&format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params}}}"#));
        loop {
            let message = buri::json::parse(&self.read()).expect("the server wrote JSON");
            if message.get("id").and_then(|v| v.as_u32()) == Some(id as u32) {
                return message;
            }
        }
    }

    fn notify(&mut self, method: &str, params: &str) {
        self.write(&format!(r#"{{"jsonrpc":"2.0","method":"{method}","params":{params}}}"#));
    }

    /// A buffer's whole text, as `didOpen` at version 1 and `didChange` after.
    fn text(&mut self, uri: &str, version: u32, text: &str) {
        let uri = buri::json::Value::str(uri).to_string();
        let text = buri::json::Value::str(text).to_string();
        if version == 1 {
            self.notify(
                "textDocument/didOpen",
                &format!(r#"{{"textDocument":{{"uri":{uri},"languageId":"buri","version":1,"text":{text}}}}}"#),
            );
        } else {
            self.notify(
                "textDocument/didChange",
                &format!(r#"{{"textDocument":{{"uri":{uri},"version":{version}}},"contentChanges":[{{"text":{text}}}]}}"#),
            );
        }
    }

    /// Every finding a pull reports for `uri`: its code, line and columns.
    fn pull(&mut self, uri: &str) -> Vec<(String, u32, u32, u32)> {
        let uri = buri::json::Value::str(uri).to_string();
        let answer = self.ask("textDocument/diagnostic", &format!(r#"{{"textDocument":{{"uri":{uri}}}}}"#));
        let items = answer.at("result.items").and_then(|v| v.as_array()).expect("a full report");
        items
            .iter()
            .map(|item| {
                let n = |path: &str| item.at(path).and_then(|v| v.as_u32()).unwrap();
                let end_line = n("range.end.line");
                assert_eq!(end_line, n("range.start.line"), "a bound spans one line");
                let code = item.get("code").and_then(|v| v.as_str()).unwrap_or("").to_string();
                (code, n("range.start.line"), n("range.start.character"), n("range.end.character"))
            })
            .collect()
    }

    fn write(&mut self, body: &str) {
        use std::io::Write;
        let stdin = self.process.stdin.as_mut().expect("the server's stdin is a pipe");
        write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        stdin.flush().unwrap();
    }

    fn read(&mut self) -> String {
        use std::io::Read;
        let stdout = self.process.stdout.as_mut().expect("the server's stdout is a pipe");
        let mut headers = String::new();
        while !headers.ends_with("\r\n\r\n") {
            let mut byte = [0u8; 1];
            assert_eq!(stdout.read(&mut byte).unwrap(), 1, "the server closed the stream");
            headers.push(byte[0] as char);
        }
        let length: usize = headers
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .expect("a message with no Content-Length")
            .trim()
            .parse()
            .unwrap();
        let mut body = vec![0u8; length];
        stdout.read_exact(&mut body).unwrap();
        String::from_utf8(body).unwrap()
    }

    fn close(mut self) {
        self.ask("shutdown", "null");
        self.notify("exit", "null");
        let status = self.process.wait().unwrap();
        assert!(status.success(), "the language server exited {status}");
    }
}

/// A library of `n` exported functions, each with a context bound its body
/// never uses, under `lead` lines of comment. Each function sits under a
/// comment of multi-byte characters, so a position counted in bytes or from
/// the wrong line start lands elsewhere.
fn unused_bounds(lead: usize, n: usize) -> String {
    let mut text: String = (0..lead).map(|i| format!("// A line above everything, {i}.\n")).collect();
    text.push_str("from \"platform/effect\" import { Allocator };\n\n");
    for i in 0..n {
        text.push_str(&format!(
            "/// Café, naïve, 日本 and 🎉: {i}.\nexport fn p{i}<C: Allocator>(ctx: C, x: Int): Int {{\n    let _ = ctx;\n    x * {i}\n}}\n\n"
        ));
    }
    text
}

/// Where [`unused_bounds`] puts function `i`'s finding: its line, and the
/// columns of `Allocator`.
fn bound_at(lead: usize, i: usize) -> (String, u32, u32, u32) {
    let line = lead + 3 + 6 * i;
    let start = "export fn p".len() + i.to_string().len() + "<C: ".len();
    ("unused-context-bound".to_string(), line as u32, start as u32, (start + "Allocator".len()) as u32)
}

/// That `found` is `want`, naming the first finding where they part rather
/// than printing thousands.
fn same(what: &str, found: &[(String, u32, u32, u32)], want: &[(String, u32, u32, u32)]) {
    if let Some(at) = (0..found.len().max(want.len())).find(|&i| found.get(i) != want.get(i)) {
        panic!(
            "{what}: {} findings published against {} expected, first apart at {at}: {:?} against {:?}",
            found.len(),
            want.len(),
            found.get(at),
            want.get(at)
        );
    }
}

/// **A file with two thousand findings publishes each once, where it is, and
/// after every edit** (PERFORMANCE.md §6.70). Each finding's position is read
/// off the line starts, and a file's findings are deduplicated through a set.
/// The library has a suite of its own, so each finding is filed by two targets
/// and published once. Between requests the server is idle, which is when it
/// gives back the pages the last burst freed.
#[test]
fn two_thousand_findings_in_one_file_are_each_published_once_where_they_are() {
    let scratch = Scratch::repo("many-findings-published");
    let n = 2000;
    scratch.write(
        "lib/big/BUILD.buri",
        "library {\n    visibility: [\"//visibility:public\"]\n\n    test {\n        sources: [\"test/big.buri\"]\n    }\n}\n",
    );
    scratch.write(
        "lib/big/test/big.buri",
        "from \"//lib/big\" import { p7 };\nfrom \"core/testing/assert\" import * as assert;\nfrom \"platform/effect/testing\" import { alloc };\n\n\
         test \"p7 multiplies\" {\n    let ctx = context { Allocator: alloc() };\n    assert.equal(p7(ctx, 3), 21);\n}\n",
    );
    let text = unused_bounds(0, n);
    scratch.write("lib/big/lib.buri", &text);
    let uri = format!("file://{}", scratch.path("lib/big/lib.buri").display());
    let mut server = Server::open(&scratch.root);

    server.text(&uri, 1, &text);
    same("opened", &server.pull(&uri), &(0..n).map(|i| bound_at(0, i)).collect::<Vec<_>>());

    // Three lines above everything move every finding down three.
    server.text(&uri, 2, &unused_bounds(3, n));
    same("three lines in", &server.pull(&uri), &(0..n).map(|i| bound_at(3, i)).collect::<Vec<_>>());

    // The last ten functions gone, and their findings with them.
    server.text(&uri, 3, &unused_bounds(3, n - 10));
    same("ten functions out", &server.pull(&uri), &(0..n - 10).map(|i| bound_at(3, i)).collect::<Vec<_>>());

    // Back to the file on disk, findings and all.
    server.text(&uri, 4, &text);
    same("as on disk", &server.pull(&uri), &(0..n).map(|i| bound_at(0, i)).collect::<Vec<_>>());
    server.close();
}

/// A binary of `n` private functions that never read their `ctx`, each called
/// from `total`, which hands `ctx` on. Every hundredth is also passed to
/// `apply` as a value, so its parameter has to stay.
fn unused_contexts(n: usize) -> String {
    let items: String = (0..n).map(|i| format!("fn p{i}<C: Allocator>(ctx: C, x: Int): Int {{\n    x * {i}\n}}\n\n")).collect();
    let calls: Vec<String> = (0..n).map(|i| format!("p{i}(ctx, {i})")).collect();
    let values: Vec<String> = (0..n).step_by(100).map(|i| format!("apply(ctx, p{i})")).collect();
    format!(
        "from \"platform/effect\" import {{ Allocator, Stdout }};\n\
         from \"node\" import {{ NodeHost }};\n\
         from \"core/io\" import * as io;\n\n\
         {items}\
         fn apply<C: Allocator>(ctx: C, f: fn(C, Int) => Int): Int {{\n    f(ctx, 7)\n}}\n\n\
         fn total<C: Allocator>(ctx: C): Int {{\n    {} +\n        {}\n}}\n\n\
         export fn main(host: NodeHost): Result<(), Str> {{\n    \
         let ctx = context {{ Allocator: host.alloc, Stdout: host.stdout }};\n    \
         io.println(ctx, \"${{total(ctx)}}\").mapErr(fn(_e) => \"no stdout\")\n}}\n",
        calls.join(" +\n        "),
        values.join(" +\n        "),
    )
}

/// **`lint --fix` removes a thousand unused contexts, and the program prints
/// what it printed** (PERFORMANCE.md §6.71). The rule collects its findings
/// first and finds every call site to rewrite in one walk of the package. A
/// function something takes as a value keeps its parameter, and is still
/// reported.
#[test]
fn lint_fix_removes_a_thousand_unused_contexts_and_the_program_still_runs() {
    let scratch = Scratch::repo("many-findings-fixed");
    let n = 1000;
    scratch.write("app/BUILD.buri", "binary {\n    outputs: [\n        { platform: \"node\" },\n    ]\n}\n");
    scratch.write("app/main.buri", &unused_contexts(n));
    let answer = (0..n).map(|i| i * i).sum::<usize>() + (0..n).step_by(100).map(|i| 7 * i).sum::<usize>();

    let before = scratch.run(&["run", "//app"]);
    before.ok();
    assert_eq!(before.stdout, format!("{answer}\n"), "{}", indent(&before.all()));
    let linted = scratch.run(&["lint", "//..."]);
    linted.exits(1);
    assert_eq!(linted.all().matches("[unused-context]").count(), n, "{}", indent(&linted.all()));

    let fixed = scratch.run(&["lint", "--fix", "//..."]);
    assert!(fixed.all().contains(&format!("fixed {} findings", n - n / 100)), "{}", indent(&fixed.all()));
    let source = scratch.read("app/main.buri");
    for i in 0..n {
        let (head, call) = if i % 100 == 0 {
            (format!("fn p{i}<C: Allocator>(ctx: C, x: Int): Int {{"), format!("p{i}(ctx, {i})"))
        } else {
            (format!("fn p{i}<C: Allocator>(x: Int): Int {{"), format!("p{i}({i})"))
        };
        assert!(source.contains(&format!("{head}\n")), "p{i}'s signature after the fix:\n{}", indent(&source));
        assert!(source.contains(&format!("{call} +\n")), "p{i}'s call after the fix:\n{}", indent(&source));
    }

    let after = scratch.run(&["run", "//app"]);
    after.ok();
    assert_eq!(after.stdout, before.stdout, "{}", indent(&after.all()));
    let again = scratch.run(&["lint", "//..."]);
    again.exits(1);
    assert_eq!(again.all().matches("[unused-context]").count(), n / 100, "{}", indent(&again.all()));
}
