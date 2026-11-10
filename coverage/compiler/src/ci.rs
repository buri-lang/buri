//! `--against-ci`: this host's shard merged with the other platforms' shards
//! from the newest green CI run on main, and gated as CI's `coverage` job
//! gates. So a drop only this host's tests show is caught before it lands.
//!
//! The other shards measured that run's commit. Each of their lines moves to
//! where `git diff` from that commit puts it, and a line the diff changed is
//! dropped, since nothing measured it as it is now. A side this host took
//! then and doesn't now counts as untaken everywhere.
//! A side this host takes in an unchanged line that CI's runner for this
//! host didn't take counts as untaken: that's most likely a difference between
//! the two machines.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{check, compare, merge, output, run, shards, Baseline, Gate, Measured, Record, Side};

/// Runs the gate, then names the branch sides no test takes any more.
/// `measured` is this host's shard, if it was already measured, and `run_id`
/// the CI run to take the others from. Whether it passed.
pub(crate) fn against_ci(
    root: &Path,
    release: bool,
    measured: Option<&str>,
    run_id: Option<&str>,
) -> bool {
    let (id, commit) = green_run(root, run_id);
    println!("against CI run {id}, of {commit}");
    let downloaded = download(root, &id);
    let present = git(root)
        .args(["cat-file", "-e", &format!("{commit}^{{commit}}")])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !present {
        run(git(root).args(["fetch", "--quiet", "origin", &commit]));
    }
    let changes = Changes::parse(&output(git(root).args([
        "diff",
        "-U0",
        "--no-renames",
        &commit,
        "--",
        "cli",
        "crates",
    ])));

    let here = match measured {
        Some(dir) => shards(Path::new(dir)),
        None => {
            let gate = Gate::new(root.to_path_buf(), release);
            gate.build();
            gate.shard("1/1");
            shards(&gate.target.join("shard"))
        }
    };
    let green = shards(&downloaded);
    let starts = starts(&here);
    for warning in changes.stale(&green, &here, &starts) {
        println!("warning: {warning}");
    }
    let then = changes.remap(&green, &starts);
    let mut here = here;
    let held = hold_to_ci(&then, &mut here);
    let mut now = then.clone();
    now.extend(here.clone());
    stop_everywhere(&then, &mut now, &here);
    let passed = check(root, &now, true);

    let (changed, unchanged): (Vec<_>, Vec<_>) = unreached(&then, &now)
        .into_iter()
        .partition(|(path, side)| changes.changed(path, side));
    let named = |sides: &[(String, Side)]| {
        sides
            .iter()
            .map(|(path, (_, span, _, side))| {
                let which = if *side == 0 { "true" } else { "false" };
                format!("\n  {path}:{}:{}, the {which} side", span[0], span[1])
            })
            .collect::<String>()
    };
    if !held.is_empty() {
        println!(
            "\nThis host takes these {} branch side(s) in unchanged lines, and CI run {id}'s \
             runner for this host didn't, so they count as untaken. A difference between this \
             machine and CI's runner does that, such as instruction counters only a physical Mac \
             has. A new test reaching them does too, and CI will count those:{}",
            held.len(),
            named(&held)
        );
    }
    if !unchanged.is_empty() {
        println!(
            "\nNo test takes these {} branch side(s), which CI run {id} took. This host's tests \
             took them there and don't here, so they count as untaken everywhere (a difference \
             between this machine and CI's can do that too):{}",
            unchanged.len(),
            named(&unchanged)
        );
    }
    if !changed.is_empty() {
        println!(
            "\nNo test here takes these {} branch side(s) in lines changed since {}. The other \
             platforms' shards measured the code before, so they can't say if those take them:{}",
            changed.len(),
            &commit[..9.min(commit.len())],
            named(&changed)
        );
        if !passed {
            let mut taken = now;
            for (path, side) in &changed {
                for host in here.keys() {
                    let record = taken.get_mut(host).and_then(|m| m.files.get_mut(path));
                    if let Some(count) = record.and_then(|r| r.branches.get_mut(side)) {
                        *count = 1;
                    }
                }
            }
            let baseline = Baseline::parse(
                &std::fs::read_to_string(root.join("coverage").join("compiler-branches.txt"))
                    .unwrap_or_default(),
            );
            let hosts = taken.keys().cloned().collect();
            let falls = compare(&baseline, &Baseline::of(&hosts, &merge(&taken))).failed;
            println!(
                "\nIf the other platforms took every one of them, coverage {}.",
                if falls {
                    "would still fall"
                } else {
                    "wouldn't fall"
                }
            );
        }
    }
    passed
}

