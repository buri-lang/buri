//! The live block `buri test` and `buri build` draw below their results while
//! work is running: a line of counts, then a row per build, link, generator or
//! test suite in progress, each with a timer.
//!
//! ```text
//! [Building 41/52 · Testing 18/30 · 1 failed]
//!   test   //libs/sim ......................... 01:32
//!   build  //apps/web/admin ................... 00:03
//!   … and 3 more
//! ```
//!
//! One thread owns the terminal. It redraws the block about ten times a
//! second, and anything else that writes while the block is up does it inside
//! [`hold`], which erases the block first and keeps the thread from drawing
//! until the write is done. So a result never lands inside a half-drawn block.
//!
//! The block is drawn only when standard error or standard output is a
//! terminal. Otherwise nothing is drawn, and work that runs for a minute is
//! named once a minute instead: `still running: test //x (1:00)`.
//!
//! With no reporter started, every call here is one load of a flag.
#![allow(
    clippy::arithmetic_side_effects,
    reason = "counts of suites and rows already in memory, and seconds since the \
              process began; a wrapped figure is a wrong progress line, never a \
              wrong build"
)]

use std::io::Write as _;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering::Relaxed};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// What a row is doing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Build,
    Link,
    Gen,
    Test,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Build => "build",
            Kind::Link => "link",
            Kind::Gen => "gen",
            Kind::Test => "test",
        }
    }
}

/// What a test may hook in: the clock rows are timed by, and a call on each
/// side of a row's start, so a test can hold the work there.
#[derive(Clone, Copy)]
pub struct Hooks {
    pub clock: fn() -> Option<Duration>,
    pub starting: fn(Kind, &str, bool),
}

static HOOKS: OnceLock<Hooks> = OnceLock::new();

/// Installs [`Hooks`]. Only a build for tests does.
pub fn hook(hooks: Hooks) {
    let _ = HOOKS.set(hooks);
}

/// Time since the process began, or the test's own clock.
fn now() -> Duration {
    static BEGAN: OnceLock<Instant> = OnceLock::new();
    let began = *BEGAN.get_or_init(Instant::now);
    HOOKS
        .get()
        .and_then(|h| (h.clock)())
        .unwrap_or_else(|| began.elapsed())
}

fn starting(kind: Kind, label: &str, begun: bool) {
    if let Some(h) = HOOKS.get() {
        (h.starting)(kind, label, begun);
    }
}

/// Which row a call is about: one piece of work, or one test suite by its
/// place in the run.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Key {
    Work(u64),
    Suite(usize),
}

struct Row {
    key: Key,
    kind: Kind,
    label: String,
    /// When it started. `None` while a test holds it before it starts.
    since: Option<Duration>,
    /// How many whole minutes a piped run has said it ran.
    told: u64,
}

struct State {
    rows: Vec<Row>,
    next: u64,
    builds_done: usize,
    builds_total: usize,
    /// Builds queued and not yet finished, by suite or program.
    pending: Vec<usize>,
    /// Suites done and in all, for `buri test`.
    suites: Option<(usize, usize)>,
    failed: usize,
}

static STATE: Mutex<State> = Mutex::new(State {
    rows: Vec::new(),
    next: 0,
    builds_done: 0,
    builds_total: 0,
    pending: Vec::new(),
    suites: None,
    failed: 0,
});

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where the block goes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// To this descriptor, a terminal.
    Live(i32),
    /// Nowhere: `still running` lines on standard error instead.
    Piped,
}

struct Term {
    mode: Mode,
    color: bool,
    /// Lines of the block on the screen now.
    drawn: usize,
}

static TERM: Mutex<Term> = Mutex::new(Term {
    mode: Mode::Piped,
    color: false,
    drawn: 0,
});

fn term() -> MutexGuard<'static, Term> {
    TERM.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether a reporter is running.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Set once the block must never be drawn again: the run is ending.
static STOPPED: AtomicBool = AtomicBool::new(false);

/// The block's lines on the screen and where, for the signal handler, which
/// can't take a lock.
static DRAWN: AtomicUsize = AtomicUsize::new(0);
static DRAWN_FD: AtomicI32 = AtomicI32::new(2);

fn active() -> bool {
    ACTIVE.load(Relaxed)
}

/// The running reporter. Dropping it erases the block and stops the thread.
pub struct Reporter {
    thread: Option<std::thread::JoinHandle<()>>,
}

