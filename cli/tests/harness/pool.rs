//! Running a corpus's cases at once, and answering in the order they are in.
//!
//! A repository case is a scratch copy and a handful of `buri` invocations, and
//! almost all of what one costs is waiting on those children. Run one at a time
//! a corpus leaves nine of ten cores idle, which is what took the `ui` corpus to
//! four minutes when the snapshot sweeps took it from nine cases to thirty-five.
//!
//! Two rules make that safe to do, and they are the whole of the design:
//!
//! * **The order is the corpus's, not the scheduler's.** [`map`] answers a
//!   `Vec` indexed like the slice it was given, so a run's failures are
//!   collected and printed in exactly the order a one-at-a-time run printed
//!   them. A case that panics is re-raised after every worker has stopped, and
//!   the one re-raised is the *first* in corpus order — again what a
//!   one-at-a-time run would have reported.
//! * **The width is the run's, not the caller's.** Cargo already runs this
//!   binary's `#[test]`s on their own threads, so fifteen corpora each opening
//!   `available_parallelism` workers would be a hundred and fifty `buri`
//!   processes on a ten-core machine. Every case takes a permit from one gate
//!   instead, so the number in flight is the machine's width however many
//!   corpora are running. The permits are lock files named for the run
//!   ([`super::sweep::run_name`]), because nextest runs each test in a process
//!   of its own and an in-process gate would be one gate per corpus.
//!
//! Nothing else is shared. A case's scratch tree is named for the process and a
//! counter ([`super::Scratch::empty`]), its goldens live in its own directory,
//! and no case in a corpus that comes through here opens a socket.
use std::any::Any;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// How many cases may be running anywhere in this test run at once.
fn width() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

/// The seats every process in a run shares, and this process's count of them.
///
/// A worker that finishes a case asks again at once, and a waiting thread asks
/// every 10 ms, so on asking alone whoever holds the seats keeps them. So a
/// waiting thread also puts itself down in `queue` at its process's seat count,
/// and nobody takes a seat while a process holding fewer waits
/// (`design/PERFORMANCE.md` §6.38).
struct Pool {
    seats: Vec<PathBuf>,
    /// A waiting thread holds a read lock on byte `held << 32 | slot`. `fcntl`
    /// locks belong to the process, so a process never sees its own, and the OS
    /// drops a dead one's.
    queue: File,
    held: AtomicUsize,
    /// This process's threads down in `queue`.
    waiting: AtomicUsize,
}

impl Pool {
    fn at(dir: &Path, width: usize) -> Pool {
        std::fs::create_dir_all(dir).unwrap();
        let queue = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join("queue"))
            .unwrap();
        Pool {
            seats: (0..width).map(|n| dir.join(format!("seat-{n}"))).collect(),
            queue,
            held: AtomicUsize::new(0),
            waiting: AtomicUsize::new(0),
        }
    }

    /// The pool every process in this run shares.
    fn run() -> &'static Pool {
        static RUN: OnceLock<Pool> = OnceLock::new();
        RUN.get_or_init(|| {
            super::sweep::once();
            let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
                .join(format!("pool-{}", super::sweep::run_name()));
            Pool::at(&dir, width())
        })
    }

    /// Whether another process holding fewer than `held` seats is waiting.
    fn someone_poorer_waits(&self, held: usize) -> bool {
        held > 0 && lock::held_by_another(&self.queue, 0, (held as i64) << 32)
    }
}

/// One worker's handles on every seat, opened once: an open per ask cost about
/// 150 CPU-seconds a run in waiting threads. See `README.md`.
struct Seats<'p> {
    pool: &'p Pool,
    handles: Vec<File>,
    /// Unique within the process, so each waiting thread has its own byte.
    slot: i64,
}

impl<'p> Seats<'p> {
    fn open(pool: &'p Pool) -> Self {
        static SLOTS: AtomicU32 = AtomicU32::new(0);
        let handles = pool
            .seats
            .iter()
            .map(|seat| File::options().create(true).append(true).open(seat).unwrap())
            .collect();
        Seats { pool, handles, slot: SLOTS.fetch_add(1, Ordering::Relaxed).into() }
    }

