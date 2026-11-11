//! The live block `buri test` and `buri build` draw at a terminal while work
//! runs, and the `still running` lines they print anywhere else.
//!
//! Each test runs `buri` on a pseudo-terminal of a size it picks, and reads
//! what reached the screen through a small model of one: text, the cursor
//! moving up, lines erased, and yellow. Time is the test's: `buri` reads its
//! clock from `<BURI_HOLD>/clock-now`, and work is held at the moment its row
//! starts (`cli/src/hold.rs`), so no test waits on a real second.
use crate::harness::hang::CappedChild;
use crate::harness::*;
use crate::held::{Held, Holding};
use std::io::Read as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A package `lib/<name>` with one suite whose test asserts `answer() == want`.
fn suite(scratch: &Scratch, name: &str, want: i64) {
    scratch.write(
        &format!("lib/{name}/BUILD.buri"),
        "library {\n  test { sources: [\"test/t.buri\"] }\n}\n",
    );
    scratch.write(
        &format!("lib/{name}/lib.buri"),
        "export fn answer(): I64 { 1 }\n",
    );
    scratch.write(
        &format!("lib/{name}/test/t.buri"),
        &format!(
            "from \"//lib/{name}\" import {{ answer }};\n\
             from \"core/testing/assert\" import * as assert;\n\
             \ntest \"{name} answers\" {{\n  assert.equal(answer(), {want});\n}}\n"
        ),
    );
}

/// Sets the clock `buri` times rows by, in whole seconds.
fn clock(held: &Held, secs: u64) {
    let fresh = held.dir().join("clock-next");
    std::fs::write(&fresh, secs.to_string()).unwrap();
    // A rename, so `buri` never reads half a number.
    std::fs::rename(fresh, held.dir().join("clock-now")).unwrap();
}

/// `buri` running on a pseudo-terminal.
struct Terminal {
    child: std::process::Child,
    bytes: Arc<Mutex<Vec<u8>>>,
    reader: Option<std::thread::JoinHandle<()>>,
}

unsafe extern "C" {
    fn posix_openpt(flags: i32) -> i32;
    fn grantpt(fd: i32) -> i32;
    fn unlockpt(fd: i32) -> i32;
    fn ptsname(fd: i32) -> *const std::ffi::c_char;
    fn ioctl(fd: i32, request: std::ffi::c_ulong, ...) -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
}

#[repr(C)]
struct Winsize {
    rows: u16,
    cols: u16,
    x: u16,
    y: u16,
}

impl Terminal {
    /// Starts `buri <args>` in `scratch` on a terminal `cols` wide and `rows`
    /// tall, with standard input, output and error all on it.
    fn start(
        held: &Held,
        scratch: &Scratch,
        args: &[&str],
        env: &[(&str, &str)],
        (cols, rows): (u16, u16),
    ) -> Terminal {
        use std::os::fd::{AsRawFd as _, FromRawFd as _};
        use std::os::unix::fs::OpenOptionsExt as _;
        const O_RDWR: i32 = 2;
        const O_NOCTTY: i32 = if cfg!(target_os = "linux") {
            0o400
        } else {
            0x20000
        };
        const TIOCSWINSZ: std::ffi::c_ulong = if cfg!(target_os = "linux") {
            0x5414
        } else {
            0x8008_7467
        };
        // SAFETY: plain calls on a descriptor this test owns; `ptsname`'s
        // string is copied before anything else can call it.
        let (main, name) = unsafe {
            let fd = posix_openpt(O_RDWR | O_NOCTTY);
            assert!(fd >= 0, "posix_openpt failed");
            assert_eq!(grantpt(fd), 0, "grantpt failed");
            assert_eq!(unlockpt(fd), 0, "unlockpt failed");
            let name = std::ffi::CStr::from_ptr(ptsname(fd))
                .to_string_lossy()
                .into_owned();
            (std::fs::File::from_raw_fd(fd), name)
        };
        let side = || {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(O_NOCTTY)
                .open(&name)
                .expect("the terminal's other side opens")
        };
        let first = side();
        let size = Winsize {
            rows,
            cols,
            x: 0,
            y: 0,
        };
        // SAFETY: `TIOCSWINSZ` reads one `winsize`, which `size` is.
        let set = unsafe { ioctl(first.as_raw_fd(), TIOCSWINSZ, &size as *const Winsize) };
        assert_eq!(set, 0, "{}", std::io::Error::last_os_error());
        let mut command = buri_command();
        command
            .args(args)
            .env_remove("NO_COLOR")
            .envs(env.iter().copied())
            .env("TERM", "xterm")
            .current_dir(&scratch.root)
            .env("BURI_HOLD", held.dir())
            .stdin(first)
            .stdout(side())
            .stderr(side());
        let child = command.spawn().expect("`buri` starts");
        // The test's copies of the other side close here, so a read ends once
        // `buri` and its children have closed theirs.
        drop(command);
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&bytes);
        let reader = std::thread::spawn(move || {
            let mut main = main;
            let mut buf = [0u8; 4096];
            while let Ok(n @ 1..) = main.read(&mut buf) {
                into.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        });
        Terminal {
            child,
            bytes,
            reader: Some(reader),
        }
    }