fn git(root: &Path) -> Command {
    let mut git = Command::new("git");
    git.current_dir(root);
    git
}

/// Run `id` and its commit, or else the newest successful CI run on main's.
fn green_run(root: &Path, id: Option<&str>) -> (String, String) {
    let mut gh = Command::new("gh");
    gh.current_dir(root).arg("run");
    match id {
        Some(id) => gh.args(["view", id]),
        None => gh.args([
            "list",
            "--workflow",
            "ci.yml",
            "--branch",
            "main",
            "--status",
            "success",
            "--limit",
            "1",
        ]),
    };
    let jq = if id.is_some() { "" } else { ".[] | " };
    let found = output(gh.args([
        "--json",
        "databaseId,headSha",
        "--jq",
        &format!("{jq}\"\\(.databaseId) \\(.headSha)\""),
    ]));
    let (id, commit) = found
        .trim()
        .split_once(' ')
        .unwrap_or_else(|| panic!("no successful CI run on main: `{}`", found.trim()));
    (id.to_string(), commit.to_string())
}

/// The run's shards, downloaded once into `target/coverage/ci/<id>`.
fn download(root: &Path, id: &str) -> PathBuf {
    let ci = crate::target(root).join("ci");
    let dir = ci.join(id);
    if dir.exists() {
        return dir;
    }
    let partial = ci.join(format!("{id}.partial"));
    let _ = std::fs::remove_dir_all(&partial);
    std::fs::create_dir_all(&partial).unwrap();
    run(Command::new("gh")
        .current_dir(root)
        .args(["run", "download", id, "-p", "coverage-shard-*", "-D"])
        .arg(&partial));
    std::fs::rename(&partial, &dir).unwrap();
    dir
}

/// Where the function of each condition this host measured starts, by file,
/// span and copy. A side the other platforms measured takes it, so a function
/// whose first line changed still keeps their counts.
type Starts = BTreeMap<(String, [u32; 4], u32), [u32; 2]>;

fn starts(here: &BTreeMap<String, Measured>) -> Starts {
    let mut starts = Starts::new();
    for (path, record) in here.values().flat_map(|m| &m.files) {
        for &(start, span, nth, _) in record.branches.keys() {
            starts.insert((path.clone(), span, nth), start);
        }
    }
    starts
}

/// One `@@ -a,b +c,d @@`: the old lines it replaces and the new ones, each as
/// start and count. A count of 0 starts at the line before.
#[derive(Clone, Copy)]
struct Hunk {
    old: (u32, u32),
    new: (u32, u32),
}

/// What `git diff -U0` changed, per file.
#[derive(Default)]
struct Changes {
    hunks: BTreeMap<String, Vec<Hunk>>,
    deleted: BTreeSet<String>,
    /// Files whose changed lines name `target_os = "linux"`.
    linux: BTreeSet<String>,
}

