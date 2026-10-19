//! Why this host can't build a runnable stencil program, for the suites that
//! skip where it can't.
//!
//! The suites' question rather than the compiler's: nothing in `buri` asks it.
//! It's answered from the backend's own [`AVAILABLE`] and
//! [`host_stencil_target`], so a suite that skips here lights up with no edit
//! the day the missing half lands, where one that asked `cfg!(target_arch)`
//! for itself would have to be edited.
use buri::compiler::backend::stencil::{host_stencil_target, AVAILABLE};

/// Why this host can't build a runnable stencil program, or `None` where it
/// can. It's the sentence a skipping suite prints.
pub fn unavailable_reason() -> Option<String> {
    if AVAILABLE {
        return None;
    }
    let Some(host) = host_stencil_target() else {
        return Some(String::from(
            "stencil has no x86_64-apple-darwin backend yet",
        ));
    };
    // `AVAILABLE` is false and the host has a target, so its library is empty.
    Some(format!(
        "this toolchain was built without {} stencils, so there is no stencil library \
         for the host to run",
        host.slug()
    ))
}

/// The sentence and the flag are one question, and a skip that printed
/// nothing would be the silent pass the suites are guarding against.
#[test]
fn the_unavailable_reason_is_present_exactly_when_the_backend_is_not() {
    assert_eq!(AVAILABLE, unavailable_reason().is_none());
    if let Some(why) = unavailable_reason() {
        assert!(!why.is_empty());
    }
}