    /// Every byte that has reached the terminal.
    fn raw(&self) -> String {
        String::from_utf8_lossy(&self.bytes.lock().unwrap()).into_owned()
    }

    /// The screen now.
    fn screen(&self) -> Screen {
        Screen::of(&self.raw())
    }

    /// Waits until the screen satisfies `ok`, and answers it.
    fn until(&self, what: &str, ok: impl Fn(&Screen) -> bool) -> Screen {
        const DEADLINE: Duration = Duration::from_secs(240);
        let started = Instant::now();
        loop {
            let screen = self.screen();
            if ok(&screen) {
                return screen;
            }
            assert!(
                started.elapsed() < DEADLINE,
                "the screen never showed {what} within {DEADLINE:?}:\n{}",
                indent(&screen.text())
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Waits until the screen shows `line`, and answers the screen.
    fn shows(&self, line: &str) -> Screen {
        self.until(&format!("{line:?}"), |s| {
            s.lines().iter().any(|l| l == line)
        })
    }

    /// Waits for `buri` to end, and answers its status and the final screen.
    fn finish(mut self) -> (std::process::ExitStatus, Screen) {
        let status = self.child.capped_wait().unwrap();
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
        (status, self.screen())
    }
}

/// What a terminal shows after some bytes: its lines, and which characters on
/// them are yellow.
struct Screen {
    rows: Vec<Vec<(char, bool)>>,
}

impl Screen {
    fn of(raw: &str) -> Screen {
        let mut rows: Vec<Vec<(char, bool)>> = vec![Vec::new()];
        let (mut row, mut col) = (0usize, 0usize);
        let mut yellow = false;
        let mut chars = raw.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\r' => col = 0,
                '\n' => {
                    row += 1;
                    col = 0;
                    if rows.len() <= row {
                        rows.push(Vec::new());
                    }
                }
                '\u{1b}' => {
                    assert_eq!(chars.next(), Some('['), "an escape that isn't CSI");
                    let mut params = String::new();
                    let end = loop {
                        let c = chars.next().expect("an escape cut short");
                        if c.is_ascii_digit() || c == ';' {
                            params.push(c);
                        } else {
                            break c;
                        }
                    };
                    match end {
                        'A' => row = row.saturating_sub(params.parse().unwrap_or(1)),
                        'J' => {
                            rows[row].truncate(col);
                            rows.truncate(row + 1);
                        }
                        'K' => rows[row].truncate(col),
                        'm' => {
                            for p in params.split(';') {
                                match p {
                                    "33" => yellow = true,
                                    "" | "0" | "39" => yellow = false,
                                    _ => {}
                                }
                            }
                        }
                        other => panic!("an escape this model doesn't know: {other:?}"),
                    }
                }
                c => {
                    let line = &mut rows[row];
                    while line.len() < col {
                        line.push((' ', false));
                    }
                    if col < line.len() {
                        line[col] = (c, yellow);
                    } else {
                        line.push((c, yellow));
                    }
                    col += 1;
                }
            }
        }
        while rows.last().is_some_and(Vec::is_empty) {
            rows.pop();
        }
        Screen { rows }
    }

    fn lines(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|r| r.iter().map(|(c, _)| c).collect())
            .collect()
    }

