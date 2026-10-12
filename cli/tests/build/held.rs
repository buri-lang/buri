//! `BURI_HOLD` from a test's side (`cli/src/hold.rs`): `buri` waits at a point
//! until the test lets it go, so the test decides a race and the scheduler
//! doesn't.

use crate::harness::{buri_command, indent, Scratch};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::Duration;

/// A directory of held points, outside the repository so no sweep lists it.
pub struct Held {
    scratch: Scratch,
}

impl Held {
    pub fn new(name: &str) -> Held {
        Held { scratch: Scratch::empty(&format!("{name}-held")) }
    }

    /// What `BURI_HOLD` is set to.
    pub fn dir(&self) -> &Path {
        &self.scratch.root
    }

    /// Makes `buri`'s `n`th visit to `point` wait for [`Held::at`].
    pub fn hold(&self, point: &str, n: usize) {
        unsafe extern "C" {
            fn mkfifo(path: *const std::ffi::c_char, mode: u32) -> i32;
        }
        let name = std::ffi::CString::new(self.file(point, n).as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a NUL-terminated path, and a mode.
        assert_eq!(unsafe { mkfifo(name.as_ptr(), 0o644) }, 0, "mkfifo failed");
    }

    /// Hands `buri`'s `n`th visit to `point` the bytes `signals`, without
    /// waiting: each is a signal it raises there.
    pub fn raise(&self, point: &str, n: usize, signals: &[u8]) {
        std::fs::write(self.file(point, n), signals).unwrap();
    }

    /// Hands `buri`'s `n`th visit to `point` the text `value`, without waiting.
    pub fn set(&self, point: &str, n: usize, value: &str) {
        std::fs::write(self.file(point, n), value).unwrap();
    }

    /// Waits until `buri` is held at `point`'s `n`th visit, runs `between`, and
    /// lets it go on.
    pub fn at(&self, point: &str, n: usize, between: impl FnOnce()) {
        let holding = self.reached(point, n);
        between();
        holding.go();
    }

    /// Waits until `buri` is held at `point`'s `n`th visit, and keeps it there
    /// until [`Holding::go`].
    pub fn reached(&self, point: &str, n: usize) -> Holding {
        use std::os::unix::fs::OpenOptionsExt as _;
        const O_NONBLOCK: i32 = if cfg!(target_os = "macos") { 0x4 } else { 0o4000 };
        const DEADLINE: Duration = Duration::from_secs(240);
        let path = self.file(point, n);
        // A writer that won't wait is refused until `buri` has the FIFO open.
        let started = std::time::Instant::now();
        loop {
            if let Ok(file) = std::fs::OpenOptions::new().write(true).custom_flags(O_NONBLOCK).open(&path) {
                return Holding(file);
            }
            assert!(started.elapsed() < DEADLINE, "`buri` never reached {point} {n} within {DEADLINE:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Starts `buri <args>` in `scratch`, held here, its output in files here.
    pub fn start(&self, scratch: &Scratch, args: &[&str]) -> Child {
        self.start_with_env(scratch, args, &[])
    }

    /// The same, with variables added to its environment.
    pub fn start_with_env(&self, scratch: &Scratch, args: &[&str], env: &[(&str, &str)]) -> Child {
        let mut command = buri_command();
        command.envs(env.iter().copied());
        command
            .args(args)
            .current_dir(&scratch.root)
            .env("BURI_HOLD", self.dir())
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(self.dir().join("stdout")).unwrap())
            .stderr(std::fs::File::create(self.dir().join("stderr")).unwrap());
        command.spawn().expect("`buri` starts")
    }

    /// What a run [`Held::start`]ed printed.
    pub fn printed(&self) -> String {
        let read = |name: &str| std::fs::read_to_string(self.dir().join(name)).unwrap_or_default();
        indent(&format!("{}{}", read("stdout"), read("stderr")))
    }

    fn file(&self, point: &str, n: usize) -> PathBuf {
        self.scratch.path(&format!("{point}-{n}"))
    }
}

/// `buri` waiting at a held point.
pub struct Holding(std::fs::File);

impl Holding {
    /// Lets `buri` go on.
    pub fn go(mut self) {
        // The byte lets `buri` go (`cli/src/hold.rs`).
        std::io::Write::write_all(&mut self.0, b"\n").expect("the held point takes its byte");
    }
}