impl Changes {
    fn parse(diff: &str) -> Changes {
        let mut changes = Changes::default();
        let mut path = String::new();
        let mut header = false;
        for line in diff.lines() {
            if line.starts_with("diff --git ") {
                header = true;
            } else if let Some(old) = line.strip_prefix("--- a/").filter(|_| header) {
                path = old.to_string();
            } else if let Some(new) = line.strip_prefix("+++ ").filter(|_| header) {
                match new.strip_prefix("b/") {
                    Some(new) => path = new.to_string(),
                    None => {
                        changes.deleted.insert(path.clone());
                    }
                }
            } else if let Some(rest) = line.strip_prefix("@@ -") {
                header = false;
                let range = |text: &str| {
                    let (start, count) = text.split_once(',').unwrap_or((text, "1"));
                    (start.parse().unwrap(), count.parse().unwrap())
                };
                let mut words = rest.split(' ');
                let old = range(words.next().unwrap());
                let new = range(words.next().unwrap().trim_start_matches('+'));
                changes
                    .hunks
                    .entry(path.clone())
                    .or_default()
                    .push(Hunk { old, new });
            } else if !header && line.contains("target_os = \"linux\"") {
                changes.linux.insert(path.clone());
            }
        }
        changes
    }

    /// Where old `line` of `path` is now, unless the diff changed it.
    fn line(&self, path: &str, line: u32) -> Option<u32> {
        let mut now = line;
        for hunk in self.hunks.get(path).into_iter().flatten() {
            let ((old, old_count), (new, new_count)) = (hunk.old, hunk.new);
            if (old..old + old_count).contains(&line) {
                return None;
            }
            // The first line after the hunk, before and after.
            let old_end = old + old_count.max(u32::from(old_count == 0));
            let new_end = new + new_count.max(u32::from(new_count == 0));
            if line >= old_end {
                now = line - old_end + new_end;
            }
        }
        Some(now)
    }

    /// Whether the diff wrote any line of the condition `side` is on.
    fn changed(&self, path: &str, (_, span, _, _): &Side) -> bool {
        self.hunks.get(path).into_iter().flatten().any(|hunk| {
            let (new, count) = hunk.new;
            (span[0]..=span[2]).any(|line| (new..new + count).contains(&line))
        })
    }

    /// Where a side measured at the old commit is now, unless the diff
    /// changed its condition. Its function starts where this host's copy of
    /// the condition says, or else where its old first line moved.
    fn side(&self, path: &str, (start, span, nth, side): Side, starts: &Starts) -> Option<Side> {
        let (from, to) = (self.line(path, span[0])?, self.line(path, span[2])?);
        if to - from != span[2] - span[0] {
            return None;
        }
        let span = [from, span[1], to, span[3]];
        let start = match starts.get(&(path.to_string(), span, nth)) {
            Some(&start) => start,
            None => [self.line(path, start[0])?, start[1]],
        };
        Some((start, span, nth, side))
    }

    /// Every platform's measurement, at the lines it has now.
    fn remap(
        &self,
        platforms: &BTreeMap<String, Measured>,
        starts: &Starts,
    ) -> BTreeMap<String, Measured> {
        let moved = |path: &str, record: &Record| Record {
            folded: record.folded,
            lines: record
                .lines
                .iter()
                .filter_map(|(&at, &hits)| Some((self.line(path, at)?, hits)))
                .collect(),
            branches: record
                .branches
                .iter()
                .filter_map(|(&side, &taken)| Some((self.side(path, side, starts)?, taken)))
                .collect(),
        };
        platforms
            .iter()
            .map(|(host, measured)| {
                let files = measured
                    .files
                    .iter()
                    .filter(|(path, _)| !self.deleted.contains(*path))
                    .map(|(path, record)| (path.clone(), moved(path, record)))
                    .collect();
                (host.clone(), Measured { files })
            })
            .collect()
    }