    fn text(&self) -> String {
        self.lines().join("\n")
    }

    /// Whether the line reading `line` is yellow from end to end.
    fn yellow(&self, line: &str) -> bool {
        let at = self.lines().iter().position(|l| l == line);
        let at = at.unwrap_or_else(|| panic!("no line {line:?}:\n{}", indent(&self.text())));
        self.rows[at].iter().all(|(_, y)| *y)
    }

    /// Whether any line of the block is on the screen.
    fn has_block(&self) -> bool {
        self.lines()
            .iter()
            .any(|l| l.starts_with("[Building") || l.starts_with("  test   "))
    }
}

/// A terminal wide and tall enough for every row.
const ROOMY: (u16, u16) = (100, 30);

/// Skips a test where this host can't build native tests.
fn native() -> bool {
    if let Some(gap) = ci::native_host_gap() {
        ci::missing("build::progress", &gap);
        return false;
    }
    true
}

/// While a suite runs, the block shows the counts and the suite's row, its
/// timer counting up; once the run ends the block is gone and the summary is
/// the last thing on the screen.
#[test]
fn a_running_suite_has_a_row_whose_timer_counts_up() {
    if !native() {
        return;
    }
    let scratch = Scratch::repo("progress-timer");
    suite(&scratch, "a", 1);
    let held = Held::new("progress-timer");
    clock(&held, 0);
    held.hold("begun", "test-lib_a");
    let term = Terminal::start(&held, &scratch, &["test"], &[], ROOMY);
    let running = held.reached("begun", "test-lib_a");
    let screen = term.shows("  test   //lib/a .. 00:00");
    assert!(
        screen
            .lines()
            .iter()
            .any(|l| l == "[Building 1/1 · Testing 0/1]"),
        "{}",
        indent(&screen.text())
    );
    clock(&held, 95);
    term.shows("  test   //lib/a .. 01:35");
    running.go();
    let (status, screen) = term.finish();
    assert!(status.success(), "{}", indent(&screen.text()));
    assert!(!screen.has_block(), "{}", indent(&screen.text()));
    let piped = Scratch::repo("progress-timer-piped");
    suite(&piped, "a", 1);
    let run = piped.run(&["test"]);
    let last = strip_ansi(&run.ok().all())
        .lines()
        .last()
        .map(blank_seconds);
    assert_eq!(
        screen.lines().last().map(|l| blank_seconds(l)),
        last,
        "{}",
        indent(&screen.text())
    );
}

