//! The platforms bundled with the toolchain: `native`, `node` and `web`.
//!
//! Each is an ordinary `platform` rule, embedded as source by `buri-stdlib`
//! and read by the same reader a repository's build files go through.

use crate::build::buildfile::{read_build_file, PlatformName, PlatformRule};
use crate::diagnostics::FileId;

/// Every bundled platform's name and `BUILD.buri`, and every other file a
/// bundled platform's rule names: `(platform, path, text)`.
pub use buri_stdlib::platforms::{BUNDLED, FILES};

/// A file a bundled platform's rule names. Each one is embedded, which
/// `every_file_a_bundled_platform_names_is_embedded` checks.
pub fn file(platform: PlatformName, path: &str) -> &'static str {
    FILES
        .iter()
        .find(|(p, at, _)| *p == platform.name() && *at == path)
        .map_or("", |(_, _, text)| *text)
}

/// A bundled platform's rule, read once per process. Every bundled build file
/// holds one, which `every_bundled_platform_reads_cleanly` checks.
pub fn bundled(name: PlatformName) -> &'static PlatformRule {
    static RULES: std::sync::OnceLock<[PlatformRule; 3]> = std::sync::OnceLock::new();
    let rules = RULES.get_or_init(|| {
        PlatformName::BUNDLED.map(|name| {
            read_build_file(build_file(name), FileId(0))
                .value
                .platform
                .unwrap_or_default()
        })
    });
    let [native, node, web] = rules;
    match name {
        PlatformName::Native => native,
        PlatformName::Node => node,
        PlatformName::Web => web,
    }
}

/// A bundled platform's `BUILD.buri`.
fn build_file(name: PlatformName) -> &'static str {
    BUNDLED
        .iter()
        .find(|(n, _)| *n == name.name())
        .map_or("", |(_, text)| *text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bundled build file the reader refuses would be a platform with no
    /// entries, failing every output that names it.
    #[test]
    fn every_bundled_platform_reads_cleanly() {
        assert_eq!(BUNDLED.len(), PlatformName::BUNDLED.len());
        for name in PlatformName::BUNDLED {
            let read = read_build_file(build_file(name), FileId(0));
            assert!(read.errors.is_empty(), "{name:?}: {:#?}", read.errors);
            assert!(
                read.value.platform.is_some(),
                "{name:?} has no `platform` rule"
            );
            let rule = bundled(name);
            assert_eq!(rule.entries.len(), 1, "{name:?}");
            assert_eq!(rule.entries[0].name.value, "main", "{name:?}");
        }
        let native = &bundled(PlatformName::Native).entries[0];
        assert_eq!((native.variants.len(), native.variant_required), (4, false));
    }

    /// A `js` file or an asset a bundled rule names and the toolchain does not
    /// embed would fail every output of the platform.
    #[test]
    fn every_file_a_bundled_platform_names_is_embedded() {
        for name in PlatformName::BUNDLED {
            let rule = bundled(name);
            let named = rule
                .entries
                .iter()
                .filter_map(|e| e.js.as_ref())
                .chain(&rule.assets);
            for path in named {
                assert!(
                    FILES
                        .iter()
                        .any(|(p, at, _)| *p == name.name() && *at == path.value),
                    "{name:?}'s `{}` is not embedded",
                    path.value
                );
            }
        }
    }
}
