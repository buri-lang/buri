//! Closes pipes a test process inherited by accident, before `main` runs.
//!
//! macOS has no `pipe2`, so a pipe is made and only then marked close-on-exec.
//! nextest spawns tests from several threads, so a test started in that gap
//! inherits another test's stdout or stderr pipe and passes it on to every
//! `buri`, linker and JS engine it runs. The other test then shows as LEAK
//! until this one finishes. Nothing here is meant to inherit a pipe.

#[used]
#[cfg_attr(target_os = "macos", link_section = "__DATA,__mod_init_func")]
#[cfg_attr(target_os = "linux", link_section = ".init_array")]
static AT_START: extern "C" fn() = close_inherited_pipes;

extern "C" fn close_inherited_pipes() {
    use std::os::unix::fs::FileTypeExt;
    use std::os::unix::io::FromRawFd;
    unsafe extern "C" {
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        fn close(fd: i32) -> i32;
    }
    const F_GETFD: i32 = 1;
    // Collected first: the listing's own descriptor is closed before the loop.
    let Ok(entries) = std::fs::read_dir("/dev/fd") else { return };
    let fds: Vec<i32> = entries
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse().ok())
        .filter(|&fd| fd > 2)
        .collect();
    for fd in fds {
        // SAFETY: `fcntl` only reads the descriptor table.
        if unsafe { fcntl(fd, F_GETFD) } == -1 {
            continue;
        }
        // SAFETY: `fd` is open, and `ManuallyDrop` leaves closing it to the line below.
        let file = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd) });
        if file.metadata().is_ok_and(|m| m.file_type().is_fifo()) {
            // SAFETY: nothing in this process has seen `fd` yet.
            unsafe { close(fd) };
        }
    }
}