/// `line` with every time in seconds, such as the summary's `(0.4s)`, blanked.
fn blank_seconds(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let digits = |from: usize| {
            chars[from..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .count()
        };
        let whole = digits(i);
        if whole > 0 && chars.get(i + whole) == Some(&'.') {
            let part = digits(i + whole + 1);
            if part > 0 && chars.get(i + whole + 1 + part) == Some(&'s') {
                out.push_str("<s>");
                i += whole + 1 + part + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// A row turns yellow at thirty seconds, and never without colour.
#[test]
fn a_row_turns_yellow_at_thirty_seconds() {
    if !native() {
        return;
    }
    for (name, args, env) in [
        ("colour", &["test"][..], &[][..]),
        ("never", &["test", "--color=never"][..], &[][..]),
        ("no-color", &["test"][..], &[("NO_COLOR", "1")][..]),
    ] {
        let scratch = Scratch::repo(&format!("progress-yellow-{name}"));
        suite(&scratch, "a", 1);
        let held = Held::new(&format!("progress-yellow-{name}"));
        clock(&held, 29);
        held.hold("begun", "test-lib_a");
        let term = Terminal::start(&held, &scratch, args, env, ROOMY);
        let running = held.reached("begun", "test-lib_a");
        clock(&held, 29 + 29);
        let screen = term.shows("  test   //lib/a .. 00:29");
        assert!(!screen.yellow("  test   //lib/a .. 00:29"), "{name}");
        clock(&held, 29 + 30);
        let screen = term.shows("  test   //lib/a .. 00:30");
        assert_eq!(
            screen.yellow("  test   //lib/a .. 00:30"),
            name == "colour",
            "{name}"
        );
        if name != "colour" {
            assert!(!term.raw().contains("\u{1b}[33m"), "{name}");
        }
        running.go();
        let (status, _) = term.finish();
        assert!(status.success(), "{name}");
    }
}

/// More rows than the terminal has lines for end in `… and N more`.
#[test]
fn rows_past_the_terminal_height_are_counted_instead() {
    if !native() {
        return;
    }
    let scratch = Scratch::repo("progress-height");
    let held = Held::new("progress-height");
    clock(&held, 0);
    for name in ["a", "b", "c", "d"] {
        suite(&scratch, name, 1);
        held.hold("begun", format!("test-lib_{name}"));
    }
    let term = Terminal::start(&held, &scratch, &["test", "--jobs=4"], &[], (100, 5));
    let running: Vec<Holding> = ["a", "b", "c", "d"]
        .iter()
        .map(|name| held.reached("begun", format!("test-lib_{name}")))
        .collect();
    let screen = term.shows("  … and 3 more");
    let rows = screen
        .lines()
        .iter()
        .filter(|l| l.starts_with("  test   "))
        .count();
    assert_eq!(rows, 1, "{}", indent(&screen.text()));
    for holding in running {
        holding.go();
    }
    let (status, screen) = term.finish();
    assert!(status.success(), "{}", indent(&screen.text()));
    assert!(!screen.has_block(), "{}", indent(&screen.text()));
}

/// A label too long for the terminal keeps its end.
#[test]
fn a_long_label_is_shortened_from_the_left() {
    if !native() {
        return;
    }
    let scratch = Scratch::repo("progress-width");
    suite(&scratch, "averylongname/inner", 1);
    let held = Held::new("progress-width");
    clock(&held, 0);
    held.hold("begun", "test-lib_averylongname_inner");
    let term = Terminal::start(&held, &scratch, &["test"], &[], (30, 24));
    let running = held.reached("begun", "test-lib_averylongname_inner");
    term.shows("  test   …name/inner .. 00:00");
    running.go();
    let (status, screen) = term.finish();
    assert!(status.success(), "{}", indent(&screen.text()));
}

/// Rows are listed oldest first, whatever order their suites are in.
#[test]
fn rows_are_oldest_first() {
    if !native() {
        return;
    }
    let scratch = Scratch::repo("progress-order");
    suite(&scratch, "a", 1);
    suite(&scratch, "b", 1);
    let held = Held::new("progress-order");
    clock(&held, 0);
    for name in ["a", "b"] {
        held.hold("begin", format!("test-lib_{name}"));
        held.hold("begun", format!("test-lib_{name}"));
    }
    let term = Terminal::start(&held, &scratch, &["test", "--jobs=4"], &[], ROOMY);
    held.at("begin", "test-lib_b", || {});
    let b = held.reached("begun", "test-lib_b");
    clock(&held, 10);
    term.shows("  test   //lib/b .. 00:10");
    held.at("begin", "test-lib_a", || {});
    let a = held.reached("begun", "test-lib_a");
    let screen = term.shows("  test   //lib/a .. 00:00");
    let rows: Vec<String> = screen
        .lines()
        .into_iter()
        .filter(|l| l.starts_with("  test   "))
        .collect();
    assert_eq!(
        rows,
        ["  test   //lib/b .. 00:10", "  test   //lib/a .. 00:00"],
        "{}",
        indent(&screen.text())
    );
    a.go();
    b.go();
    let (status, screen) = term.finish();
    assert!(status.success(), "{}", indent(&screen.text()));
}

/// A failure is reported above the block as it would be anywhere else, and
/// the counts say one failed.
#[test]
fn a_failure_prints_above_the_block_whole() {
    if !native() {
        return;
    }
    let scratch = Scratch::repo("progress-failure");
    suite(&scratch, "a", 2);
    suite(&scratch, "b", 1);
    let held = Held::new("progress-failure");
    clock(&held, 0);
    held.hold("begun", "test-lib_b");
    let term = Terminal::start(&held, &scratch, &["test", "--jobs=4"], &[], ROOMY);
    let b = held.reached("begun", "test-lib_b");
    let screen = term.shows("[Building 2/2 · Testing 1/2 · 1 failed]");
    assert!(
        screen
            .lines()
            .iter()
            .any(|l| l == "  test   //lib/b .. 00:00"),
        "{}",
        indent(&screen.text())
    );
    b.go();
    let (status, screen) = term.finish();
    assert!(!status.success(), "{}", indent(&screen.text()));
    assert!(!screen.has_block(), "{}", indent(&screen.text()));
    let piped = Scratch::repo("progress-failure-piped");
    suite(&piped, "a", 2);
    suite(&piped, "b", 1);
    let run = piped.run(&["test", "--jobs=4"]);
    let normal = |text: &str| -> Vec<String> {
        strip_ansi(text)
            .replace(piped.root.to_str().unwrap(), "<root>")
            .replace(scratch.root.to_str().unwrap(), "<root>")
            .lines()
            .map(|l| blank_seconds(&blank_test_time(l)))
            .collect()
    };
    let mut want = normal(&run.stdout);
    want.extend(normal(&run.stderr));
    want.sort();
    let mut got = normal(&screen.text());
    got.sort();
    assert_eq!(got, want, "{}", indent(&screen.text()));
}

/// Ctrl-C while the block is up leaves no block behind.
#[test]
fn ctrl_c_erases_the_block() {
    if !native() {
        return;
    }
    let scratch = Scratch::repo("progress-ctrl-c");
    suite(&scratch, "a", 1);
    let held = Held::new("progress-ctrl-c");
    clock(&held, 0);
    held.hold("begun", "test-lib_a");
    let term = Terminal::start(&held, &scratch, &["test"], &[], ROOMY);
    let running = held.reached("begun", "test-lib_a");
    term.shows("  test   //lib/a .. 00:00");
    // SAFETY: a signal to the child this test started.
    assert_eq!(unsafe { kill(term.child.id() as i32, 2) }, 0);
    let (status, screen) = term.finish();
    drop(running);
    assert!(!status.success());
    assert!(!screen.has_block(), "{}", indent(&screen.text()));
}

/// Piped, nothing is drawn, and work that runs a minute is named once a
/// minute.
#[test]
fn piped_output_names_work_still_running_each_minute() {
    if !native() {
        return;
    }
    let scratch = Scratch::repo("progress-piped");
    suite(&scratch, "a", 1);
    let held = Held::new("progress-piped");
    clock(&held, 0);
    held.hold("begun", "test-lib_a");
    let mut child = held.start(&scratch, &["test"]);
    let running = held.reached("begun", "test-lib_a");
    let stderr = || std::fs::read_to_string(held.dir().join("stderr")).unwrap();
    let until = |line: &str| {
        let started = Instant::now();
        while !stderr().lines().any(|l| l == line) {
            assert!(
                started.elapsed() < Duration::from_secs(240),
                "never said {line:?}:\n{}",
                held.printed()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    clock(&held, 59);
    clock(&held, 61);
    until("still running: test //lib/a (1:00)");
    clock(&held, 125);
    until("still running: test //lib/a (2:00)");
    running.go();
    assert!(child.capped_wait().unwrap().success(), "{}", held.printed());
    let printed = held.printed();
    assert!(!printed.contains('\u{1b}'), "{printed}");
    assert!(!printed.contains("[Building"), "{printed}");
    let said: Vec<&str> = printed
        .lines()
        .filter(|l| l.contains("still running"))
        .collect();
    assert_eq!(said.len(), 2, "{printed}");
}

/// Piped, a run that takes under a minute prints what it always did.
#[test]
fn piped_output_is_unchanged() {
    if !native() {
        return;
    }
    let scratch = Scratch::repo("progress-piped-quick");
    suite(&scratch, "a", 2);
    let run = scratch.run(&["test", "--color=never"]);
    let all = run.all();
    assert!(!all.contains('\u{1b}'), "{all}");
    assert!(!all.contains("still running"), "{all}");
    assert!(!all.contains("[Building"), "{all}");
}

/// A generator running is a `gen` row naming its tool and the package it
/// generates for.
#[test]
fn a_generator_has_a_row() {
    if !native() {
        return;
    }
    let scratch = Scratch::repo("progress-gen");
    scratch.write(
        "lib/proto/BUILD.buri",
        "library {\n  generators: [{ tool: \"proto\", inputs: [\"p.proto\"] }]\n  test { sources: [\"test/t.buri\"] }\n}\n",
    );
    scratch.write(
        "lib/proto/p.proto",
        "edition = \"2026\";\n\nmessage M {\n  int64 x = 1;\n}\n",
    );
    scratch.write(
        "lib/proto/lib.buri",
        "from \"//lib/proto/p.proto\" export { M };\n\nexport fn answer(): I64 { 1 }\n",
    );
    scratch.write(
        "lib/proto/test/t.buri",
        "from \"//lib/proto\" import { answer };\nfrom \"core/testing/assert\" import * as assert;\n\ntest \"answers\" {\n  assert.equal(answer(), 1);\n}\n",
    );
    let held = Held::new("progress-gen");
    clock(&held, 0);
    held.hold("begun", "gen-proto_lib_proto");
    let term = Terminal::start(&held, &scratch, &["test"], &[], ROOMY);
    let running = held.reached("begun", "gen-proto_lib_proto");
    term.until("a gen row", |s| {
        s.lines()
            .iter()
            .any(|l| l.starts_with("  gen    proto → //lib/proto ..") && l.ends_with("00:00"))
    });
    running.go();
    let (status, screen) = term.finish();
    assert!(status.success(), "{}", indent(&screen.text()));
    assert!(!screen.has_block(), "{}", indent(&screen.text()));
}

/// A suite's build and its link each have a row.
#[test]
fn a_build_and_a_link_have_rows() {
    if !native() {
        return;
    }
    let scratch = Scratch::repo("progress-build");
    suite(&scratch, "a", 1);
    let held = Held::new("progress-build");
    clock(&held, 0);
    held.hold("begun", "build-lib_a");
    held.hold("begun", "link-lib_a");
    let term = Terminal::start(&held, &scratch, &["test"], &[], ROOMY);
    held.at("begun", "build-lib_a", || {
        term.shows("  build  //lib/a .. 00:00");
    });
    held.at("begun", "link-lib_a", || {
        term.shows("  link   //lib/a .. 00:00");
    });
    let (status, screen) = term.finish();
    assert!(status.success(), "{}", indent(&screen.text()));
}

/// Between one piece of work ending and the next starting, nothing is running
/// and no block is on the screen.
#[test]
fn the_block_goes_while_nothing_runs() {
    if !native() {
        return;
    }
    let scratch = Scratch::repo("progress-between");
    suite(&scratch, "a", 1);
    let held = Held::new("progress-between");
    clock(&held, 0);
    held.hold("begun", "link-lib_a");
    held.hold("begin", "test-lib_a");
    let term = Terminal::start(&held, &scratch, &["test"], &[], ROOMY);
    held.at("begun", "link-lib_a", || {
        term.shows("  link   //lib/a .. 00:00");
    });
    held.at("begin", "test-lib_a", || {
        term.until("no block", |s| !s.has_block());
    });
    let (status, screen) = term.finish();
    assert!(status.success(), "{}", indent(&screen.text()));
}

/// `buri build` draws the block too, without a suite count.
#[test]
fn buri_build_shows_its_builds() {
    if !native() {
        return;
    }
    let scratch = Scratch::repo("progress-buri-build");
    scratch.binary_package(
        "app",
        "from \"node\" import { NodeHost };\n\n\
         export fn main(host: NodeHost): Result<(), Str> { .Ok(()) }\n",
    );
    let held = Held::new("progress-buri-build");
    clock(&held, 0);
    held.hold("begun", "build-app");
    let term = Terminal::start(&held, &scratch, &["build"], &[], ROOMY);
    held.at("begun", "build-app", || {
        let screen = term.shows("  build  //app .. 00:00");
        assert!(
            screen.lines().iter().any(|l| l == "[Building 0/1]"),
            "{}",
            indent(&screen.text())
        );
    });
    let (status, screen) = term.finish();
    assert!(status.success(), "{}", indent(&screen.text()));
    assert!(!screen.has_block(), "{}", indent(&screen.text()));
}