    fn take(&self) -> Permit<'_> {
        let mut queued = Queued { seats: self, byte: None };
        loop {
            let held = self.pool.held.load(Ordering::SeqCst);
            if !self.pool.someone_poorer_waits(held) {
                if let Some(seat) = self.handles.iter().find(|seat| seat.try_lock().is_ok()) {
                    self.pool.held.fetch_add(1, Ordering::SeqCst);
                    return Permit { seat, pool: self.pool };
                }
            }
            queued.at(held);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// A thread's place in `queue`, given up when it gets a seat or unwinds.
struct Queued<'s, 'p> {
    seats: &'s Seats<'p>,
    byte: Option<i64>,
}

impl Queued<'_, '_> {
    /// Down at `held` seats, moving when this process's count has changed.
    fn at(&mut self, held: usize) {
        let byte = (held as i64) << 32 | self.seats.slot;
        if self.byte == Some(byte) {
            return;
        }
        let queue = &self.seats.pool.queue;
        lock::set(queue, lock::READ, byte);
        match self.byte.replace(byte) {
            Some(old) => lock::set(queue, lock::UNLOCK, old),
            None => _ = self.seats.pool.waiting.fetch_add(1, Ordering::SeqCst),
        }
    }
}

impl Drop for Queued<'_, '_> {
    fn drop(&mut self) {
        if let Some(byte) = self.byte {
            lock::set(&self.seats.pool.queue, lock::UNLOCK, byte);
            self.seats.pool.waiting.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// A seat at the machine. Dropping it unlocks the seat, including when the case
/// holding it panics, and the OS releases it if the process dies.
struct Permit<'s> {
    seat: &'s File,
    pool: &'s Pool,
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.seat.unlock().unwrap();
        self.pool.held.fetch_sub(1, Ordering::SeqCst);
    }
}

/// `fcntl` record locks, which std doesn't wrap. Unlike `flock`'s, they belong
/// to the process, and `F_GETLK` asks about a whole byte range in one call.
mod lock {
    use std::fs::File;
    use std::os::unix::io::AsRawFd;

    unsafe extern "C" {
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    }

    #[cfg(target_os = "macos")]
    #[repr(C)]
    struct Flock {
        start: i64,
        len: i64,
        pid: i32,
        kind: i16,
        whence: i16,
    }
    #[cfg(target_os = "macos")]
    const GET: i32 = 7;
    #[cfg(target_os = "macos")]
    const SET: i32 = 8;
    #[cfg(target_os = "macos")]
    pub const READ: i16 = 1;
    #[cfg(target_os = "macos")]
    pub const UNLOCK: i16 = 2;
    #[cfg(target_os = "macos")]
    const WRITE: i16 = 3;

    #[cfg(target_os = "linux")]
    #[repr(C)]
    struct Flock {
        kind: i16,
        whence: i16,
        start: i64,
        len: i64,
        pid: i32,
    }
    #[cfg(target_os = "linux")]
    const GET: i32 = 5;
    #[cfg(target_os = "linux")]
    const SET: i32 = 6;
    #[cfg(target_os = "linux")]
    pub const READ: i16 = 0;
    #[cfg(target_os = "linux")]
    pub const UNLOCK: i16 = 2;
    #[cfg(target_os = "linux")]
    const WRITE: i16 = 1;

    fn call(file: &File, cmd: i32, kind: i16, start: i64, len: i64) -> Flock {
        let mut lock = Flock { kind, whence: 0, start, len, pid: 0 };
        // SAFETY: `lock` is a `struct flock` and outlives the call.
        let status = unsafe { fcntl(file.as_raw_fd(), cmd, &mut lock as *mut Flock) };
        assert_eq!(status, 0, "fcntl: {}", std::io::Error::last_os_error());
        lock
    }

    /// Sets or clears this process's lock on one byte. Never waits: read locks
    /// share, and nobody sets a write lock.
    pub fn set(file: &File, kind: i16, byte: i64) {
        call(file, SET, kind, byte, 1);
    }

    /// Whether another process holds a lock in `start..start + len`.
    pub fn held_by_another(file: &File, start: i64, len: i64) -> bool {
        call(file, GET, WRITE, start, len).kind != UNLOCK
    }
}

/// What one worker came back with: the item's answer, or the panic it took.
type Answer<R> = Result<R, Box<dyn Any + Send>>;

/// `f` over every item, on several threads, answered in the items' own order.
///
/// A panic inside `f` is caught, every other item is still attempted, and the
/// earliest one to panic is re-raised once they have all stopped. So a broken
/// case fails the run with its own message, the way it does when the corpus is
/// walked one case at a time.
pub fn map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    map_in(Pool::run(), items, f)
}

fn map_in<T: Sync, R: Send>(pool: &Pool, items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = pool.seats.len().min(items.len()).max(1);
    let next = Mutex::new(0usize);
    let done: Mutex<Vec<(usize, Answer<R>)>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                let seats = Seats::open(pool);
                loop {
                    let at = {
                        let mut next = next.lock().unwrap_or_else(|e| e.into_inner());
                        let at = *next;
                        *next += 1;
                        at
                    };
                    let Some(item) = items.get(at) else { return };
                    let answer = {
                        let _permit = seats.take();
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(item)))
                    };
                    done.lock().unwrap_or_else(|e| e.into_inner()).push((at, answer));
                }
            });
        }
    });

    let mut answers = done.into_inner().unwrap_or_else(|e| e.into_inner());
    answers.sort_by_key(|(at, _)| *at);
    let mut out = Vec::with_capacity(answers.len());
    for (_, answer) in answers {
        match answer {
            Ok(r) => out.push(r),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }
    out
}

