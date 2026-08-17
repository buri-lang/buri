//! `buri test --watch`: the declared input set, the poller, and the loop.
//!
//! There is no file watcher here, and that is the design rather than a gap
//! (`design/native/BUILD-AND-WATCH.md` §1.2). The build already enumerates
//! every file a key is computed from — it has to, in order to hash them — so
//! the set of files worth watching is a declared, enumerated, usually
//! few-hundred-file list rather than a directory tree that has to be crawled
//! and filtered. Polling that list with `stat` costs a syscall per file per
//! sweep and buys two properties a real watcher has to work for:
//!
//!   * **Self-trigger immunity.** A build writes into `.buri/`, which is not a
//!     declared input of anything, so nothing the toolchain writes can wake
//!     the loop. The one mode that does write to the source tree — `--accept`
//!     — is refused alongside `--watch` in `arguments::parse`.
//!   * **`.git/`, `target/` and `node_modules/` are not watched**, without an
//!     ignore list, because they are not inputs anybody declared.
//!
//! The incrementality is the cache's rather than the loop's. A pass re-runs the
//! whole invocation; a suite whose inputs did not move is served from the cache
//! and costs a hash of its sources, and a suite whose inputs moved is re-run.
//! The machinery that decides what to re-run under `--watch` is therefore the
//! same machinery that decides it without, which is the only way to be sure the
//! two agree.
#![allow(
    clippy::print_stdout,
    reason = "the run separator and a pass's own output are this command's output; \
              every diagnostic about the code still leaves through `Session::emit`"
)]
#![allow(
    clippy::arithmetic_side_effects,
    reason = "the arithmetic here is a pass counter, a column count for the run \
              separator, and a wall clock reduced modulo a day by literal \
              divisors — none of it takes a length or an offset from a file the \
              user wrote"
)]

use crate::build::session::Session;
use crate::build::workspace::{RuleKind, TargetId};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How often the declared set is swept, and the settle window, which is one
/// further quiet sweep.
///
/// So a save is acted on between 150 and 300 ms after it lands, and a sequence
/// of writes — a formatter rewriting twelve files, a `git checkout` — is
/// coalesced into one pass. It is not configurable: this is a sweep interval
/// rather than a debounce, and a flag here is a flag nobody could choose a
/// value for (BUILD-AND-WATCH.md §4.3).
pub const SWEEP: Duration = Duration::from_millis(150);

// ---------------------------------------------------------------------------
// What is watched
// ---------------------------------------------------------------------------

/// Every file a `buri test` invocation's keys are computed from, plus the two
/// kinds of file that decide what those keys even are.
///
/// Per selected target, the union of:
///
///   * every path `actions::contribute` enumerates for every member of the
///     target's closure — the rule's entry point, its `sources`, its
///     `proto_sources`, and its `testing/` sources;
///   * every path `actions::test_key` enumerates — the suite's `sources`, its
///     `data`, and the closure of every library its `test { dependencies }` and
///     `testing { dependencies }` name;
///   * every `BUILD.buri` in the repository;
///   * the repository's `REPO.buri`.
///
/// The first two are exactly the inputs the keys are computed from, so a change
/// that does not move a key does not exist as far as the loop is concerned. The
/// last two are in no key's input list but change the graph itself — a new
/// dependency edge, a new source, a changed tag vocabulary — and a pass opens a
/// fresh `Session`, so a change to either is picked up whole.
///
/// Every `BUILD.buri` rather than the closure's, which is one file per package
/// and the difference between a loop that can be fixed and one that cannot: a
/// build file that stops parsing takes its package out of the graph, so a set
/// built from the closure would no longer contain the file whose repair is the
/// thing being waited for.
///
/// Sorted and deduplicated: two targets in one repository share most of their
/// closure, and the same file must not be swept twice.
pub fn inputs(s: &Session, targets: &[TargetId]) -> Vec<PathBuf> {
    let mut out: BTreeSet<PathBuf> = BTreeSet::new();
    out.insert(s.root.join("REPO.buri"));
    for id in s.ws.ids() {
        out.insert(s.ws.pkg(id).build_path.clone());
    }
    for &target in targets {
        for member in s.ws.closure(target) {
            declared_sources(s, member, &mut out);
        }
        // The libraries the *test* code reaches, which are not in the closure —
        // a test dependency is not a dependency of the thing being shipped — and
        // are compiled into the suite all the same. `actions::test_key` adds
        // exactly this set, and the two enumerations are the same enumeration
        // or the loop serves a verdict for code it stopped watching.
        for (dep, _) in s.ws.test_dep_edges(target) {
            for member in s.ws.closure(dep) {
                declared_sources(s, member, &mut out);
            }
        }
        // The suite's own inputs, which are on the selected target rather than
        // on its closure: a dependency's test sources are not built by this
        // run and are not in this run's key.
        let pkg = s.ws.pkg(target.pkg);
        let suite = match target.kind {
            RuleKind::Library => pkg.build.library.as_ref().and_then(|l| l.test.as_ref()),
            RuleKind::Binary => pkg.build.binary.as_ref().and_then(|b| b.test.as_ref()),
        };
        if let Some(suite) = suite {
            for x in suite.sources.iter().chain(&suite.data) {
                out.insert(pkg.dir.join(&x.value));
            }
        }
    }
    out.into_iter().collect()
}

