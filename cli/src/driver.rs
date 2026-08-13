//! Putting the front end together: load a unit, check it, report.

use crate::buildfile::Platform;
use crate::check::{Checked, Checker};
use crate::compile::{Loaded, Loader, Unit};
use crate::diag::{Diagnostics, SourceMap};
use crate::workspace::Workspace;

pub struct Analysis {
    pub loaded: Loaded,
    pub checked: Checked,
    pub diags: Diagnostics,
}

/// Loads and checks one unit. The two halves are separate so that `lint` and
/// `query` can stop after loading.
pub fn analyze(ws: Option<&Workspace>, map: &mut SourceMap, unit: &Unit) -> Analysis {
    let mut diags = Diagnostics::new();
    let loaded = {
        let mut loader = Loader::new(ws, map, &mut diags);
        loader.load_unit(unit);
        loader.finish()
    };
    let checked = Checker::new(&loaded, ws, &mut diags).run();
    diags.sort(map);
    Analysis { loaded, checked, diags }
}

/// Loads and checks every module of the standard library, with no repository.
/// This is what `buri version --self-check` runs, and what the toolchain's own
/// tests use.
pub fn analyze_stdlib(map: &mut SourceMap) -> Analysis {
    let mut diags = Diagnostics::new();
    let loaded = {
        let mut loader = Loader::new(None, map, &mut diags);
        loader.load_all_std();
        loader.finish()
    };
    let checked = Checker::new(&loaded, None, &mut diags).run();
    diags.sort(map);
    Analysis { loaded, checked, diags }
}

/// The platform a build defaults to when nothing selects one.
pub fn host_platform() -> Platform {
    // Only the JavaScript backend exists, so it is what a build produces.
    Platform::Js
}

/// The platform a *test suite* is checked against when it names none. A suite
/// runs once, on the host platform (TAGS.md, "Tags and tests"), and the host
/// is this machine — code tagged for the machine it was written for is not
/// being asked to run in a browser just because the backend emits JavaScript.
pub fn host_native_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Linux
    }
}