#[cfg(test)]
mod pool_tests {
    use super::*;
    use std::io::{BufRead, Write};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    /// The answers are the items', in the items' order, however the work landed
    /// on the threads.
    #[test]
    fn the_answers_come_back_in_the_order_the_items_were_in() {
        let items: Vec<usize> = (0..200).collect();
        assert_eq!(map(&items, |n| n * 2), items.iter().map(|n| n * 2).collect::<Vec<_>>());
    }

    /// Never more cases in flight than the machine is wide, which is the claim
    /// the gate exists to make.
    #[test]
    fn no_more_cases_run_at_once_than_there_are_permits() {
        static LIVE: AtomicUsize = AtomicUsize::new(0);
        static PEAK: AtomicUsize = AtomicUsize::new(0);
        let items: Vec<usize> = (0..64).collect();
        map(&items, |_| {
            let live = LIVE.fetch_add(1, Ordering::SeqCst) + 1;
            PEAK.fetch_max(live, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(2));
            LIVE.fetch_sub(1, Ordering::SeqCst);
        });
        assert!(
            PEAK.load(Ordering::SeqCst) <= width(),
            "{} cases ran at once on a machine {} wide",
            PEAK.load(Ordering::SeqCst),
            width()
        );
    }

    /// The first case to panic *in the corpus's order* is the one the run
    /// reports, whichever thread reached its own panic first.
    #[test]
    fn the_earliest_panic_is_the_one_raised() {
        let items: Vec<usize> = (0..40).collect();
        let raised = std::panic::catch_unwind(|| {
            map(&items, |n| {
                if *n == 7 || *n == 23 {
                    panic!("case {n} is broken");
                }
            })
        })
        .expect_err("a panicking case fails the run");
        let said = raised.downcast_ref::<String>().map(String::as_str).unwrap_or_default();
        assert_eq!(said, "case 7 is broken");
    }

    /// Set in the busy process the test below starts: the pool it fills.
    const BUSY: &str = "BURI_POOL_BUSY";

    #[derive(Debug, PartialEq)]
    enum Seen {
        BusyTook,
        WaiterTook,
    }

    /// A process waiting for a seat gets the next one a busy process frees,
    /// rather than the busy process taking it back. Counted in seats handed
    /// out, not time: the busy process is this test binary, run again, holding
    /// both seats of a private pool and ending one case per line it reads.
    #[test]
    fn a_waiting_process_gets_the_next_seat_a_busy_one_frees() {
        if let Some(dir) = std::env::var_os(BUSY) {
            return busy(Path::new(&dir));
        }
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("pool-fairness-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let pool = &Pool::at(&dir, 2);
        let this = module_path!().split_once("::").unwrap().1;
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("{this}::a_waiting_process_gets_the_next_seat_a_busy_one_frees")])
            .arg("--nocapture")
            .env(BUSY, &dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut ends = child.stdin.take().unwrap();
        let out = child.stdout.take().unwrap();
        let (seen, events) = mpsc::channel();
        let took = seen.clone();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                if line.starts_with("took") && took.send(Seen::BusyTook).is_err() {
                    return;
                }
            }
        });
        // Only a hang guard: the order of events is the assertion.
        let next = || events.recv_timeout(Duration::from_secs(120)).expect("the pool hung");
        assert_eq!((next(), next()), (Seen::BusyTook, Seen::BusyTook));

        let (release, released) = mpsc::channel::<()>();
        let released = Mutex::new(released);
        let mut taken_back = 0;
        std::thread::scope(|scope| {
            scope.spawn(|| {
                map_in(pool, &[()], |_| {
                    seen.send(Seen::WaiterTook).unwrap();
                    released.lock().unwrap().recv().unwrap();
                })
            });
            while pool.waiting.load(Ordering::SeqCst) == 0 {
                std::thread::sleep(Duration::from_millis(1));
            }
            while taken_back < 20 {
                writeln!(ends).unwrap();
                match next() {
                    Seen::BusyTook => taken_back += 1,
                    Seen::WaiterTook => break,
                }
            }
            // Lets both sides finish, whatever happened.
            drop(ends);
            release.send(()).unwrap();
        });
        assert!(child.wait().unwrap().success());
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(taken_back, 0, "the busy process took back {taken_back} seats while another waited");
    }

    /// Fills both seats of the pool at `dir` and keeps them filled, ending one
    /// case per line on stdin, every case at once when stdin closes.
    fn busy(dir: &Path) {
        let items: Vec<usize> = (0..100).collect();
        map_in(&Pool::at(dir, 2), &items, |n| {
            println!("took {n}");
            std::io::stdin().read_line(&mut String::new()).unwrap();
        });
    }
}