    /// Why CI's shards from the platforms not `here` are stale for this
    /// change, per file: its changed lines name `target_os = "linux"`, or it
    /// changed a branch side that only those platforms took.
    fn stale(
        &self,
        green: &BTreeMap<String, Measured>,
        here: &BTreeMap<String, Measured>,
        starts: &Starts,
    ) -> Vec<String> {
        let mut stale: BTreeMap<&str, &str> = self
            .linux
            .iter()
            .map(|path| (path.as_str(), "changes `target_os = \"linux\"` code"))
            .collect();
        let taken_here = |path: &str, side: &Side| {
            here.keys().any(|host| {
                green
                    .get(host)
                    .and_then(|m| m.files.get(path))
                    .and_then(|r| r.branches.get(side))
                    .is_some_and(|&taken| taken > 0)
            })
        };
        let theirs = green.iter().filter(|(host, _)| !here.contains_key(*host));
        for (path, record) in theirs.flat_map(|(_, m)| &m.files) {
            let changed = record.branches.iter().any(|(side, &taken)| {
                taken > 0 && self.side(path, *side, starts).is_none() && !taken_here(path, side)
            });
            if changed {
                stale
                    .entry(path)
                    .or_insert("changes a branch only the other platforms took");
            }
        }
        stale
            .into_iter()
            .map(|(path, why)| {
                format!("{path} {why}, so their shards from CI, measured before it, are stale")
            })
            .collect()
    }
}

/// Zeroes each side `here` takes that the same platform left untaken in
/// `then`, and returns them. `then` holds only unchanged lines, so this
/// machine taking a side CI's runner for it didn't is most likely a
/// difference between the two, which CI would never count. Without it, a
/// side only this machine takes hides a fall CI sees.
fn hold_to_ci(
    then: &BTreeMap<String, Measured>,
    here: &mut BTreeMap<String, Measured>,
) -> Vec<(String, Side)> {
    let mut held = Vec::new();
    for (host, measured) in here.iter_mut() {
        let Some(ci) = then.get(host) else { continue };
        for (path, record) in &mut measured.files {
            for (side, taken) in &mut record.branches {
                let untaken_on_ci = ci
                    .files
                    .get(path)
                    .and_then(|r| r.branches.get(side))
                    .is_some_and(|&t| t == 0);
                if *taken > 0 && untaken_on_ci {
                    *taken = 0;
                    held.push((path.clone(), *side));
                }
            }
        }
    }
    held
}

/// Takes each side `here`'s platforms took in `then` and don't now out of
/// every platform in `now`: a change that stops a side here most likely stops
/// it everywhere.
fn stop_everywhere(
    then: &BTreeMap<String, Measured>,
    now: &mut BTreeMap<String, Measured>,
    here: &BTreeMap<String, Measured>,
) {
    for (host, measured) in here {
        for (path, record) in &measured.files {
            for (side, _) in record.branches.iter().filter(|(_, &taken)| taken == 0) {
                let took = then
                    .get(host)
                    .and_then(|m| m.files.get(path))
                    .and_then(|r| r.branches.get(side))
                    .is_some_and(|&taken| taken > 0);
                if !took {
                    continue;
                }
                for other in now.values_mut() {
                    if let Some(taken) = other
                        .files
                        .get_mut(path)
                        .and_then(|r| r.branches.get_mut(side))
                    {
                        *taken = 0;
                    }
                }
            }
        }
    }
}

