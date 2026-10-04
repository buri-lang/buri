//! The platforms bundled with the toolchain: `native`, `node` and `web`.
//!
//! Each is an ordinary `platform` rule, embedded as source by `buri-stdlib`
//! and read by the same reader a repository's build files go through.

use crate::build::buildfile::{read_build_file, PlatformRule};
use crate::diagnostics::FileId;

/// Every bundled platform's name and `BUILD.buri`, and every other file a
/// bundled platform's rule names: `(platform, path, text)`.
pub use buri_stdlib::platforms::{BUNDLED, FILES};

/// The file at `path` in the bundled platform called `platform`.
pub fn file(platform: &str, path: &str) -> Option<&'static str> {
    FILES.iter().find(|(p, at, _)| *p == platform && *at == path).map(|(_, _, text)| *text)
}

/// The bundled platform called `name`, read once per process.
pub fn bundled(name: &str) -> Option<&'static PlatformRule> {
    static RULES: std::sync::OnceLock<Vec<(&'static str, PlatformRule)>> = std::sync::OnceLock::new();
    let rules = RULES.get_or_init(|| {
        BUNDLED
            .iter()
            .filter_map(|(name, text)| Some((*name, read_build_file(text, FileId(0)).value.platform?)))
            .collect()
    });
    rules.iter().find(|(n, _)| *n == name).map(|(_, rule)| rule)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bundled build file the reader refuses would be a platform with no
    /// entries, failing every output that names it.
    #[test]
    fn every_bundled_platform_reads_cleanly() {
        for (name, text) in BUNDLED {
            let read = read_build_file(text, FileId(0));
            assert!(read.errors.is_empty(), "{name}: {:#?}", read.errors);
            let rule = bundled(name).unwrap_or_else(|| panic!("{name} has no `platform` rule"));
            assert_eq!(rule.entries.len(), 1, "{name}");
            assert_eq!(rule.entries[0].name.value, "main", "{name}");
        }
        let native = bundled("native").map(|r| (r.entries[0].variants.len(), r.entries[0].variant_required));
        assert_eq!(native, Some((4, true)));
    }

    /// A `js` file or an asset a bundled rule names and the toolchain does not
    /// embed would fail every output of the platform.
    #[test]
    fn every_file_a_bundled_platform_names_is_embedded() {
        for (name, _) in BUNDLED {
            let rule = bundled(name).unwrap_or_else(|| panic!("{name} has no `platform` rule"));
            let named = rule.entries.iter().filter_map(|e| e.js.as_ref()).chain(&rule.assets);
            for path in named {
                assert!(file(name, &path.value).is_some(), "{name}'s `{}` is not embedded", path.value);
            }
        }
    }
}
