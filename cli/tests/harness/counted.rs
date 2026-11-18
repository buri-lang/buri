//! A child's instructions, less the kernel's work each preemption adds.
//!
//! macOS's `ri_instructions` counts the kernel's work on the process too, and
//! there's no user-only count without root. Each involuntary context switch
//! adds about [`PER_PREEMPTION`], and a loaded machine switches often: a 771 M
//! run read 790 M after 1,701 switches, and 771 M once they were taken off.

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
/// The child must have been spawned with its output piped. Unlike
/// [`super::hang::CappedChild`], the wait has no cap: the count is only for
/// programs a test expects to finish in a few seconds.
#[cfg(target_os = "macos")]
pub fn counted_output(mut child: Child) -> (Output, Option<u64>) {
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
    const RUSAGE_INFO_V4: i32 = 4;
    drop(child.stdin.take());
    let (out, err) = super::hang::drain(&mut child);
    let pid = child.id();
    let id = i32::try_from(pid).unwrap();
    let mut info = [0u64; 16];
    // SAFETY: `info` is larger than the 104-byte `siginfo_t`. The child stays
    // a zombie, so its usage can still be read.
    let exited = unsafe { waitid(P_PID, pid, info.as_mut_ptr(), WEXITED | WNOWAIT) } == 0;
    assert!(exited, "process {pid} could not be waited for");
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
pub fn counted_output(child: Child) -> (Output, Option<u64>) {
    use super::hang::CappedChild;
    (child.capped_wait_with_output().unwrap(), None)
}