/// Starts the reporter. `testing` adds the suites' counts to the summary.
pub fn start(color: bool, testing: bool) -> Reporter {
    use std::io::IsTerminal as _;
    let mode = if std::io::stderr().is_terminal() {
        Mode::Live(2)
    } else if std::io::stdout().is_terminal() {
        Mode::Live(1)
    } else {
        Mode::Piped
    };
    {
        let mut s = state();
        s.rows.clear();
        s.pending.clear();
        s.builds_done = 0;
        s.builds_total = 0;
        s.suites = testing.then_some((0, 0));
        s.failed = 0;
    }
    {
        let mut t = term();
        t.mode = mode;
        t.color = color;
        t.drawn = 0;
    }
    if let Mode::Live(fd) = mode {
        DRAWN_FD.store(fd, Relaxed);
        interrupt::listen();
    }
    STOPPED.store(false, Relaxed);
    ACTIVE.store(true, Relaxed);
    let thread = std::thread::Builder::new()
        .name("buri-progress".into())
        .spawn(|| {
            while !STOPPED.load(Relaxed) {
                std::thread::park_timeout(Duration::from_millis(100));
                tick();
            }
        })
        .ok();
    Reporter { thread }
}

impl Drop for Reporter {
    fn drop(&mut self) {
        STOPPED.store(true, Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
        erase(&mut term());
        ACTIVE.store(false, Relaxed);
        interrupt::restore();
        let mut s = state();
        s.rows.clear();
        s.suites = None;
    }
}

/// Erases the block for good, for a run ending in an error: a panic, or an
/// internal error. Never waits, so a panic inside [`hold`] can't hang on it.
pub fn abandon() {
    if !active() {
        return;
    }
    STOPPED.store(true, Relaxed);
    if let Ok(mut t) = TERM.try_lock() {
        erase(&mut t);
    }
}

/// Erases whatever block is on the screen.
fn erase(t: &mut Term) {
    if let (Mode::Live(fd), true) = (t.mode, t.drawn > 0) {
        let text = format!("\r\x1b[{}A\x1b[J", t.drawn);
        write_to(fd, &text);
    }
    t.drawn = 0;
    DRAWN.store(0, Relaxed);
}

fn write_to(fd: i32, text: &str) {
    let _ = if fd == 1 {
        let mut out = std::io::stdout().lock();
        out.write_all(text.as_bytes()).and_then(|()| out.flush())
    } else {
        let mut err = std::io::stderr().lock();
        err.write_all(text.as_bytes()).and_then(|()| err.flush())
    };
}

/// Holds the terminal for one write: the block is erased, and not drawn again
/// until this is dropped. Taken again on a thread that holds it, it does
/// nothing, so a caller and its callee can both take it.
pub fn hold() -> Hold {
    if !active() || HOLDING.with(std::cell::Cell::get) {
        return Hold { term: None };
    }
    let mut t = term();
    erase(&mut t);
    HOLDING.with(|h| h.set(true));
    Hold { term: Some(t) }
}

thread_local! {
    /// Whether this thread is inside a [`hold`].
    static HOLDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// See [`hold`].
pub struct Hold {
    term: Option<MutexGuard<'static, Term>>,
}

impl Drop for Hold {
    fn drop(&mut self) {
        if let Some(term) = self.term.take() {
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();
            HOLDING.with(|h| h.set(false));
            drop(term);
        }
    }
}

/// A piece of work's row, gone when this is dropped.
pub struct Work {
    id: Option<u64>,
}

/// Starts a row for one piece of work.
pub fn begin(kind: Kind, label: &str) -> Work {
    if !active() {
        return Work { id: None };
    }
    let id = {
        let mut s = state();
        s.next += 1;
        let id = s.next;
        s.rows.push(Row {
            key: Key::Work(id),
            kind,
            label: label.to_string(),
            since: None,
            told: 0,
        });
        id
    };
    show(Key::Work(id), kind, label);
    Work { id: Some(id) }
}

/// Makes a reserved row visible, from now, with the test's hooks around it.
fn show(key: Key, kind: Kind, label: &str) {
    starting(kind, label, false);
    let at = now();
    if let Some(row) = state().rows.iter_mut().find(|r| r.key == key) {
        row.since = Some(at);
    }
    starting(kind, label, true);
}

impl Work {
    /// The same work moving on to `kind`, timed from now.
    pub fn set(&mut self, kind: Kind) {
        let Some(id) = self.id else {
            return;
        };
        let label = {
            let mut s = state();
            let Some(row) = s.rows.iter_mut().find(|r| r.key == Key::Work(id)) else {
                return;
            };
            row.kind = kind;
            row.since = None;
            row.told = 0;
            row.label.clone()
        };
        show(Key::Work(id), kind, &label);
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            state().rows.retain(|r| r.key != Key::Work(id));
        }
    }
}

/// A build of suite or program `n` is queued.
pub fn queued(n: usize) {
    if !active() {
        return;
    }
    let mut s = state();
    if !s.pending.contains(&n) {
        s.pending.push(n);
        s.builds_total += 1;
    }
}

/// The build of suite or program `n` is done, if one was queued.
pub fn built(n: usize) {
    if !active() {
        return;
    }
    built_in(&mut state(), n);
}

fn built_in(s: &mut State, n: usize) {
    if let Some(at) = s.pending.iter().position(|&p| p == n) {
        s.pending.swap_remove(at);
        s.builds_done += 1;
    }
}

/// The run holds `total` suites, `done` of them answered already.
pub fn suites(total: usize, done: usize) {
    if !active() {
        return;
    }
    state().suites = Some((done, total));
}

/// Suite `n`'s tests are running. Its row stays until [`answered`].
pub fn testing(n: usize, label: &str) {
    if !active() {
        return;
    }
    {
        let mut s = state();
        built_in(&mut s, n);
        if s.rows.iter().any(|r| r.key == Key::Suite(n)) {
            return;
        }
        s.rows.push(Row {
            key: Key::Suite(n),
            kind: Kind::Test,
            label: label.to_string(),
            since: None,
            told: 0,
        });
    }
    show(Key::Suite(n), Kind::Test, label);
}

/// Suite `n` has its answer.
pub fn answered(n: usize) {
    if !active() {
        return;
    }
    let mut s = state();
    built_in(&mut s, n);
    s.rows.retain(|r| r.key != Key::Suite(n));
    if let Some((done, _)) = &mut s.suites {
        *done += 1;
    }
}

/// Suite `n` goes back to be built again: its row goes, and it isn't done.
pub fn sent_back(n: usize) {
    if !active() {
        return;
    }
    state().rows.retain(|r| r.key != Key::Suite(n));
}

/// How many failures the results above the block hold.
pub fn failed(n: usize) {
    if !active() {
        return;
    }
    state().failed = n;
}

/// One redraw.
fn tick() {
    let mut t = term();
    if STOPPED.load(Relaxed) {
        return;
    }
    let at = now();
    match t.mode {
        Mode::Live(fd) => {
            let (cols, lines) = size(fd);
            let block = render(&state(), at, cols, lines, t.color);
            if block.is_empty() && t.drawn == 0 {
                return;
            }
            let mut text = String::new();
            if t.drawn > 0 {
                text.push_str(&format!("\r\x1b[{}A\x1b[J", t.drawn));
            }
            for line in &block {
                text.push_str(line);
                text.push('\n');
            }
            write_to(fd, &text);
            t.drawn = block.len();
            DRAWN.store(block.len(), Relaxed);
        }
        Mode::Piped => {
            let mut text = String::new();
            for row in &mut state().rows {
                let Some(since) = row.since else {
                    continue;
                };
                let minutes = at.saturating_sub(since).as_secs() / 60;
                if minutes > row.told {
                    row.told = minutes;
                    text.push_str(&format!(
                        "still running: {} {} ({minutes}:00)\n",
                        row.kind.name(),
                        row.label
                    ));
                }
            }
            if !text.is_empty() {
                write_to(2, &text);
            }
        }
    }
}

/// The terminal's columns and lines, or 80 by 24 when it won't say.
fn size(fd: i32) -> (usize, usize) {
    #[repr(C)]
    #[derive(Default)]
    struct Winsize {
        rows: u16,
        cols: u16,
        x: u16,
        y: u16,
    }
    unsafe extern "C" {
        fn ioctl(fd: i32, request: std::ffi::c_ulong, ...) -> i32;
    }
    const TIOCGWINSZ: std::ffi::c_ulong = if cfg!(target_os = "linux") {
        0x5413
    } else {
        0x4008_7468
    };
    let mut w = Winsize::default();
    // SAFETY: `TIOCGWINSZ` writes one `winsize`, which `w` is.
    let ok = unsafe { ioctl(fd, TIOCGWINSZ, &mut w as *mut Winsize) } == 0;
    let cols = if ok && w.cols > 0 { w.cols } else { 80 };
    let rows = if ok && w.rows > 0 { w.rows } else { 24 };
    (usize::from(cols), usize::from(rows))
}

/// Work that has run this long is shown in yellow.
const SLOW: Duration = Duration::from_secs(30);

/// The block's lines for a terminal `cols` wide and `lines` tall: none when no
/// work is running.
fn render(s: &State, at: Duration, cols: usize, lines: usize, color: bool) -> Vec<String> {
    let mut rows: Vec<(&Row, Duration)> =
        s.rows.iter().filter_map(|r| Some((r, r.since?))).collect();
    if rows.is_empty() {
        return Vec::new();
    }
    // Oldest first; `sort_by_key` is stable, so rows started together keep
    // the order they started in.
    rows.sort_by_key(|(_, since)| *since);
    let width = cols.saturating_sub(1).max(1);
    let mut summary = format!("[Building {}/{}", s.builds_done, s.builds_total);
    if let Some((done, total)) = s.suites {
        summary.push_str(&format!(" · Testing {done}/{total}"));
    }
    if s.failed > 0 {
        summary.push_str(&format!(" · {} failed", s.failed));
    }
    summary.push(']');
    let mut out = vec![String::new(), cut(&summary, width)];
    // The block leaves the terminal's last line for the cursor.
    let room = lines.saturating_sub(1).saturating_sub(out.len());
    let (shown, more) = if rows.len() <= room {
        (rows.len(), 0)
    } else if room == 0 {
        (0, 0)
    } else {
        (room - 1, rows.len() - (room - 1))
    };
    // The kind, the label, at least two dots, the timer, and the spaces
    // between them.
    const FIXED: usize = 18;
    let longest = rows
        .iter()
        .take(shown)
        .map(|(r, _)| r.label.chars().count())
        .max()
        .unwrap_or(0);
    let label_width = longest.min(width.saturating_sub(FIXED)).max(1);
    for (row, since) in rows.iter().take(shown) {
        let label = fit(&row.label, label_width);
        let dots = ".".repeat(label_width - label.chars().count() + 2);
        let ran = at.saturating_sub(*since);
        let secs = ran.as_secs();
        let line = format!(
            "  {:<6} {label} {dots} {:02}:{:02}",
            row.kind.name(),
            secs / 60,
            secs % 60
        );
        let line = cut(&line, width);
        if color && ran >= SLOW {
            out.push(format!("\x1b[33m{line}\x1b[0m"));
        } else {
            out.push(line);
        }
    }
    if more > 0 {
        out.push(cut(&format!("  … and {more} more"), width));
    }
    out
}

/// `label` in `width` characters, shortened from the left.
fn fit(label: &str, width: usize) -> String {
    let count = label.chars().count();
    if count <= width {
        return label.to_string();
    }
    let kept: String = label.chars().skip(count - width + 1).collect();
    format!("…{kept}")
}

/// `line` cut to `width` characters.
fn cut(line: &str, width: usize) -> String {
    line.chars().take(width).collect()
}

/// Ctrl-C while the block is up: erase it, then end the way the signal would
/// have ended the process.
mod interrupt {
    use super::{DRAWN, DRAWN_FD, STOPPED};
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

