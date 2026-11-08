//! The live block `buri test` and `buri build` draw below their results while
//! they run: a line of counts, then a row per build, link, generator or test
//! suite in progress, each with a timer.
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
//! The block is drawn only when standard error is a terminal. Otherwise
//! nothing is drawn, and work that runs for a minute is named once a minute
//! instead: `still running: test //x (1:00)`.
//!
//! With no reporter started, every call here is one load of a flag.
#![allow(
    clippy::arithmetic_side_effects,
    reason = "counts of suites and rows already in memory, and seconds since the \
              process began; a wrapped figure is a wrong progress line, never a \
              wrong build"
)]

use std::io::Write as _;
use std::num::NonZeroU16;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
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

fn hooks() -> Hooks {
    HOOKS.get().copied().unwrap_or(Hooks {
        clock: || None,
        starting: |_, _, _| {},
    })
}

/// Time since the process began, or the test's own clock.
fn now() -> Duration {
    static BEGAN: OnceLock<Instant> = OnceLock::new();
    let began = *BEGAN.get_or_init(Instant::now);
    (hooks().clock)().unwrap_or_else(|| began.elapsed())
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
    /// Whether this is `buri test`, whose summary counts suites.
    testing: bool,
    /// Suites done and in all.
    suites: (usize, usize),
    failed: usize,
}

static STATE: Mutex<State> = Mutex::new(State {
    rows: Vec::new(),
    next: 0,
    builds_done: 0,
    builds_total: 0,
    pending: Vec::new(),
    testing: false,
    suites: (0, 0),
    failed: 0,
});

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The terminal, which the thread draws under and [`hold`] holds.
struct Term {
    /// Whether standard error is a terminal. If not, the block isn't drawn.
    live: bool,
    color: bool,
}

static TERM: Mutex<Term> = Mutex::new(Term {
    live: false,
    color: false,
});

fn term() -> MutexGuard<'static, Term> {
    TERM.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether a reporter is running.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Set once the block must never be drawn again: the run is ending.
static STOPPED: AtomicBool = AtomicBool::new(false);

/// Lines of the block on the screen now. An atomic, so the signal handler,
/// which can't take a lock, can erase them.
static DRAWN: AtomicUsize = AtomicUsize::new(0);

fn active() -> bool {
    ACTIVE.load(Relaxed)
}

/// The running reporter. Dropping it erases the block and stops the thread.
pub struct Reporter {
    _running: (),
}

/// Starts the reporter. `testing` adds the suites' counts to the summary.
pub fn start(color: bool, testing: bool) -> Reporter {
    use std::io::IsTerminal as _;
    {
        let mut s = state();
        s.rows.clear();
        s.pending.clear();
        s.builds_done = 0;
        s.builds_total = 0;
        s.testing = testing;
        s.suites = (0, 0);
        s.failed = 0;
    }
    {
        let mut t = term();
        t.live = std::io::stderr().is_terminal();
        t.color = color;
    }
    DRAWN.store(0, Relaxed);
    interrupt::listen();
    STOPPED.store(false, Relaxed);
    ACTIVE.store(true, Relaxed);
    std::thread::spawn(|| {
        while !STOPPED.load(Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
            tick();
        }
    });
    Reporter { _running: () }
}

impl Drop for Reporter {
    fn drop(&mut self) {
        STOPPED.store(true, Relaxed);
        // Under the terminal's lock, so a redraw that began before the stop
        // is finished first; every redraw after it sees the stop.
        let _t = term();
        erase();
        ACTIVE.store(false, Relaxed);
        interrupt::restore();
        state().rows.clear();
    }
}

/// Erases the block for good, for a run ending in an error: a panic, or an
/// internal error. Takes no lock, so a panic inside [`hold`] can't hang on it.
pub fn abandon() {
    STOPPED.store(true, Relaxed);
    erase();
}

unsafe extern "C" {
    fn write(fd: i32, buf: *const std::ffi::c_void, count: usize) -> isize;
}

/// Erases whatever block is on the screen. Async-signal-safe: an atomic swap
/// and a `write`.
fn erase() {
    let (bytes, len) = erasing(DRAWN.swap(0, Relaxed));
    // SAFETY: at most all of `bytes`, to standard error.
    unsafe { write(2, bytes.as_ptr().cast(), len.min(bytes.len())) };
}

/// The bytes that erase a block `n` lines tall, below 1000, and how many of
/// them to write: none when `n` is 0. A count may have leading zeros.
fn erasing(n: usize) -> ([u8; 10], usize) {
    let digit = |d: usize| b'0' + u8::try_from(d % 10).unwrap_or(0);
    let bytes = [
        b'\r',
        0x1b,
        b'[',
        digit(n / 100),
        digit(n / 10),
        digit(n),
        b'A',
        0x1b,
        b'[',
        b'J',
    ];
    (bytes, bytes.len() * usize::from(n > 0))
}

