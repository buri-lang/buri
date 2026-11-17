//! A `libSystem.tbd` of only the symbols a macOS link needs
//! (`design/PERFORMANCE.md` §6.151).
//!
//! The SDK's `libSystem.tbd` is about 290 KB of YAML naming every symbol of
//! libSystem and the libraries it re-exports, and lld parses all of it on every
//! link. A program imports about eighty of them. So a link is handed a
//! `libSystem.tbd` naming only the symbols its inputs need, under the SDK's
//! install name and version, and writes the same bytes.
//!
//! A symbol the cut leaves out fails the link rather than changing it, and
//! `build::link` then links again with the whole `libSystem`.

use std::path::{Path, PathBuf};

/// libSystem's exports for one target, from its `.tbd`.
pub struct Exports {
    target: String,
    /// The first document's `install-name`, `current-version` and
    /// `compatibility-version` lines.
    header: String,
    symbols: crate::hash::Set<Vec<u8>>,
    /// The `$ld$` symbols, which change how other symbols link at some
    /// deployment targets, so every cut keeps them all.
    ld: Vec<String>,
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn usize_at(bytes: &[u8], at: usize) -> Option<usize> {
    usize::try_from(u32_at(bytes, at)?).ok()
}

/// How many more flow lists `line` opens than it closes.
fn depth(line: &str) -> isize {
    line.bytes()
        .map(|b| match b {
            b'[' => 1,
            b']' => -1,
            _ => 0,
        })
        .sum()
}

/// The items of a YAML flow list, `[ a, 'b' ]`, unquoted.
fn items(list: &str) -> impl Iterator<Item = &str> {
    let inner = list.trim().trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(|item| item.trim().trim_matches('\''))
        .filter(|item| !item.is_empty())
}

impl Exports {
    /// The exports of `tbd`, a version 4 text stub, for `target`, such as
    /// `arm64-macos`. `None` when a flow list never closes. A stub without a
    /// header makes a cut that fails its link, which then links again whole.
    pub fn read(tbd: &str, target: &str) -> Option<Exports> {
        let mut out = Exports {
            target: target.to_string(),
            header: String::new(),
            symbols: crate::hash::Set::default(),
            ld: Vec::new(),
        };
        let mut documents = tbd.split("--- !tapi-tbd\n");
        documents.next()?;
        for (n, document) in documents.enumerate() {
            let mut section = String::new();
            let mut ours = false;
            let mut lines = document.lines();
            while let Some(line) = lines.next() {
                // A flow list may go on over several lines.
                let mut entry = line.to_string();
                let mut open = depth(line);
                while open > 0 {
                    let more = lines.next()?;
                    open = open.saturating_add(depth(more));
                    entry.push_str(more);
                }
                let trimmed = entry.trim_start();
                let item = trimmed.strip_prefix("- ");
                ours &= item.is_none();
                let Some((key, value)) = item.unwrap_or(trimmed).split_once(':') else {
                    continue;
                };
                if !entry.starts_with(' ') {
                    section = key.to_string();
                    let kept = ["install-name", "current-version", "compatibility-version"];
                    if n == 0 && kept.contains(&key) {
                        out.header.push_str(&entry);
                        out.header.push('\n');
                    }
                    continue;
                }
                if section != "exports" && section != "reexports" {
                    continue;
                }
                match key {
                    "targets" => ours = items(value).any(|t| t == target),
                    "symbols" if ours => {
                        for name in items(value) {
                            if name.starts_with("$ld$") {
                                out.ld.push(name.to_string());
                            } else {
                                out.symbols.insert(name.as_bytes().to_vec());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Some(out)
    }

    /// Whether libSystem exports `name`.
    pub fn has(&self, name: &[u8]) -> bool {
        self.symbols.contains(name)
    }

    /// A `.tbd` exporting `names`, which [`Exports::has`] all said yes to,
    /// and the `$ld$` symbols.
    pub fn cut(&self, names: &[&[u8]]) -> String {
        let quoted: Vec<String> = names
            .iter()
            .map(|name| String::from_utf8_lossy(name).into_owned())
            .chain(self.ld.iter().cloned())
            .map(|name| format!("'{name}'"))
            .collect();
        format!(
            "--- !tapi-tbd\ntbd-version: 4\ntargets: [ {target} ]\n{header}exports:\n  - targets: [ \
             {target} ]\n    symbols: [ {symbols} ]\n...\n",
            target = self.target,
            header = self.header,
            symbols = quoted.join(", "),
        )
    }
}

/// Calls `f` with each external symbol a 64-bit Mach-O object needs and
/// doesn't define. `None` when `object` isn't one.
pub fn undefined<'a>(object: &'a [u8], mut f: impl FnMut(&'a [u8])) -> Option<()> {
    (u32_at(object, 0)? == 0xfeed_facf).then_some(())?;
    let mut at = 32usize;
    for _ in 0..u32_at(object, 16)? {
        // `LC_SYMTAB`.
        if u32_at(object, at)? == 2 {
            let symbols = usize_at(object, at.checked_add(8)?)?;
            let count = usize_at(object, at.checked_add(12)?)?;
            let names = object.get(usize_at(object, at.checked_add(16)?)?..)?;
            let table = object.get(symbols..symbols.checked_add(count.checked_mul(16)?)?)?;
            for symbol in table.as_chunks::<16>().0 {
                // External and undefined, and not a debugging entry.
                if symbol.get(4)? & 0xef != 0x01 {
                    continue;
                }
                let name = names.get(usize_at(symbol, 0)?..)?;
                f(name.get(..name.iter().position(|&b| b == 0)?)?);
            }
        }
        at = at.checked_add(usize_at(object, at.checked_add(4)?)?)?;
    }
    Some(())
}

/// [`undefined`] for every object member of an archive. `None` when
/// `archive` isn't one.
pub fn archive_undefined<'a>(archive: &'a [u8], mut f: impl FnMut(&'a [u8])) -> Option<()> {
    archive.starts_with(b"!<arch>\n").then_some(())?;
    let mut at = 8usize;
    while at < archive.len() {
        let header = archive.get(at..at.checked_add(60)?)?;
        let number = |field: &[u8]| -> Option<usize> {
            std::str::from_utf8(field).ok()?.trim().parse().ok()
        };
        let size = number(header.get(48..58)?)?;
        let start = at.checked_add(60)?;
        let body = archive.get(start..start.checked_add(size)?)?;
        // A BSD archive's long name comes first in the member's contents.
        let name = header
            .strip_prefix(b"#1/")
            .and_then(|n| number(n.get(..13)?));
        let body = body.get(name.unwrap_or(0)..)?;
        // The symbol index isn't an object, and is passed over.
        let _ = undefined(body, &mut f);
        at = start.checked_add(size)?.checked_add(size & 1)?;
    }
    Some(())
}

/// The SDK's `libSystem.tbd` that `-lSystem` names on `argv`, a macOS link's.
/// `None` when lld might find another: when a `-L` directory, searched before
/// the SDK, has a `libSystem` of its own, under the SDK root or not, or when
/// one is named in an argument of its own.
pub fn find(argv: &[String]) -> Option<PathBuf> {
    let root = argv.iter().skip_while(|arg| *arg != "-syslibroot").nth(1)?;
    let tbd = Path::new(root).join("usr/lib/libSystem.tbd");
    let libsystem = |dir: &Path| {
        ["tbd", "dylib", "so", "a"]
            .iter()
            .any(|ext| dir.join(format!("libSystem.{ext}")).exists())
    };
    let shadowed = argv
        .iter()
        .filter_map(|arg| arg.strip_prefix("-L"))
        .any(|dir| {
            dir.is_empty()
                | libsystem(Path::new(dir))
                | libsystem(Path::new(&format!("{root}{dir}")))
        });
    (!shadowed & tbd.exists()).then_some(tbd)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TBD: &str = "--- !tapi-tbd
tbd-version:     4
targets:         [ x86_64-macos, arm64-macos ]
install-name:    '/usr/lib/libSystem.B.dylib'
current-version: 1345.100.2
exports:
  - targets:         [ x86_64-macos, arm64-macos ]
    symbols:         [ '$ld$hide$os10.0$_old', _a,
                       _b ]
  - targets:         [ x86_64-macos ]
    symbols:         [ _intel ]
--- !tapi-tbd
tbd-version:     4
targets:         [ arm64-macos ]
install-name:    '/usr/lib/system/libsystem_c.dylib'
current-version: 1
exports:
  - targets:         [ arm64-macos ]
    symbols:         [ _c ]
    objc-classes:    [ NotASymbol ]
...
";

    #[test]
    fn the_exports_are_every_documents_symbols_for_the_target() {
        let exports = Exports::read(TBD, "arm64-macos").unwrap();
        let mut names: Vec<&[u8]> = exports.symbols.iter().map(Vec::as_slice).collect();
        names.sort_unstable();
        assert_eq!(names, [&b"_a"[..], b"_b", b"_c"]);
        assert_eq!(exports.ld, ["$ld$hide$os10.0$_old"]);
        assert!(exports
            .header
            .contains("install-name:    '/usr/lib/libSystem.B.dylib'"));
        assert!(!exports.header.contains("libsystem_c"));
    }

    #[test]
    fn a_cut_names_the_symbols_asked_for_and_the_ld_ones() {
        let exports = Exports::read(TBD, "arm64-macos").unwrap();
        let cut = exports.cut(&[b"_b"]);
        assert!(
            cut.contains("symbols: [ '_b', '$ld$hide$os10.0$_old' ]"),
            "{cut}"
        );
        assert!(cut.contains("install-name:    '/usr/lib/libSystem.B.dylib'\n"));
        assert!(Exports::read(&cut, "arm64-macos").unwrap().has(b"_b"));
    }

    #[test]
    fn bytes_that_are_no_object_have_no_symbols() {
        assert!(undefined(b"hello", |_| {}).is_none());
        assert!(archive_undefined(b"hello", |_| {}).is_none());
    }
}
