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
//! `jsonc` and `json5`, which `std/json` checks and formats natively; a
//! `REPO.buri` may give one of them more extensions and nothing else. A
//! language of a repository's own names the `tool` rules that check, format
//! and generate from it. Only a file some rule's `inputs` lists is checked or
//! formatted, so a `package.json` beside the sources is left alone.

pub mod json;

use crate::build::buildfile::Spanned;
use crate::diagnostics::{Diagnostic, Span};
use std::path::Path;

/// One language, and the extensions that select it.
#[derive(Clone, Debug)]
pub struct Language {
    pub name: String,
    pub extensions: Vec<Spanned<String>>,
    pub kind: Kind,
}

/// Who checks and formats a language's files.
#[derive(Clone, Debug)]
pub enum Kind {
    /// `json`, `jsonc` or `json5`: `std/json`, in-tree.
    BuiltIn(json::Dialect),
    /// A language a `REPO.buri` declared. Each field is the tool it names, as
    /// written; the entry point of the same name on that tool does the work.
    Custom(Tools),
}

/// The tools a repository's own language names.
#[derive(Clone, Debug, Default)]
pub struct Tools {
    pub check: Option<Spanned<String>>,
    pub format: Option<Spanned<String>>,
    pub generate: Option<Spanned<String>>,
}

impl Tools {
    /// The tool named for the entry point `entry`, if the language names one.
    pub fn named(&self, entry: &str) -> Option<&Spanned<String>> {
        match entry {
            "check" => self.check.as_ref(),
            "format" => self.format.as_ref(),
            "generate" => self.generate.as_ref(),
            _ => None,
        }
    }
}

impl Language {
    /// The dialect `std/json` reads this language as, for a built-in one.
    pub fn dialect(&self) -> Option<json::Dialect> {
        match &self.kind {
            Kind::BuiltIn(d) => Some(*d),
            Kind::Custom(_) => None,
        }
    }

    /// The tools a repository's own language names, or `None` for a built-in.
    pub fn tools(&self) -> Option<&Tools> {
        match &self.kind {
            Kind::BuiltIn(_) => None,
            Kind::Custom(t) => Some(t),
        }
    }
}

/// Every language a repository has: the built-in ones, with whatever
/// extensions its `REPO.buri` added, and the ones it declared.
#[derive(Clone, Debug)]
pub struct Languages {
    pub all: Vec<Language>,
}

impl Default for Languages {
    fn default() -> Languages {
        let builtin = |name: &str, extension: &str, dialect| Language {
            name: name.to_string(),
            extensions: vec![Spanned::new(extension.to_string(), Span::NONE)],
            kind: Kind::BuiltIn(dialect),
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
    /// schema whose extension names no built-in language.
    pub fn dialect_of(&self, path: &str) -> json::Dialect {
        self.of(path).and_then(Language::dialect).unwrap_or(json::Dialect::Json)
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
    /// What a tool's check said in its own words, where a tool rather than
    /// this toolchain found it. See [`Said`].
    pub said: Option<Box<Said>>,
}

/// The sentences a tool's `check` wrote for one of its diagnostics.
///
/// A tool cannot invent a catalogue page, so its message prints under the code
/// it asked for when the catalogue has that code, and under `tool-diagnostic`
/// naming it when it does not — the rule a generator's diagnostics follow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Said {
    pub message: String,
    pub note: Option<String>,
    pub fix: Option<String>,
}

impl Finding {
    pub fn new(code: &str, file: &str, span: (usize, usize), binds: Vec<(&str, String)>) -> Finding {
        Finding {
            code: code.to_string(),
            file: file.to_string(),
            span,
            binds: binds.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            said: None,
        }
    }

    /// The diagnostic, once the file is at `span` in a source map.
    pub fn diagnostic(&self, span: Span) -> Diagnostic {
        let Some(said) = &self.said else {
            let mut d = Diagnostic::templated(&self.code, span);
            for (k, v) in &self.binds {
                d.bind(k.clone(), v.clone());
            }
            return d;
        };
        let mut d = match crate::documentation::page_of_code(&self.code).is_some() {
            true => Diagnostic::error(span, said.message.clone()).with_code(self.code.clone()),
            false => Diagnostic::templated("tool-diagnostic", span)
                .with_bind("code", self.code.clone())
                .with_bind("message", said.message.clone()),
        };
        if let Some(note) = &said.note {
            d = d.with_note(note.clone());
        }
        if let Some(fix) = &said.fix {
            d = d.with_fix(fix.clone());
        }
        d
    }

    fn to_json(&self) -> crate::json::Value {
        use crate::json::Value;
        let text = |t: &Option<String>| t.as_ref().map_or(Value::Null, Value::str);
        let said = match &self.said {
            None => Value::Null,
            Some(s) => Value::object(vec![
                ("message", Value::str(&s.message)),
                ("note", text(&s.note)),
                ("fix", text(&s.fix)),
            ]),
        };
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
            ("said", said),
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
        let text = |s: &crate::json::Value, name| s.get(name).and_then(|t| t.as_str()).map(str::to_string);
        let said = match v.get("said") {
            Some(s @ crate::json::Value::Object(_)) => {
                Some(Box::new(Said { message: text(s, "message")?, note: text(s, "note"), fix: text(s, "fix") }))
            }
            _ => None,
        };
        Some(Finding {
            code: v.get("code")?.as_str()?.to_string(),
            file: v.get("file")?.as_str()?.to_string(),
            span: (offset("start")?, offset("end")?),
            binds,
            said,
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
    /// The schema a contract checks the file against, instead of its own.
    pub contract: Option<String>,
}

impl Check {
    /// Reads what checking `path` needs. `read` answers for a repository path,
    /// and is where an editor's unsaved buffers come in.
    pub fn prepare(
        languages: &Languages,
        path: &str,
        text: String,
        contract: Option<String>,
        read: &dyn Fn(&str) -> Option<String>,
    ) -> Check {
        let mut asked = std::collections::BTreeSet::new();
        let mut recording = |p: &str| {
            asked.insert(p.to_string());
            read(p)
        };
        let reads =
            json::schema_files(path, &text, contract.as_deref(), &|p| languages.dialect_of(p), &mut recording);
        Check { path: path.to_string(), text, reads, asked, contract }
    }

    pub fn run(&self, languages: &Languages) -> Vec<Finding> {
        json::check(&self.path, &self.text, self.contract.as_deref(), &|p| languages.dialect_of(p), &self.reads)
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
/// `None` for a file that does not parse, and for one in no built-in language.
pub fn format(languages: &Languages, path: &str, text: &str) -> Option<String> {
    json::format(text, languages.of(path)?.dialect()?)
}
