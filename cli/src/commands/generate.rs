//! `buri gen`.
//!
//! Rewrites the six fields that restate the sources — `sources`,
//! `dependencies`, `test.sources`, `test.dependencies`, `testing.sources`,
//! `testing.dependencies` — and no others. `generators`, `tags`, `platforms`,
//! `timeout_seconds`, `visibility`, `outputs`, `test.platforms`,
//! and every comment come back saying exactly what they said — see
//! `crate::build::regenerate`, which does the rewriting.
//!
//! With no target argument it regenerates the whole repository, which is what
//! every command with no target argument does. A tree restated one directory
//! at a time would be one where `gen --check` passes where you are standing
//! and fails one directory over.
//!
//! Whether a package's build file is out of date is kept in `.buri/cache`
//! ([`Answers`]), so a `gen --check` of an unchanged tree analyses nothing.
//! What isn't kept is worked out with every package's targets checked as one
//! compilation ([`Batch`]).
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "what was rewritten, and what is out of date under `--check`, is this \
              command's output; diagnostics still leave through `Session::emit`"
)]

use crate::build::cache::{hash_bytes, Action, ActionKey, Cache, KeyBuilder};
use crate::build::regenerate::{self, Batch, Worked};
use crate::build::session::{self, Session};
use crate::build::workspace::{PackageId, RuleKind, TargetId, Workspace};
use crate::commands::arguments;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Rewrites the fields that restate the sources, and no others. `tags`,
/// `platforms`, `timeout_seconds`, `visibility`, `outputs`,
/// `test.platforms`, and every comment come back saying exactly what they
/// said.
pub fn command_generate(args: &arguments::Args) -> i32 {
    let code = generate_all(args);
    if crate::profile::enabled() {
        eprintln!("build files worked out {}", WORKED_OUT.load(Ordering::Relaxed));
    }
    code
}

/// The packages whose build file this command worked out rather than
/// recalled: `build files worked out` in a `BURI_PROFILE` report.
static WORKED_OUT: AtomicU64 = AtomicU64::new(0);

fn generate_all(args: &arguments::Args) -> i32 {
    let (mut session, targets) = match session::open_and_resolve(&args.flags, &args.targets) {
        Ok(both) => both,
        Err(c) => return c as i32,
    };
    let check = args.flags.check;

    let mut packages: Vec<PackageId> = targets.iter().map(|t| t.package).collect();
    packages.sort();
    packages.dedup();

    // A remembered answer stands in for a package unless `gen` is to write
    // it: only whether it's out of date is kept, not the new text.
    let answers = Answers::open(&session, &args.flags);
    let recalled = answers.recall(&session, &packages);
    let todo: Vec<PackageId> = packages
        .iter()
        .zip(&recalled)
        .filter(|(_, stale)| stale.is_none_or(|stale| stale && !check))
        .map(|(p, _)| *p)
        .collect();
    let mut worked: BTreeMap<PackageId, Result<Worked, crate::diagnostics::Diagnostic>> = BTreeMap::new();
    if !todo.is_empty() {
        let units: Vec<TargetId> = todo
            .iter()
            .flat_map(|&package| {
                let p = session.workspace.package(package);
                [(RuleKind::Library, p.has_library()), (RuleKind::Binary, p.has_binary())]
                    .into_iter()
                    .filter(|(_, has)| *has)
                    .map(move |(kind, _)| TargetId { package, kind })
            })
            .collect();
        let batch = Batch::of(&mut session, &units);
        for &package in &todo {
            WORKED_OUT.fetch_add(1, Ordering::Relaxed);
            worked.insert(package, regenerate::regenerate_in(&mut session, package, batch.as_ref()));
        }
        // Before anything is written: a record holds the tree it was worked
        // out from.
        answers.remember(&session, &worked);
    }

    let mut stale = Vec::new();
    for (&package, recalled) in packages.iter().zip(recalled) {
        let update = match worked.remove(&package) {
            Some(Ok(done)) => done.update,
            Some(Err(d)) => {
                session.emit(&d);
                return 1;
            }
            None => {
                if recalled == Some(true) {
                    stale.push(session.workspace.package(package).path.clone());
                }
                continue;
            }
        };
        let Some(update) = update else { continue };
        stale.push(session.workspace.package(package).path.clone());
        if !check {
            let path = session.workspace.package(package).build_path.clone();
            if std::fs::write(&path, &update.text).is_ok() {
                println!("updated {}/BUILD.buri", session.workspace.package(package).path);
                for line in &update.summary {
                    println!("  {line}");
                }
            }
        }
    }

    if check {
        for p in &stale {
            println!("{p}/BUILD.buri is out of date");
        }
        return if stale.is_empty() { 0 } else { 1 };
    }
    0
}

/// The shape of a record, so a change to it is a miss rather than a
/// misreading.
const FORMAT: &str = "buri-gen-answer-1";

/// Whether each package's build file was out of date, kept between runs.
///
/// A record is filed under the build graph, the package and the files `gen`
/// lists in it, and holds everything else its answer read: each target's
/// closure ([`crate::build::sources::closure_over`]), any file whose imports
/// were read off disk, and the key each generator it loaded ran under. That
/// key covers the tool's program, files no rule lists included. A record
/// holds only while every one of those is what it was.
struct Answers {
    cache: Cache,
    root: PathBuf,
    /// Which files exist and what every build file says.
    graph: u64,
}

impl Answers {
    fn open(session: &Session, flags: &arguments::Flags) -> Answers {
        let mut sources = crate::build::sources::Sources::at(&session.root, flags.clone());
        let graph = sources.graph_key(&crate::build::sources::Overlay::new());
        Answers { cache: Cache::open(&session.root), root: session.root.clone(), graph }
    }