    unsafe extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
        fn raise(sig: i32) -> i32;
        fn write(fd: i32, buf: *const std::ffi::c_void, count: usize) -> isize;
    }

    const SIGINT: i32 = 2;
    const SIG_DFL: usize = 0;

    /// A store, a `write`, `signal` and `raise`, all async-signal-safe. The
    /// sequence is spelled into a buffer on the stack.
    extern "C" fn erased(sig: i32) {
        STOPPED.store(true, Relaxed);
        let n = DRAWN.swap(0, Relaxed);
        if n > 0 {
            let mut buf = [0u8; 32];
            let mut len = 0;
            let mut put = |b: u8| {
                if let Some(slot) = buf.get_mut(len) {
                    *slot = b;
                    len += 1;
                }
            };
            for &b in b"\r\x1b[" {
                put(b);
            }
            let mut digits = [0u8; 20];
            let mut d = 0;
            let mut m = n;
            while m > 0 || d == 0 {
                if let Some(slot) = digits.get_mut(d) {
                    *slot = b'0' + (m % 10) as u8;
                }
                d += 1;
                m /= 10;
            }
            while d > 0 {
                d -= 1;
                put(digits.get(d).copied().unwrap_or(b'0'));
            }
            for &b in b"A\x1b[J" {
                put(b);
            }
            // SAFETY: `len` bytes of `buf`, to the block's descriptor.
            unsafe { write(DRAWN_FD.load(Relaxed), buf.as_ptr().cast(), len) };
        }
        // SAFETY: the default disposition back, then the signal again.
        unsafe {
            signal(sig, SIG_DFL);
            raise(sig);
        }
    }

    const SIG_IGN: usize = 1;

    /// The handler [`listen`] replaced, or `NONE`.
    static BEFORE: AtomicUsize = AtomicUsize::new(NONE);
    const NONE: usize = usize::MAX;

    /// Takes `SIGINT`, unless the process was started with it ignored.
    pub fn listen() {
        // SAFETY: an ordinary `signal` call with a function this module owns.
        let before = unsafe { signal(SIGINT, erased as *const () as usize) };
        if before == SIG_IGN {
            // SAFETY: the ignored disposition back, as it was.
            unsafe { signal(SIGINT, SIG_IGN) };
        } else {
            BEFORE.store(before, Relaxed);
        }
    }

    /// Puts back what [`listen`] replaced.
    pub fn restore() {
        let before = BEFORE.swap(NONE, Relaxed);
        if before != NONE {
            // SAFETY: the handler that was there before, back.
            unsafe { signal(SIGINT, before) };
        }
    }
}
