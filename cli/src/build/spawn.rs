//! How an action's process is started.
//!
//! This module used to be called `sandbox` and used to try to be one: a fresh
//! directory per action holding read-only copies of its declared inputs, and a
//! `sandbox-exec` profile denying the network. Both are gone, deliberately, and
//! the reason is the shape of this language rather than a compromise about
//! effort.
//!
//! **Hermeticity here is a property of the type system.** Every ambient read is
//! a `$host_*` intrinsic; `core/host` is importable only from the module that
//! exports `main`; a test's capabilities are fakes the runner injects. A library
//! or a test source has no *name* for the environment, the clock, the
//! filesystem, or the network, so there is nothing for an OS confinement to
//! confine. And the action set is closed — there are four kinds, all of them
//! this toolchain's own code, and no way for a repository to define a fifth —
//! so there is no user-supplied program to distrust either.
//!
//! What OS confinement would have bought is a second opinion about *toolchain*
//! bugs: an intrinsic that leaked something it should not. It would have bought
//! that on macOS only, for writes and the network only, and not for reads at
//! all — a profile tight enough to deny reads also denies the JavaScript
//! runtime its own binary. A partial second opinion on one platform is not
//! worth a mechanism that has to be maintained, probed, and explained, and it
//! is not what catches that class of bug: the reject corpus and
//! `--check-reproducible` are.
//!
//! What remains is about **determinism**, which is a different property and the
//! one the cache depends on:
//!
//! - **An explicit environment.** `env_clear`, then exactly two constants:
//!   `TZ=UTC` and `SOURCE_DATE_EPOCH=0`. Not to hide the parent's environment
//!   from a program that could read it — nothing in an action can — but so that
//!   the same action produces the same bytes on a machine set to a different
//!   time zone or carrying a different `LANG`.
//! - **A frozen clock.** [`FIXED_CLOCK_JS`], spliced into the action's script.
//!   Belt and braces against a runtime regression, and what makes a
//!   reproducibility check meaningful for a suite: two runs of one suite
//!   produce the same record rather than two records differing in a timing
//!   field.

use std::path::{Path, PathBuf};
use std::process::Command;

/// JavaScript spliced into an action's script, after the runtime is defined and
/// before the action runs.
///
/// Every override is guarded, because the minifier drops what the program does
/// not reach: a suite that never asks the time carries no host clock, and
/// assigning to a name that was dropped would be a `ReferenceError` in module
/// code. A guard that finds nothing has nothing to freeze.
///
/// Nothing here **declares** a name at module scope, for the same reason read
/// from the other side: this text is spliced after the generator has run, so
/// the minifier cannot know about it, and a `var` beside a mangled `let` of the
/// same name is a `SyntaxError` before a line of either runs. The random seed
/// is therefore a closure's local.
pub const FIXED_CLOCK_JS: &str = concat!(
    "// The action's clock: 1970-01-01T00:00:00Z, frozen.\n",
    "try{Date.now=function(){return 0;};}catch(e){}\n",
    "try{if(typeof $host_HostClock_nowMillis===\"function\")",
    "$host_HostClock_nowMillis=function(){return 0;};}catch(e){}\n",
    // Replaced rather than left alone: the runtime's own `sleepMillis` waits
    // on a real timer, which a frozen `Date.now` does not shorten by a
    // millisecond. Where no time elapses, sleeping for it takes no time.
    //
    // The replacement is not `async`, and does not need to be: its callers
    // `await` it, and `await 0` is `0`.
    "try{if(typeof $host_HostClock_sleepMillis===\"function\")",
    "$host_HostClock_sleepMillis=function(){return 0;};}catch(e){}\n",
    // The seed lives inside the closure rather than in a module-scope `var`.
    // This text is spliced *after* the generator has run, so the minifier never
    // sees it and cannot keep its own names away from one declared here — and
    // its mangled globals are short `$`-prefixed names, of which `$r` is one it
    // reaches once a program is large enough. A name nothing declares is a name
    // nothing can collide with.
    "try{Math.random=(function(){var r=1;return function(){",
    "r=(r*1103515245+12345)&0x7fffffff;return r/0x80000000;};})();}catch(e){}\n",
);

/// The fixed instant, as `SOURCE_DATE_EPOCH` spells it.
pub const SOURCE_DATE_EPOCH: &str = "0";

/// The command that runs `program` for an action.
///
/// `None` when the program cannot be resolved to an absolute path, which is the
/// caller's cue to say "install a JavaScript runtime" rather than to spawn
/// something and misreport what went wrong.
pub fn command(program: &str) -> Option<Command> {
    let exe = resolve(program)?;
    let mut cmd = Command::new(&exe);
    // Cleared, then exactly two constants. Both are the clock; neither is
    // inherited.
    cmd.env_clear();
    cmd.env("TZ", "UTC");
    cmd.env("SOURCE_DATE_EPOCH", SOURCE_DATE_EPOCH);
    Some(cmd)
}

/// `bun` -> `/usr/local/bin/bun`, using the parent's `PATH`.
///
/// Done here, before the environment is cleared, because a child with no `PATH`
/// cannot look a program up and a child with a `PATH` does not have an explicit
/// environment. Resolving in the parent is the only way to have both.
pub fn resolve(program: &str) -> Option<PathBuf> {
    let candidate = Path::new(program);
    if candidate.is_absolute() {
        return candidate.is_file().then(|| candidate.to_path_buf());
    }
    if program.contains(std::path::MAIN_SEPARATOR) {
        return std::fs::canonicalize(candidate).ok();
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(program)).find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixed_clock_freezes_every_source_of_time() {
        // A change to the runtime that renames one of these leaves the guard
        // finding nothing and the clock running, which is exactly the silent
        // failure this names.
        for name in
            ["Date.now", "$host_HostClock_nowMillis", "$host_HostClock_sleepMillis", "Math.random"]
        {
            assert!(FIXED_CLOCK_JS.contains(name), "the fixed clock does not freeze {name}");
        }
    }

    #[test]
    fn an_absolute_program_resolves_to_itself_and_a_missing_one_to_nothing() {
        assert_eq!(resolve("/usr/bin/true"), Some(PathBuf::from("/usr/bin/true")));
        assert_eq!(resolve("/no/such/program"), None);
        assert_eq!(resolve("buri-does-not-ship-this"), None);
    }

    /// The environment is exactly the two constants that are the clock, and
    /// `get_envs` reports the overrides on top of a cleared environment — so
    /// this is the child's whole environment rather than a subset of it.
    #[test]
    fn a_spawned_action_carries_no_inherited_variable() {
        let cmd = command("/usr/bin/true").expect("/usr/bin/true resolves");
        let vars: Vec<String> =
            cmd.get_envs().filter_map(|(k, v)| v.map(|_| k.to_string_lossy().to_string())).collect();
        assert_eq!(vars.len(), 2, "an action's environment holds more than the clock: {vars:?}");
        assert!(vars.contains(&"TZ".to_string()));
        assert!(vars.contains(&"SOURCE_DATE_EPOCH".to_string()));
    }
}