/// The sides `now` leaves untaken that `then`, the same platforms before,
/// took or didn't have.
fn unreached(
    then: &BTreeMap<String, Measured>,
    now: &BTreeMap<String, Measured>,
) -> Vec<(String, Side)> {
    let taken = |platforms: &BTreeMap<String, Measured>| {
        let mut sides: BTreeMap<(String, Side), bool> = BTreeMap::new();
        for (path, record) in platforms.values().flat_map(|m| &m.files) {
            for (&side, &count) in &record.branches {
                *sides.entry((path.clone(), side)).or_default() |= count > 0;
            }
        }
        sides
    };
    let before = taken(then);
    taken(now)
        .into_iter()
        .filter(|(key, taken)| !taken && before.get(key) != Some(&false))
        .map(|(key, _)| key)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINUX: &str = "x86_64-unknown-linux-gnu";
    const MAC: &str = "aarch64-apple-darwin";
    const BUILD: &str = "cli/src/commands/build.rs";

    /// Two lines inserted after line 2, line 6 changed, lines 9 and 10
    /// deleted, and a file deleted.
    const DIFF: &str = "diff --git a/cli/src/commands/build.rs b/cli/src/commands/build.rs
index 1..2 100644
--- a/cli/src/commands/build.rs
+++ b/cli/src/commands/build.rs
@@ -2,0 +3,2 @@ fn a() {
+    let x = 1;
+    #[cfg(target_os = \"linux\")]
@@ -6 +8 @@ fn b() {
-    if old {
+    if new {
@@ -9,2 +10,0 @@ fn c() {
-    gone();
--- a/x
diff --git a/crates/js/src/gone.rs b/crates/js/src/gone.rs
deleted file mode 100644
--- a/crates/js/src/gone.rs
+++ /dev/null
@@ -1,3 +0,0 @@
-fn gone() {}
";

    /// A side of the only condition on `line`, in a function that starts on
    /// line 1.
    fn side(line: u32, side: u32) -> Side {
        ([1, 1], [line, 5, line, 9], 0, side)
    }

    /// One side per `(path, line, side, taken)`.
    fn measured(sides: &[(&str, u32, u32, u64)]) -> Measured {
        let mut measured = Measured::default();
        for &(path, line, which, taken) in sides {
            let record = measured.files.entry(path.to_string()).or_default();
            record.branches.insert(side(line, which), taken);
            record.lines.insert(line, taken);
        }
        measured
    }

    fn platforms(of: &[(&str, Measured)]) -> BTreeMap<String, Measured> {
        of.iter()
            .map(|(host, m)| (host.to_string(), m.clone()))
            .collect()
    }

    #[test]
    fn a_line_moves_by_what_the_diff_inserted_and_deleted_before_it() {
        let changes = Changes::parse(DIFF);
        let at = |line| changes.line(BUILD, line);
        assert_eq!(
            (1..=12).map(at).collect::<Vec<_>>(),
            [
                Some(1),
                Some(2),
                Some(5),
                Some(6),
                Some(7),
                None,
                Some(9),
                Some(10),
                None,
                None,
                Some(11),
                Some(12)
            ]
        );
        assert_eq!(changes.line("cli/src/main.rs", 6), Some(6));
        assert!(changes.deleted.contains("crates/js/src/gone.rs"));
        assert_eq!(changes.linux, BTreeSet::from([BUILD.to_string()]));
        let gone = changes.remap(
            &platforms(&[(LINUX, measured(&[("crates/js/src/gone.rs", 1, 0, 1)]))]),
            &Starts::new(),
        );
        assert!(gone[LINUX].files.is_empty());
        assert!(changes.changed(BUILD, &side(4, 0)));
        assert!(changes.changed(BUILD, &side(8, 1)));
        assert!(!changes.changed(BUILD, &side(9, 1)));
    }

    #[test]
    fn a_condition_an_insertion_splits_is_dropped() {
        let changes = Changes::parse(DIFF);
        let none = Starts::new();
        // Lines 2 and 3 are 2 and 5 now.
        assert_eq!(
            changes.side(BUILD, ([1, 1], [2, 5, 3, 9], 0, 1), &none),
            None
        );
        assert_eq!(
            changes.side(BUILD, ([1, 1], [3, 5, 4, 9], 0, 1), &none),
            Some(([1, 1], [5, 5, 6, 9], 0, 1))
        );
    }

    #[test]
    fn a_condition_keeps_its_counts_when_its_functions_first_line_changes() {
        let changes = Changes::parse(DIFF);
        // A function that started on line 6, which changed, and starts on 7 now.
        let old = ([6, 1], [7, 5, 7, 9], 0, 0);
        assert_eq!(changes.side(BUILD, old, &Starts::new()), None);
        let starts = Starts::from([((BUILD.to_string(), [9, 5, 9, 9], 0), [7, 1])]);
        assert_eq!(
            changes.side(BUILD, old, &starts),
            Some(([7, 1], [9, 5, 9, 9], 0, 0))
        );
    }

    #[test]
    fn a_side_this_host_stopped_taking_is_named_where_it_is_now() {
        let changes = Changes::parse(DIFF);
        let green = platforms(&[
            (LINUX, measured(&[(BUILD, 4, 0, 1), (BUILD, 7, 0, 0)])),
            (MAC, measured(&[(BUILD, 7, 0, 3), (BUILD, 4, 1, 2)])),
        ]);
        // This host's run after the change: line 7 is line 9 now, and no test
        // takes it.
        let here = platforms(&[(MAC, measured(&[(BUILD, 9, 0, 0), (BUILD, 6, 1, 2)]))]);
        let then = changes.remap(&green, &starts(&here));
        let mut now = then.clone();
        now.extend(here.clone());
        assert_eq!(unreached(&then, &now), [(BUILD.to_string(), side(9, 0))]);
        assert_eq!(
            changes.stale(&green, &here, &starts(&here)),
            [
                "cli/src/commands/build.rs changes `target_os = \"linux\"` code, so their shards \
              from CI, measured before it, are stale"
            ]
        );
    }

    #[test]
    fn a_side_only_this_machine_takes_counts_as_untaken() {
        let then = platforms(&[
            (LINUX, measured(&[(BUILD, 4, 0, 0), (BUILD, 5, 0, 0)])),
            (MAC, measured(&[(BUILD, 4, 0, 0), (BUILD, 5, 0, 1)])),
        ]);
        // Line 6's side is new, so CI's runner can't vouch either way.
        let mut here = platforms(&[(
            MAC,
            measured(&[(BUILD, 4, 0, 7), (BUILD, 5, 0, 1), (BUILD, 6, 0, 1)]),
        )]);
        assert_eq!(
            hold_to_ci(&then, &mut here),
            [(BUILD.to_string(), side(4, 0))]
        );
        let branches = &here[MAC].files[BUILD].branches;
        assert_eq!(branches[&side(4, 0)], 0);
        assert_eq!(branches[&side(5, 0)], 1);
        assert_eq!(branches[&side(6, 0)], 1);
    }

    #[test]
    fn a_side_this_host_stops_taking_stops_everywhere() {
        let then = platforms(&[
            (LINUX, measured(&[(BUILD, 4, 0, 1), (BUILD, 5, 0, 1)])),
            (MAC, measured(&[(BUILD, 4, 0, 1)])),
        ]);
        let here = platforms(&[(MAC, measured(&[(BUILD, 4, 0, 0), (BUILD, 5, 0, 0)]))]);
        let mut now = then.clone();
        now.extend(here.clone());
        stop_everywhere(&then, &mut now, &here);
        // Line 5's side, which this host never took, is still Linux's.
        assert_eq!(now[LINUX].files[BUILD].branches[&side(4, 0)], 0);
        assert_eq!(now[LINUX].files[BUILD].branches[&side(5, 0)], 1);
        assert_eq!(unreached(&then, &now), [(BUILD.to_string(), side(4, 0))]);
    }

    #[test]
    fn a_change_to_a_branch_only_the_other_platforms_took_makes_them_stale() {
        const A: &str = "crates/js/src/a.rs";
        let changes = Changes::parse(
            "diff --git a/crates/js/src/a.rs b/crates/js/src/a.rs
--- a/crates/js/src/a.rs
+++ b/crates/js/src/a.rs
@@ -6 +6 @@
-    if old {
+    if new {
",
        );
        let here = platforms(&[(MAC, Measured::default())]);
        let stale = |linux: u64, mac: u64| {
            let green = platforms(&[
                (LINUX, measured(&[(A, 6, 0, linux)])),
                (MAC, measured(&[(A, 6, 0, mac)])),
            ]);
            changes.stale(&green, &here, &Starts::new())
        };
        assert_eq!(
            stale(1, 0),
            [
                "crates/js/src/a.rs changes a branch only the other platforms took, so their \
              shards from CI, measured before it, are stale"
            ]
        );
        assert!(stale(1, 1).is_empty());
        assert!(stale(0, 0).is_empty());
    }
}
