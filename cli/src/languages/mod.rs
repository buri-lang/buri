//! Languages: what a file a build file references is written in, and how the
//! toolchain checks and formats it.
//!
//! ```text
//! language {
//!     name: "jsonc"
//!     extensions: [".code-workspace"]
//! }
//! ```
//!
//! The extension decides the language. The built-in languages are `json`,
//! `jsonc` and `json5`; a `REPO.buri` may give one of them more extensions and
//! nothing else. Only a file some rule's `inputs` lists is checked or
//! formatted, so a `package.json` beside the sources is left alone.

pub mod json;

use crate::build::buildfile::Spanned;
use crate::diagnostics::{Diagnostic, Span};
use std::path::Path;

/// One language, and the extensions that select it.
#[derive(Clone, Debug)]
pub struct Language {
    pub name: &'static str,
    pub extensions: Vec<Spanned<String>>,
    pub dialect: json::Dialect,
}

/// Every language a repository has: the built-in ones, with whatever
/// extensions its `REPO.buri` added.
#[derive(Clone, Debug)]
pub struct Languages {
    pub all: Vec<Language>,
}

impl Default for Languages {
    fn default() -> Languages {
        let builtin = |name, extension: &str, dialect| Language {
            name,
            extensions: vec![Spanned::new(extension.to_string(), Span::NONE)],
            dialect,
        };
        Languages {
            all: vec![
                builtin("json", ".json", json::Dialect::Json),
                builtin("jsonc", ".jsonc", json::Dialect::Jsonc),
                builtin("json5", ".json5", json::Dialect::Json5),
            ],
        }
    }
}

impl Languages {
    pub fn named(&self, name: &str) -> Option<&Language> {
        self.all.iter().find(|l| l.name == name)
    }

    /// The language of a file, by the longest extension its name ends with.
    pub fn of(&self, path: &str) -> Option<&Language> {
        let name = path.rsplit('/').next().unwrap_or(path);
        self.all
            .iter()
            .flat_map(|l| l.extensions.iter().map(move |e| (l, e.value.as_str())))
            .filter(|(_, e)| name.len() > e.len() && name.ends_with(e))
            .max_by_key(|(_, e)| e.len())
            .map(|(l, _)| l)
    }

    /// The dialect to read a file as: its language's, or plain JSON for a
    /// schema whose extension names no language.
    pub fn dialect_of(&self, path: &str) -> json::Dialect {
        self.of(path).map_or(json::Dialect::Json, |l| l.dialect)
    }
}

/// Something a check found, in a file of the repository.
///
/// Carried as data rather than as a [`Diagnostic`] because it is found in the
/// build layer, cached, and only later placed in a source map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub code: String,
    /// Repository-relative.
    pub file: String,
    /// Byte offsets into that file.
    pub span: (usize, usize),
    /// The page's placeholders.
    pub binds: Vec<(String, String)>,
}

impl Finding {
    pub fn new(code: &str, file: &str, span: (usize, usize), binds: Vec<(&str, String)>) -> Finding {
        Finding {
            code: code.to_string(),
            file: file.to_string(),
            span,
            binds: binds.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        }
    }

    /// The diagnostic, once the file is at `span` in a source map.
    pub fn diagnostic(&self, span: Span) -> Diagnostic {
        let mut d = Diagnostic::templated(&self.code, span);
        for (k, v) in &self.binds {
            d.bind(k.clone(), v.clone());
        }
        d
    }

    fn to_json(&self) -> crate::json::Value {
        use crate::json::Value;
        Value::object(vec![
            ("code", Value::str(&self.code)),
            ("file", Value::str(&self.file)),
            ("start", Value::number(i64::try_from(self.span.0).unwrap_or(i64::MAX))),
            ("end", Value::number(i64::try_from(self.span.1).unwrap_or(i64::MAX))),
            (
                "binds",
                Value::Array(
                    self.binds
                        .iter()
                        .map(|(k, v)| Value::Array(vec![Value::str(k), Value::str(v)]))
                        .collect(),
                ),
            ),
        ])
    }

    fn from_json(v: &crate::json::Value) -> Option<Finding> {
        let offset = |name| v.get(name).and_then(|n| n.as_u32()).map(|n| n as usize);
        let binds = v
            .get("binds")?
            .as_array()?
            .iter()
            .map(|pair| {
                let [k, v] = pair.as_array()? else { return None };
                Some((k.as_str()?.to_string(), v.as_str()?.to_string()))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Finding {
            code: v.get("code")?.as_str()?.to_string(),
            file: v.get("file")?.as_str()?.to_string(),
            span: (offset("start")?, offset("end")?),
            binds,
        })
    }

    /// The cache's form of one check's findings.
    pub fn encode(findings: &[Finding]) -> String {
        crate::json::Value::Array(findings.iter().map(Finding::to_json).collect()).to_string()
    }

    pub fn decode(text: &str) -> Option<Vec<Finding>> {
        crate::json::parse(text).ok()?.as_array()?.iter().map(Finding::from_json).collect()
    }
}

/// One referenced file's check, ready to key and run.
pub struct Check {
    /// Repository-relative.
    pub path: String,
    pub text: String,
    /// Every other file the check reads, by repository path.
    pub reads: std::collections::BTreeMap<String, String>,
    /// Every path it looked for, found or not: a schema that appears later
    /// changes the verdict as surely as one that is edited.
    pub asked: std::collections::BTreeSet<String>,
}

impl Check {
    /// Reads what checking `path` needs. `read` answers for a repository path,
    /// and is where an editor's unsaved buffers come in.
    pub fn prepare(languages: &Languages, path: &str, text: String, read: &dyn Fn(&str) -> Option<String>) -> Check {
        let mut asked = std::collections::BTreeSet::new();
        let mut recording = |p: &str| {
            asked.insert(p.to_string());
            read(p)
        };
        let reads = json::schema_files(path, &text, &|p| languages.dialect_of(p), &mut recording);
        Check { path: path.to_string(), text, reads, asked }
    }

    pub fn run(&self, languages: &Languages) -> Vec<Finding> {
        json::check(&self.path, &self.text, &|p| languages.dialect_of(p), &self.reads)
    }
}

/// A reader for repository paths under `root`, with `overlay` over the disk.
pub fn reader<'a>(
    root: &'a Path,
    overlay: &'a crate::build::sources::Overlay,
) -> impl Fn(&str) -> Option<String> + 'a {
    move |rel: &str| {
        let full = root.join(rel);
        match overlay.get(&full) {
            Some(text) => Some(text.clone()),
            None => std::fs::read_to_string(&full).ok(),
        }
    }
}

/// The canonical text of a file in a built-in language, by its name alone.
pub fn format(languages: &Languages, path: &str, text: &str) -> Option<String> {
    json::format(text, languages.of(path)?.dialect)
}
