//! The embedded standard library: its sources, the bundled platforms' sources
//! and build files, and the one table every other crate asks about them.
//!
//! Below every other toolchain crate, because the source map names standard
//! library files and the formatter reads `core/character`'s printable table.
//! What needs the checker's types (`defining_module`) or the build-file reader
//! (`is_entry_declaration`) stays with the checker, which re-exports this
//! table as `compiler::standard_library`.

pub mod compiler {
    pub mod standard_library;
}

/// The files of the platforms bundled with the toolchain, other than their
/// `platform.buri`, which is a module in [`compiler::standard_library::MODULES`].
pub mod platforms {
    /// Every bundled platform's name and `BUILD.buri`.
    pub const BUNDLED: &[(&str, &str)] = &[
        ("native", include_str!("platforms/native/BUILD.buri")),
        ("node", include_str!("platforms/node/BUILD.buri")),
        ("web", include_str!("platforms/web/BUILD.buri")),
    ];

    /// Every other file a bundled platform's rule names, an entry's `js` file
    /// or an asset: `(platform, path, text)`.
    pub const FILES: &[(&str, &str, &str)] = &[
        ("web", "main.mjs", include_str!("platforms/web/main.mjs")),
        ("web", "index.html", include_str!("platforms/web/index.html")),
    ];
}
