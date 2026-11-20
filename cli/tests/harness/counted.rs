//! A child's instructions, less the kernel's work each preemption adds.
//!
//! macOS's `ri_instructions` counts the kernel's work on the process too, and
//! there's no user-only count without root. Each involuntary context switch
//! adds about [`PER_PREEMPTION`], and a loaded machine switches often: a 771 M
//! run read 790 M after 1,701 switches, and 771 M once they were taken off.

use super::hang::Bounds;
use std::process::{Child, Output};

/// The instructions one involuntary context switch adds: the slope of
/// `ri_instructions` against `ru_nivcsw`, 10.8 to 11.4 k over runs with up to
/// 1,701 switches, on an M3 Pro under macOS 26.6.
pub const PER_PREEMPTION: u64 = 11_000;

/// What `child` wrote and how it exited, as [`Child::wait_with_output`], and
/// the instructions it retired less [`PER_PREEMPTION`] for each time it was
/// preempted. `None` where the kernel counts nothing: every platform but macOS,
/// and macOS in a virtual machine, such as CI's.
///
/// The child must have been spawned with its output piped. The wait is capped
/// as [`super::hang::CappedChild`]'s are, with a tighter ceiling on processor
/// time: [`WORK_SHARE`].
pub fn counted_output(child: Child) -> (Output, Option<u64>) {
    let standard = Bounds::idle(super::hang::cap());
    let work = standard.work / WORK_SHARE;
    counted_output_within(child, Bounds { work, ..standard })
}

/// The share of the standard ceiling on processor time a counted program gets:
/// 20 minutes / 40 = 30 s, or 20 times that under Valgrind.
///
/// The wait had no cap at all, so a counted program that never finished held
/// its test until the job's runner died, and nothing named it: the suspected
/// cause of nightlies 44670440615 and 44693285711 losing the macOS training
/// runner. Of the 576 counted runs in `native` and `language` with the LLVM
/// backend, on 2026-11-20 on an M3 Pro, the most any used was 0.35 s of
/// processor time (a `bun` run) and the longest took 1.2 s. 30 s is 85 times
/// the first, enough for a slower runner, and a program looping for ever fails
/// half a minute in rather than 20 minutes. A program asleep is caught by the
/// standard idle cap.
const WORK_SHARE: u32 = 40;

/// [`counted_output`] under `bounds`.
#[cfg(target_os = "macos")]
pub fn counted_output_within(mut child: Child, bounds: Bounds) -> (Output, Option<u64>) {
    use std::os::unix::process::ExitStatusExt;
    unsafe extern "C" {
        /// `<libproc.h>`: fills the `rusage_info_v<flavor>` for `pid`.
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut u64) -> i32;
        fn waitid(idtype: i32, id: u32, info: *mut u64, options: i32) -> i32;
        fn wait4(pid: i32, status: *mut i32, options: i32, usage: *mut i64) -> i32;
    }
    const P_PID: i32 = 1;
    const WEXITED: i32 = 4;
    const WNOWAIT: i32 = 32;
    const WNOHANG: i32 = 1;
    const RUSAGE_INFO_V4: i32 = 4;
    drop(child.stdin.take());
    let (out, err) = super::hang::drain(&mut child);
    let pid = child.id();
    let id = i32::try_from(pid).unwrap();
    let what = super::hang::named(pid);
    // Asks whether the child has exited and leaves it a zombie, so its usage
    // can still be read. The status is `wait4`'s below.
    let exited = |_: &mut Child| {
        let mut info = [0u64; 16];
        // SAFETY: `info` is larger than the 104-byte `siginfo_t`.
        let asked = unsafe { waitid(P_PID, pid, info.as_mut_ptr(), WEXITED | WNOWAIT | WNOHANG) };
        if asked != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // `si_pid`, the fourth `int`, the high half of the second `u64`, is 0
        // while the child still runs.
        Ok((info[1] >> 32 != 0).then(|| std::process::ExitStatus::from_raw(0)))
    };
    if let Err(killed) = super::hang::watch_until(&mut child, &what, bounds, exited) {
        super::hang::fail(&killed);
    }
    let mut usage = [0u64; 37];
    // SAFETY: `rusage_info_v4` is 37 `u64`s.
    let read = unsafe { proc_pid_rusage(id, RUSAGE_INFO_V4, usage.as_mut_ptr()) } == 0;
    // `ri_instructions`.
    let instructions = usage.get(31).copied().filter(|&n| read && n > 0);
    let mut status = 0;
    // `struct rusage`: two `timeval`s, then 14 `long`s, `ru_nivcsw` last.
    let mut rusage = [0i64; 18];
    // SAFETY: `rusage` is the 144-byte `struct rusage`. This reaps the child,
    // so `child` is never waited on after it.
    let reaped = unsafe { wait4(id, &mut status, 0, rusage.as_mut_ptr()) } == id;
    assert!(reaped, "process {pid} could not be reaped");
    let preemptions = rusage.get(17).map_or(0, |&n| u64::try_from(n).unwrap_or(0));
    let output = Output {
        status: std::process::ExitStatus::from_raw(status),
        stdout: out.take(),
        stderr: err.take(),
    };
    let counted =
        instructions.map(|n| n.saturating_sub(preemptions.saturating_mul(PER_PREEMPTION)));
    (output, counted)
}

#[cfg(not(target_os = "macos"))]
pub fn counted_output_within(mut child: Child, bounds: Bounds) -> (Output, Option<u64>) {
    drop(child.stdin.take());
    let what = super::hang::named(child.id());
    let (out, err) = super::hang::drain(&mut child);
    let status = match super::hang::watch(&mut child, &what, bounds) {
        Ok(status) => status,
        Err(killed) => super::hang::fail(&killed),
    };
    let output = Output {
        status,
        stdout: out.take(),
        stderr: err.take(),
    };
    (output, None)
}

#[cfg(test)]
mod counted_tests {
    use super::*;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    /// **A counted program that never exits fails its test by name.** The
    /// wait had no cap, so a hang here ran until the runner died, with no log
    /// naming anything. The cap is 200 ms here.
    #[test]
    fn a_counted_program_that_never_exits_fails_naming_it() {
        let child = std::process::Command::new("sleep")
            .arg("600")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("`sleep` is on PATH");
        let id = child.id();
        let start = Instant::now();
        let bounds = Bounds::idle(Duration::from_millis(200));
        let fired = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            counted_output_within(child, bounds)
        }));
        let waited = start.elapsed();

        let panic = fired.expect_err("a program that sleeps for ten minutes outlives the cap");
        let said = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(
            said.contains("the hang cap fired") && said.contains("sleep"),
            "the cap fired and did not name the program: {said:?}"
        );
        assert!(
            said.contains("a_counted_program_that_never_exits_fails_naming_it"),
            "the cap fired and did not name the test: {said:?}"
        );
        assert!(
            waited < Duration::from_secs(300),
            "a 200 ms cap took {waited:?} to fire"
        );
        let alive = std::process::Command::new("kill")
            .args(["-0", &id.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("`kill` is on PATH");
        assert!(
            !alive.success(),
            "the cap fired and left process {id} running"
        );
    }
}
