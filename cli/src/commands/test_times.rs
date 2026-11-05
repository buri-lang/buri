//! How long each native test took the last time it ran, kept in
//! `.buri/test-times` so the next `buri test` can start its slowest tests
//! first.
//!
//! Only the order tests are handed to processes reads it, never the report.
//! It lives beside `.buri/cache` rather than in it, so a new toolchain, which
//! empties the cache, still finds which tests are slow. A missing file is an
//! empty one: the run goes by test count and source length.
//!
//! One line per test: its nanoseconds, the suite's label and the test's name,
//! separated by tabs. The name is last, so it may hold a tab.

use std::collections::HashMap;

/// The file's path under the repository root.
const PATH: &str = ".buri/test-times";

/// Each suite's tests' times, by label and then by name.
#[derive(Default)]
pub struct Times(HashMap<String, HashMap<String, u64>>);

impl Times {
    /// The times recorded under `root`.
    pub fn load(root: &std::path::Path) -> Times {
        let mut times = Times::default();
        let text = std::fs::read_to_string(root.join(PATH)).unwrap_or_default();
        let lines = text.lines().filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            Some((fields.next()?.parse().ok()?, fields.next()?, fields.next()?))
        });
        for (ns, label, name) in lines {
            let suite = times.0.entry(label.to_string()).or_default();
            suite.insert(name.to_string(), ns);
        }
        times
    }

    /// The recorded times of the suite `label`'s tests, if it has any.
    pub fn suite(&self, label: &str) -> Option<&HashMap<String, u64>> {
        self.0.get(label)
    }

    /// Records each suite in `timed`, by label, in place of its earlier
    /// times. Failing to write is not failing: the next run goes by what it
    /// finds.
    pub fn save(mut self, root: &std::path::Path, timed: Vec<(String, HashMap<String, u64>)>) {
        if timed.is_empty() {
            return;
        }
        self.0.extend(timed);
        let mut lines: Vec<String> = self
            .0
            .iter()
            .flat_map(|(label, tests)| {
                tests
                    .iter()
                    // A line per test, so a name over several has none.
                    .filter(|(name, _)| !name.contains('\n'))
                    .map(move |(name, ns)| format!("{ns}\t{label}\t{name}\n"))
            })
            .collect();
        lines.sort();
        let path = root.join(PATH);
        crate::build::create_parent(&path);
        // Renamed into place, so a run reading it sees one run's times whole.
        let partial = path.with_file_name(format!(".test-times-{}", std::process::id()));
        let _ = std::fs::write(&partial, lines.concat())
            .and_then(|()| std::fs::rename(&partial, &path));
    }
}