    /// Each package's key, worked out on every core.
    fn keys(&self, workspace: &Workspace, packages: &[PackageId]) -> Vec<Option<ActionKey>> {
        crate::parallel::map(packages.len(), |i| {
            let package = workspace.package(*packages.get(i)?);
            let (mut files, mut schemas) = (Vec::new(), Vec::new());
            regenerate::collect(&package.dir, &package.dir, &mut files, &mut schemas);
            let mut k = KeyBuilder::new(Action::Regenerate, arguments::BuildMode::Debug);
            k.input("graph", &self.graph.to_le_bytes());
            k.rule_identity(&package.label(), "gen", &files);
            k.input("schemas", schemas.join("\n").as_bytes());
            Some(k.finish())
        })
    }

    /// Per package, whether its build file is out of date, where a record
    /// still holds.
    fn recall(&self, session: &Session, packages: &[PackageId]) -> Vec<Option<bool>> {
        let workspace = &*session.workspace;
        let records: Vec<Option<Record>> = self
            .keys(workspace, packages)
            .iter()
            .map(|key| self.cache.get(key.as_ref()?).and_then(|bytes| Record::decode(&bytes)))
            .collect();
        let generated_hold = |record: &Record| {
            record.generated.iter().all(|(path, kind, key)| {
                let Some(package) = workspace.package_by_path(path) else { return false };
                let Some(kind) = kind_named(kind) else { return false };
                workspace.generated.key_of(TargetId { package, kind }).unwrap_or_default() == *key
            })
        };
        let names: BTreeSet<&str> =
            records.iter().flatten().flat_map(|r| r.reads.iter().map(|(rel, _)| rel.as_str())).collect();
        let digests = self.digests(names.into_iter().map(|rel| self.root.join(rel)).collect());
        records
            .into_iter()
            .map(|record| {
                let record = record?;
                let held = generated_hold(&record)
                    && record.reads.iter().all(|(rel, was)| digests.get(&self.root.join(rel)) == Some(was));
                held.then_some(record.stale)
            })
            .collect()
    }

    /// Files each package's answer what it read, before anything is written.
    fn remember(
        &self,
        session: &Session,
        worked: &BTreeMap<PackageId, Result<Worked, crate::diagnostics::Diagnostic>>,
    ) {
        let workspace = &*session.workspace;
        let done: Vec<(PackageId, &Worked)> =
            worked.iter().filter_map(|(p, w)| Some((*p, w.as_ref().ok()?))).collect();
        let packages: Vec<PackageId> = done.iter().map(|(p, _)| *p).collect();
        let keys = self.keys(workspace, &packages);
        let files: BTreeSet<PathBuf> = done.iter().flat_map(|(_, w)| w.reads.iter().cloned()).collect();
        let digests = self.digests(files.into_iter().collect());
        for ((_, w), key) in done.iter().zip(&keys) {
            let Some(key) = key else { continue };
            let mut generated: BTreeSet<(String, &'static str, String)> = BTreeSet::new();
            for rule in &w.generated {
                let path = workspace.package(rule.package).path.clone();
                let key = workspace.generated.key_of(*rule).unwrap_or_default();
                generated.insert((path, rule.kind.name(), key));
            }
            let mut reads: BTreeMap<String, &str> = BTreeMap::new();
            for path in &w.reads {
                if let Some(digest) = digests.get(path) {
                    reads.insert(workspace.rel_of(path), digest);
                }
            }
            let mut out = format!("{FORMAT}\n{}\n{}\n", if w.update.is_some() { "stale" } else { "same" }, generated.len());
            for (path, kind, key) in &generated {
                out.push_str(&format!("{path}\t{kind}\t{key}\n"));
            }
            for (rel, digest) in &reads {
                out.push_str(&format!("{rel}\t{digest}\n"));
            }
            self.cache.put(key, out.as_bytes());
        }
    }

    /// The digest of each file's bytes now, or `absent`, read on every core.
    fn digests(&self, files: Vec<PathBuf>) -> BTreeMap<PathBuf, String> {
        let digests = crate::parallel::map(files.len(), |i| files.get(i).map_or_else(String::new, |p| digest(p)));
        files.into_iter().zip(digests).collect()
    }
}

fn digest(path: &Path) -> String {
    std::fs::read(path).map_or_else(|_| "absent".to_string(), |bytes| hash_bytes(&bytes))
}

fn kind_named(name: &str) -> Option<RuleKind> {
    [RuleKind::Library, RuleKind::Binary, RuleKind::Tool].into_iter().find(|k| k.name() == name)
}

/// One package's answer as [`Answers::remember`] wrote it.
struct Record {
    stale: bool,
    /// Each generator rule's package path, kind and key.
    generated: Vec<(String, String, String)>,
    /// Each file read, repository-relative, with its digest.
    reads: Vec<(String, String)>,
}

impl Record {
    fn decode(bytes: &[u8]) -> Option<Record> {
        let text = std::str::from_utf8(bytes).ok()?;
        let mut lines = text.lines();
        if lines.next()? != FORMAT {
            return None;
        }
        let stale = match lines.next()? {
            "stale" => true,
            "same" => false,
            _ => return None,
        };
        let count: usize = lines.next()?.parse().ok()?;
        let mut generated = Vec::with_capacity(count);
        for _ in 0..count {
            let mut fields = lines.next()?.splitn(3, '\t');
            generated.push((fields.next()?.to_string(), fields.next()?.to_string(), fields.next()?.to_string()));
        }
        let mut reads = Vec::new();
        for line in lines {
            let (rel, digest) = line.split_once('\t')?;
            reads.push((rel.to_string(), digest.to_string()));
        }
        Some(Record { stale, generated, reads })
    }
}
