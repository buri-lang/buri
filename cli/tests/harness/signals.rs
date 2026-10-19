//! Gives every process a test starts the default `SIGINT` and `SIGQUIT`, before
//! `main` runs.
//!
//! A shell starts a background job, `cargo test ... &` or `nohup`, with both
//! ignored, and an ignored signal stays ignored across `exec`. The test
//! process, every `buri` it runs, and every program and child those start
//! would then shrug off a Ctrl-C, so a test that sends one waits for an exit
//! that never comes. A result must not depend on how the suite was started.
//!
//! This is the harness's job and not `buri`'s: a person who runs `buri run`
//! from a background job has asked for a program that ignores `SIGINT`, and
//! that is passed through to it.

use super::hang::{Capped, CappedChild};
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};

unsafe extern "C" {
    fn signal(sig: i32, handler: usize) -> usize;
    fn kill(pid: i32, sig: i32) -> i32;
}

const SIGQUIT: i32 = 3;
const SIGINT: i32 = 2;
const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;

#[used]
#[cfg_attr(target_os = "macos", link_section = "__DATA,__mod_init_func")]
#[cfg_attr(target_os = "linux", link_section = ".init_array")]
static AT_START: extern "C" fn() = default_dispositions;

extern "C" fn default_dispositions() {
    for sig in [SIGINT, SIGQUIT] {
        // SAFETY: `signal` with the default disposition, before any thread exists.
        unsafe { signal(sig, SIG_DFL) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::{Duration, Instant};

    /// Set in the process the test below starts with `SIGINT` ignored.
    const IGNORING: &str = "BURI_TEST_STARTED_IGNORING_SIGINT";

    /// A test process started with `SIGINT` ignored, as a background job
    /// starts one, still ends a child it sends `SIGINT` to.
    #[test]
    fn a_suite_started_ignoring_sigint_still_ends_what_it_interrupts() {
        if std::env::var_os(IGNORING).is_some() {
            let mut child = Command::new("sleep")
                .arg("60")
                .spawn()
                .expect("`sleep` is on PATH");
            let pid = i32::try_from(child.id()).expect("a pid fits an i32");
            // SAFETY: an ordinary `kill` on the child just started.
            unsafe { kill(pid, SIGINT) };
            let deadline = Instant::now() + Duration::from_secs(20);
            let status = loop {
                if let Some(status) = child.try_wait().expect("the child can be waited on") {
                    break status;
                }
                if Instant::now() > deadline {
                    child.kill().ok();
                    child.capped_wait().ok();
                    panic!("the child ignored SIGINT");
                }
                std::thread::sleep(Duration::from_millis(20));
            };
            assert_eq!(status.signal(), Some(SIGINT), "the child ended {status}");
            return;
        }
        let this = module_path!().split_once("::").map_or("", |(_, rest)| rest);
        let mut command = Command::new(std::env::current_exe().expect("this binary has a path"));
        command
            .args([
                "--exact",
                &format!("{this}::a_suite_started_ignoring_sigint_still_ends_what_it_interrupts"),
            ])
            .env(IGNORING, "1");
        // SAFETY: `signal` is async-signal-safe, which is all a `pre_exec` closure may call.
        unsafe {
            command.pre_exec(|| {
                signal(SIGINT, SIG_IGN);
                Ok(())
            });
        }
        let out = command.capped_output().expect("this binary runs again");
        assert!(
            out.status.success(),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