/// Every file one rule declares, which is every file `actions::contribute`
/// hashes into that rule's part of a key. One function, so that "the watch set
/// mirrors the key" is a shared enumeration rather than two lists that have to
/// be kept in step by hand.
fn declared_sources(s: &Session, member: TargetId, out: &mut BTreeSet<PathBuf>) {
    let pkg = s.ws.pkg(member.pkg);
    out.insert(pkg.build_path.clone());
    let dir = &pkg.dir;
    match member.kind {
        RuleKind::Library => {
            out.insert(dir.join("lib.buri"));
            if let Some(lib) = &pkg.build.library {
                for x in lib.sources.iter().chain(&lib.proto_sources) {
                    out.insert(dir.join(&x.value));
                }
                if let Some(testing) = &lib.testing {
                    out.insert(dir.join("testing/lib.buri"));
                    for x in &testing.sources {
                        out.insert(dir.join(&x.value));
                    }
                }
            }
        }
        RuleKind::Binary => {
            out.insert(dir.join("main.buri"));
            if let Some(bin) = &pkg.build.binary {
                for x in bin.sources.iter().chain(&bin.proto_sources) {
                    out.insert(dir.join(&x.value));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The poller
// ---------------------------------------------------------------------------

/// What one `stat` says about one declared input.
///
/// A file that is not there has a stamp too, rather than being absent from the
/// map: deleting a source a `BUILD.buri` still names is a change the loop must
/// see, and so is putting it back.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Stamp {
    /// `None` when the file does not exist, or when the filesystem has no
    /// modification time to give — in which case the length carries the whole
    /// of the signal, which is the honest degradation.
    modified: Option<SystemTime>,
    len: u64,
    present: bool,
}

fn stamp(path: &Path) -> Stamp {
    match std::fs::metadata(path) {
        Ok(m) => Stamp { modified: m.modified().ok(), len: m.len(), present: true },
        Err(_) => Stamp { modified: None, len: 0, present: false },
    }
}

/// One sweep of the declared set.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct Snapshot(BTreeMap<PathBuf, Stamp>);

impl Snapshot {
    /// Stats every path. The order is the map's, so two sweeps of one set
    /// compare field by field with no sorting.
    pub fn sweep(paths: &[PathBuf]) -> Snapshot {
        Snapshot(paths.iter().map(|p| (p.clone(), stamp(p))).collect())
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        self.0.keys().cloned().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The paths whose stamp differs, sorted. A path only one side names counts
    /// as changed: a source that appeared or a source that went away is exactly
    /// the thing that has to wake the loop.
    pub fn difference(&self, other: &Snapshot) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for (path, stamp) in &other.0 {
            if self.0.get(path) != Some(stamp) {
                out.push(path.clone());
            }
        }
        for path in self.0.keys() {
            if !other.0.contains_key(path) {
                out.push(path.clone());
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Adopts a new declared set, keeping the stamp already held for a path
    /// that is in both and stating a new one for a path that has just been
    /// declared.
    ///
    /// The stamp for an old path is kept rather than re-taken, so that an edit
    /// that landed *while* a pass was running is still seen on the next sweep
    /// rather than being absorbed by the rebase. An empty set is not adopted:
    /// that is what a pass whose `Session` would not open returns, and forgetting
    /// what to watch would mean the loop could never see the file that fixes it.
    pub fn rebase(&mut self, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        let mut next = BTreeMap::new();
        for path in paths {
            let held = self.0.get(path).cloned();
            next.insert(path.clone(), held.unwrap_or_else(|| stamp(path)));
        }
        self.0 = next;
    }
}

// ---------------------------------------------------------------------------
// The loop
// ---------------------------------------------------------------------------

/// What one pass of the loop did, as the loop needs to see it.
///
/// The output is a string rather than something already printed because of one
/// rule: **a pass with nothing to do prints nothing at all**. Whether there was
/// anything to say is only known once the pass has run, and a watch mode that
/// prints on every sweep trains you not to read it (BUILD-AND-WATCH.md §4.4).
pub struct Pass {
    pub code: i32,
    /// The declared set this pass computed, for the next sweep. Empty when the
    /// pass could not open a repository, in which case the previous set stands.
    pub inputs: Vec<PathBuf>,
    /// Failures, accepted diffs, and the summary line — exactly what the pass
    /// would have printed without `--watch`.
    pub output: String,
    /// Every suite was served from the cache and every one passed, so there is
    /// nothing to report and the loop stays silent.
    pub quiet: bool,
}

/// Why a pass is happening: which one it is, and what moved.
pub struct Trigger {
    /// 1 for the pass the loop opens with, which no edit triggered.
    pub pass: usize,
    /// The declared inputs whose stamp moved, repository-absolute.
    pub changed: Vec<PathBuf>,
}

/// The loop, with the three things a test needs to inject.
pub struct Watch {
    /// How often the declared set is swept. [`SWEEP`] outside the tests.
    pub interval: Duration,
    /// Stop after this many passes. `None` outside the tests: a watch session
    /// ends when the terminal it is attached to signals it.
    pub passes: Option<usize>,
    /// Whether to draw the run separator. This is the injected "is a terminal"
    /// flag: production always sets it, because `arguments::parse` refuses
    /// `--watch` without a terminal, and the headless tests clear it so that
    /// what a pass printed is the whole of what they read.
    pub interactive: bool,
    /// Whether the separator is drawn before the pass rather than after it.
    /// `--explain` streams one line per action as the pass runs, so a separator
    /// printed afterwards would sit underneath the transcript it heads.
    pub header_first: bool,
    /// Changed paths are shown relative to this.
    pub root: PathBuf,
}

impl Watch {
    /// The production loop: sweep at [`SWEEP`], run until signalled, draw the
    /// separator.
    pub fn on(root: PathBuf, explain: bool) -> Watch {
        Watch {
            interval: SWEEP,
            passes: None,
            interactive: true,
            header_first: explain,
            root,
        }
    }

    /// Runs `pass` once, then again every time the declared set moves.
    ///
    /// The exit status is the last pass's, which is what the tests read. It is
    /// not what a shell reports: a watch session ends when the terminal signals
    /// it, and the status of a signalled process is the signal's. The design
    /// asks for a clean exit 0 on Ctrl-C, and that needs a signal handler, which
    /// needs `libc` — a dependency the bar in the workspace manifest does not
    /// admit for a status code. So the exit status of a watch session says
    /// nothing about the suites, `cli/src/docs/cli/test.md` says so, and `buri
    /// test` without the flag is what a script branches on.
    pub fn drive<F: FnMut(&Trigger) -> Pass>(&self, mut pass: F) -> i32 {
        let mut watched = Snapshot::default();
        let mut changed: Vec<PathBuf> = Vec::new();
        let mut n: usize = 1;
        loop {
            let trigger = Trigger { pass: n, changed: std::mem::take(&mut changed) };
            let header = self.separator(&trigger);
            if self.header_first {
                self.say(&header);
            }
            let result = pass(&trigger);
            if !result.quiet {
                if !self.header_first {
                    self.say(&header);
                }
                print!("{}", result.output);
            }
            // The one window in the loop, and it is only the opening pass's:
            // the set is not known until a pass has computed it, so the first
            // stamps are taken after the first pass rather than before it, and
            // an edit made *during* that pass is one the loop already accounted
            // for. Every pass after it keeps the stamps it already held, so an
            // edit that lands mid-run is seen on the next sweep.
            watched.rebase(&result.inputs);
            if self.passes.is_some_and(|k| n >= k) || watched.is_empty() {
                return result.code;
            }
            changed = self.settle(&mut watched);
            n += 1;
        }
    }

    fn say(&self, line: &str) {
        if self.interactive {
            println!("{line}");
        }
    }

    /// Sweeps until something moves, then until two consecutive sweeps agree,
    /// and answers with what moved across the whole disturbance.
    fn settle(&self, watched: &mut Snapshot) -> Vec<PathBuf> {
        let paths = watched.paths();
        loop {
            std::thread::sleep(self.interval);
            let moved = Snapshot::sweep(&paths);
            if moved == *watched {
                continue;
            }
            // The settle window. A save is one write, but a formatter, a branch
            // switch, or an editor that writes through a temporary file is many,
            // and they are one edit as far as a person is concerned.
            let mut quiet = moved;
            loop {
                std::thread::sleep(self.interval);
                let again = Snapshot::sweep(&paths);
                if again == quiet {
                    break;
                }
                quiet = again;
            }
            let changed = watched.difference(&quiet);
            *watched = quiet;
            return changed;
        }
    }

    /// The one line that separates two runs: the time it was triggered, which
    /// run it is, and what moved.
    ///
    /// ```text
    /// ── 14:02:31Z  run 3  lib/money/cents.buri ───────────────────
    /// ```
    ///
    /// Composed before the pass rather than after it, so the time on it is the
    /// time the edit was acted on rather than the time the suites finished —
    /// which is the number that tells you how long you waited.
    ///
    /// The screen is not cleared. Scrollback is where the failure you are
    /// fixing is, and the previous run's output is the thing you are comparing
    /// against (BUILD-AND-WATCH.md §4.4).
    fn separator(&self, t: &Trigger) -> String {
        const WIDTH: usize = 64;
        let mut line = format!("── {}  run {}", clock(), t.pass);
        let shown: Vec<String> =
            t.changed.iter().take(2).map(|p| self.relative(p)).collect();
        if !shown.is_empty() {
            line.push_str("  ");
            line.push_str(&shown.join(" "));
            if t.changed.len() > shown.len() {
                line.push_str(&format!(" +{} more", t.changed.len() - shown.len()));
            }
        }
        line.push(' ');
        let drawn = line.chars().count();
        if drawn < WIDTH {
            for _ in drawn..WIDTH {
                line.push('─');
            }
        }
        line
    }

    fn relative(&self, p: &Path) -> String {
        p.strip_prefix(&self.root).unwrap_or(p).display().to_string()
    }
}

/// The wall clock as `HH:MM:SSZ`, UTC.
///
/// UTC, and it says so, because the toolchain carries no timezone database and
/// will not grow one for a header line. A local time that is silently wrong is
/// worse than a UTC one that is labelled.
fn clock() -> String {
    let Ok(since) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) else {
        return "--:--:--Z".to_string();
    };
    let day = since.as_secs() % 86_400;
    format!("{:02}:{:02}:{:02}Z", day / 3_600, day % 3_600 / 60, day % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("buri-watch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// A file that is not there is a stamp rather than an absence, so a source
    /// a `BUILD.buri` still names going away is a change.
    #[test]
    fn an_appearing_and_a_vanishing_file_are_both_changes() {
        let dir = scratch("presence");
        let path = dir.join("a.buri");
        let paths = vec![path.clone()];

        let before = Snapshot::sweep(&paths);
        assert!(before.difference(&before).is_empty(), "a sweep differs from itself");

        let _ = std::fs::write(&path, "x");
        let present = Snapshot::sweep(&paths);
        assert_eq!(before.difference(&present), paths);

        let _ = std::fs::remove_file(&path);
        let gone = Snapshot::sweep(&paths);
        assert_eq!(present.difference(&gone), paths);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An edit is seen as a moved modification time or a moved length.
    ///
    /// The length is what is asserted, because a modification time's resolution
    /// belongs to the filesystem and this is not a test about HFS+. The wider
    /// claim the poller makes is deliberately coarser than the cache's: a stat
    /// cannot tell content apart, so a `git checkout` that restores a file's
    /// original bytes wakes the loop. That is not a bug — the run it triggers is
    /// served entirely from the cache and prints nothing, which is exactly why
    /// the loop is allowed to be this cheap.
    #[test]
    fn an_edit_is_seen_as_a_moved_length() {
        let dir = scratch("rewrite");
        let path = dir.join("a.buri");
        let paths = vec![path.clone()];
        let _ = std::fs::write(&path, "one");
        let before = Snapshot::sweep(&paths);
        // A length change, so that a filesystem with second-granularity
        // timestamps is not what this is measuring.
        let _ = std::fs::write(&path, "one two");
        assert_eq!(before.difference(&Snapshot::sweep(&paths)), paths);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A rebase keeps what it already knew, so an edit that landed while a pass
    /// was running is still seen; and it never adopts an empty set, because that
    /// is what a repository that stopped loading returns.
    #[test]
    fn a_rebase_keeps_old_stamps_and_refuses_an_empty_set() {
        let dir = scratch("rebase");
        let old = dir.join("a.buri");
        let new = dir.join("b.buri");
        let _ = std::fs::write(&old, "one");
        let mut watched = Snapshot::sweep(std::slice::from_ref(&old));

        // The edit that lands during the pass.
        let _ = std::fs::write(&old, "one two");
        watched.rebase(&[old.clone(), new.clone()]);
        let changed = watched.difference(&Snapshot::sweep(&[old.clone(), new.clone()]));
        assert_eq!(changed, vec![old.clone()], "a rebase swallowed an edit");

        let held = watched.paths();
        watched.rebase(&[]);
        assert_eq!(watched.paths(), held, "an empty set was adopted");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The separator carries the three things a reader needs and nothing else.
    #[test]
    fn the_separator_names_the_pass_the_time_and_what_moved() {
        let w = Watch {
            interval: SWEEP,
            passes: None,
            interactive: true,
            header_first: false,
            root: PathBuf::from("/repo"),
        };
        let first = w.separator(&Trigger { pass: 1, changed: Vec::new() });
        assert!(first.starts_with("── "), "{first}");
        assert!(first.contains("run 1"), "{first}");
        assert!(first.ends_with('─'), "{first}");

        let third = w.separator(&Trigger {
            pass: 3,
            changed: vec![
                PathBuf::from("/repo/lib/a/lib.buri"),
                PathBuf::from("/repo/lib/b/lib.buri"),
                PathBuf::from("/repo/lib/c/lib.buri"),
            ],
        });
        assert!(third.contains("lib/a/lib.buri"), "{third}");
        assert!(third.contains("+1 more"), "{third}");
        assert!(!third.contains("/repo/"), "paths are repository-relative: {third}");
    }

    /// The loop runs its opening pass without anything having changed, and the
    /// pass count is what ends it.
    #[test]
    fn the_opening_pass_has_no_trigger() {
        let dir = scratch("opening");
        let path = dir.join("a.buri");
        let _ = std::fs::write(&path, "one");
        let w = Watch {
            interval: Duration::from_millis(5),
            passes: Some(1),
            interactive: false,
            header_first: false,
            root: dir.clone(),
        };
        let mut seen = Vec::new();
        let code = w.drive(|t| {
            seen.push((t.pass, t.changed.len()));
            Pass { code: 7, inputs: vec![path.clone()], output: String::new(), quiet: true }
        });
        assert_eq!(code, 7);
        assert_eq!(seen, vec![(1, 0)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An edit that lands while the loop is sweeping wakes it, and the next
    /// pass is told which file it was.
    ///
    /// The edit is scheduled rather than made inline, and that is the shape of
    /// the thing rather than a trick: the loop stamps the declared set the
    /// moment a pass hands it back, so an edit made *before* that stamp has
    /// already been accounted for. What has to be tested is an edit that lands
    /// after it.
    #[test]
    fn an_edit_while_sweeping_triggers_the_next_pass() {
        let dir = scratch("edit");
        let path = dir.join("a.buri");
        let _ = std::fs::write(&path, "one");
        let w = Watch {
            interval: Duration::from_millis(5),
            passes: Some(2),
            interactive: false,
            header_first: false,
            root: dir.clone(),
        };
        let mut triggers: Vec<Vec<String>> = Vec::new();
        w.drive(|t| {
            triggers.push(t.changed.iter().map(|p| w.relative(p)).collect());
            if t.pass == 1 {
                let scheduled = path.clone();
                let _ = std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(60));
                    let _ = std::fs::write(&scheduled, "one two three");
                });
            }
            Pass { code: 0, inputs: vec![path.clone()], output: String::new(), quiet: true }
        });
        assert_eq!(triggers.len(), 2, "the loop did not run a second pass");
        assert!(triggers[0].is_empty());
        assert_eq!(triggers[1], vec!["a.buri".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A pass with nothing to say prints nothing, and one with something to say
    /// prints the separator with it.
    #[test]
    fn a_quiet_pass_is_silent_and_a_loud_one_is_not() {
        // The claim is about what `drive` decides rather than about what
        // reaches the terminal, so it is read off the two branches directly:
        // `quiet` suppresses the separator and the output together.
        let dir = scratch("quiet");
        let path = dir.join("a.buri");
        let _ = std::fs::write(&path, "one");
        let w = Watch {
            interval: Duration::from_millis(5),
            passes: Some(1),
            interactive: true,
            header_first: false,
            root: dir.clone(),
        };
        let code = w.drive(|_| Pass {
            code: 1,
            inputs: vec![path.clone()],
            output: "0 passed, 1 failed, 0 skipped (0.0s)\n".into(),
            quiet: false,
        });
        assert_eq!(code, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
