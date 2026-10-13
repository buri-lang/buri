//! The product's own link line, for the in-process harnesses (`native`,
//! `fuzz`) that emit objects without the `buri` binary.
//!
//! **A harness that links more permissively than the product is a harness that
//! cannot see the product's bugs.** That sentence is already in
//! `tests/native/stencil.rs`, written the day a hand-rolled `cc -o out *.o
//! libburi_rt.a` passed every program in the file while `buri test` failed 977
//! of 997 conformance tests on the same emitter: the harness had left off
//! `-dead_strip`. So nothing here spells a flag. Every argument comes from the
//! product's own `CDriver`, and this file only decides what a harness needs
//! that `buri` doesn't: linking objects already on disk, outside any cache.
#![allow(dead_code, reason = "`fuzz` uses part of it, `native` all of it")]

use buri::build::link::{self, CDriver, LibcMode, RuntimeArchive};
use buri::build::spawn;
use buri::compiler::backend::Target;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The host's driver, or `None` on a host that cannot link at all: no `cc`, or
/// the hermetic refusal.
fn host_driver() -> Option<(Target, CDriver)> {
    let target = Target {
        platform: link::host_platform()?,
        arch: link::host_arch(),
    };
    link::select(target).ok().map(|cc| (target, cc))
}

/// Stages whatever the link needs into `dir` (today the musl sysroot) and
/// returns the arguments the product passes **after** the objects and the
/// archive, in its order.
///
/// The paths in them are relative, exactly as in the product, so the command
/// must run with `current_dir(dir)`. That is `CDriver::link`'s reproducibility
/// discipline (ARCHITECTURE.md §7), not a quirk of this helper.
///
/// Empty on a host that cannot link at all. That isn't permissiveness: the
/// product can't link there either, and the harness's own driver invocation is
/// about to fail for the same reason.
pub fn args(dir: &Path) -> Vec<String> {
    let Some((_, cc)) = host_driver() else {
        return Vec::new();
    };
    if cc.libc() == LibcMode::MuslBaked {
        let _ = link::stage_sysroot(dir, None);
    }
    cc.platform_flags()
}

/// The driver [`args`] are for.
///
/// `$CC` on every host but one. On the `musl-clang` tier the *program* is the
/// answer to "which libc", and a harness that kept spawning `$CC` would link
/// against glibc while the product linked against musl.
pub fn driver() -> Option<PathBuf> {
    host_driver().map(|(_, cc)| cc.driver().to_path_buf())
}

/// The arguments a **compile** needs to agree with the link [`args`] describe.
///
/// Only the baked tier has anything to say. There the driver is the host's own
/// `cc` and `--target=` is the whole mechanism pointing it at musl, so a probe
/// compiled without it would be compiled against one libc's idea of a
/// `struct stat` and linked against another's. On the `musl-clang` tier, on
/// `BURI_MUSL=off` and on macOS, the compile and the link already name one
/// target, and a flag here would be the divergence rather than the fix.
pub fn compile_args() -> Vec<String> {
    let Some((target, cc)) = host_driver() else {
        return Vec::new();
    };
    if cc.libc() != LibcMode::MuslBaked {
        return Vec::new();
    }
    // The rendering the link's `--target=` is built from: two spellings of one
    // triple would be the divergence this exists to close.
    buri::compiler::backend::triple_text(target)
        .map(|triple| vec![format!("--target={triple}")])
        .unwrap_or_default()
}

/// Links objects and the runtime archive into `out` with `CDriver::link`'s
/// command line, for a test that needs a program to run and isn't asking about
/// the link.
///
/// The linker runs directly with the line the driver printed once, as in the
/// product. The driver runs only where there's no line to replay or replaying
/// it failed: starting the `cc` wrapper's bash and `clang` for every link was
/// most of a harness link's time.
///
/// `dir` is the directory [`args`] staged, and it must also hold the runtime
/// archive under its product name. The objects and `out` are absolute paths.
pub fn run(dir: &Path, objects: &[PathBuf], out: &Path) -> std::io::Result<Output> {
    let platform = link::host_platform()
        .ok_or_else(|| std::io::Error::other("this machine has no native platform to link for"))?;
    let cc = link::select(Target {
        platform,
        arch: link::host_arch(),
    })
    .map_err(|refusal| std::io::Error::other(refusal.message))?
    .in_dir(dir.to_path_buf());
    let runtime = RuntimeArchive::Linked;
    let names: Vec<String> = objects
        .iter()
        .map(|o| o.to_string_lossy().into_owned())
        .collect();
    let args = cc.driver_args(&names, runtime);
    let replay = cc.replay(&args, &names, runtime, true);
    if let Some(replay) = &replay {
        if let Some(argv) = writing_to(replay.argv(&names), out) {
            let mut direct = Command::new(replay.program());
            cc.pin(&mut direct);
            if let Ok(done) = spawn::output(direct.args(argv)) {
                if done.status.success() {
                    return Ok(done);
                }
            }
        }
    }
    let argv = writing_to(args, out)
        .ok_or_else(|| std::io::Error::other("the driver's arguments name no `-o artifact`"))?;
    let mut command = Command::new(cc.driver());
    cc.pin(&mut command);
    let done = spawn::output(command.args(argv))?;
    if done.status.success() && replay.is_some() {
        // The replay failed where the driver did not, so it is stale.
        cc.forget_replay(runtime, true);
    }
    Ok(done)
}

/// `argv` writing `out` rather than `artifact`. `None` unless `-o artifact`
/// appears exactly once.
fn writing_to<T: Into<OsString>>(argv: Vec<T>, out: &Path) -> Option<Vec<OsString>> {
    let mut argv: Vec<OsString> = argv.into_iter().map(Into::into).collect();
    let at: Vec<usize> = argv
        .iter()
        .enumerate()
        .skip(1)
        .zip(&argv)
        .filter(|((_, arg), flag)| *flag == "-o" && *arg == "artifact")
        .map(|((i, _), _)| i)
        .collect();
    let [at] = at.as_slice() else { return None };
    *argv.get_mut(*at)? = out.into();
    Some(argv)
}

/// The rows of the manifest a link wrote into `dir`, or `None` when there is no
/// readable one. A line that isn't three fields is skipped.
pub fn manifest(dir: &Path) -> Option<Vec<link::Row>> {
    let text = std::fs::read_to_string(dir.join("manifest")).ok()?;
    let rows = text.lines().filter_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [unit, key, status] = fields.as_slice() else {
            return None;
        };
        Some(link::Row {
            unit: (*unit).to_string(),
            key: (*key).to_string(),
            cached: *status == "cached",
        })
    });
    Some(rows.collect())
}