/// Holds the terminal for one write: the block is erased, and not drawn again
/// until this is dropped. Taken again on a thread that holds it, it does
/// nothing, so a caller and its callee can both take it.
pub fn hold() -> Hold {
    if !active() || HOLDING.with(std::cell::Cell::get) {
        return Hold { term: None };
    }
    let t = term();
    erase();
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
    let starting = hooks().starting;
    starting(kind, label, false);
    let at = now();
    state()
        .rows
        .iter_mut()
        .filter(|r| r.key == key)
        .for_each(|r| r.since = Some(at));
    starting(kind, label, true);
}

impl Work {
    /// The same work moving on to `kind`, timed from now.
    pub fn set(&mut self, kind: Kind) {
        let Some(id) = self.id else {
            return;
        };
        let label = state()
            .rows
            .iter_mut()
            .find(|r| r.key == Key::Work(id))
            .map(|row| {
                row.kind = kind;
                row.since = None;
                row.told = 0;
                row.label.clone()
            })
            .unwrap_or_default();
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
    state().suites = (done, total);
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
    s.suites.0 += 1;
}

/// Suite `n` goes back to be built again: its row goes, and it isn't done.
pub fn sent_back(n: usize) {
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
    let t = term();
    if STOPPED.load(Relaxed) {
        return;
    }
    let at = now();
    if t.live {
        let (cols, lines) = size();
        let block = render(&state(), at, cols, lines, t.color);
        let (erasing, len) = erasing(DRAWN.load(Relaxed));
        let mut text = erasing.get(..len).unwrap_or_default().to_vec();
        for line in &block {
            text.extend_from_slice(line.as_bytes());
            text.push(b'\n');
        }
        let _ = std::io::stderr().lock().write_all(&text);
        DRAWN.store(block.len(), Relaxed);
    } else {
        let mut text = String::new();
        for row in &mut state().rows {
            let minutes = row
                .since
                .map_or(0, |since| at.saturating_sub(since).as_secs() / 60);
            if minutes > row.told {
                row.told = minutes;
                text.push_str(&format!(
                    "still running: {} {} ({minutes}:00)\n",
                    row.kind.name(),
                    row.label
                ));
            }
        }
        let _ = std::io::stderr().lock().write_all(text.as_bytes());
    }
}

/// Standard error's columns and lines, or 80 by 24 when it won't say. The
/// lines are kept between 4, room for one row, and 999, which [`erasing`]
/// can spell.
fn size() -> (usize, usize) {
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
    // SAFETY: `TIOCGWINSZ` writes one `winsize`, which `w` is. On failure `w`
    // stays zero.
    unsafe { ioctl(2, TIOCGWINSZ, &mut w as *mut Winsize) };
    let cols = NonZeroU16::new(w.cols).map_or(80, NonZeroU16::get);
    let rows = NonZeroU16::new(w.rows).map_or(24, NonZeroU16::get);
    (usize::from(cols), usize::from(rows).clamp(4, 999))
}

/// Work that has run this long is shown in yellow.
const SLOW: Duration = Duration::from_secs(30);

/// The block's lines for a terminal `cols` wide and `lines` tall, at least 4:
/// none when no work is running.
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
    if s.testing {
        let (done, total) = s.suites;
        summary.push_str(&format!(" · Testing {done}/{total}"));
    }
    if s.failed > 0 {
        summary.push_str(&format!(" · {} failed", s.failed));
    }
    summary.push(']');
    let mut out = vec![String::new(), cut(&summary, width)];
    // The block leaves the terminal's last line for the cursor, so there is
    // room for at least one line below the summary.
    let room = lines - 1 - out.len();
    let shown = if rows.len() <= room {
        rows.len()
    } else {
        room - 1
    };
    let more = rows.len() - shown;
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

/// Ctrl-C while the block is up: erase it, then let the signal do what it
/// did before the reporter started.
mod interrupt {
    use super::{erase, STOPPED};
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

    unsafe extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
        fn raise(sig: i32) -> i32;
    }

    const SIGINT: i32 = 2;

    /// The handler [`listen`] replaced, at first the default one.
    static BEFORE: AtomicUsize = AtomicUsize::new(0);

    /// A store, an erase, `signal` and `raise`, all async-signal-safe.
    extern "C" fn erased(sig: i32) {
        STOPPED.store(true, Relaxed);
        erase();
        // SAFETY: the handler from before back, then the signal again.
        unsafe {
            signal(sig, BEFORE.load(Relaxed));
            raise(sig);
        }
    }

    pub fn listen() {
        // SAFETY: an ordinary `signal` call with a function this module owns.
        let before = unsafe { signal(SIGINT, erased as *const () as usize) };
        BEFORE.store(before, Relaxed);
    }

    /// Puts back what [`listen`] replaced.
    pub fn restore() {
        // SAFETY: the handler that was there before, back.
        unsafe { signal(SIGINT, BEFORE.load(Relaxed)) };
    }
}
