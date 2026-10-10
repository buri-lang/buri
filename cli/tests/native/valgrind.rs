//! Programs under Valgrind's memcheck or helgrind, and the issues their errors
//! become (cli/tests/README.md, "Valgrind").
//!
//! ```text
//! BURI_MEMCHECK=/tmp/memcheck cargo test -p buri --test native -- conformance::
//! BURI_HELGRIND=/tmp/helgrind cargo test -p buri --test native -- fan_out::
//! cargo run -p buri --example valgrind -- tests helgrind /tmp/helgrind 'runtime unit tests' <test binary> [args]
//! cargo run -p buri --example valgrind -- report /tmp/helgrind
//! ```
//!
//! With `BURI_MEMCHECK` or `BURI_HELGRIND` set, `shared` runs every program
//! this domain built under that tool. Each run leaves `runs/<run>.xml`, the
//! tool's report, and `runs/<run>.about`: the program, its build, and how to
//! run it again. `report` turns each error into `issues/<signature>.md`, a
//! title line and a body. The signature is the error's kind and the first
//! function past the allocator, so one bug met by a hundred programs is one
//! draft. The scheduled workflows (`.github/workflows/memcheck.yml` and
//! `races.yml`) file the drafts.

// The example uses `report`, `run` and `Tool`, and the test binary the rest.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A Valgrind tool this harness runs programs under.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tool {
    Memcheck,
    Helgrind,
}

impl Tool {
    pub fn named(name: &str) -> Option<Tool> {
        match name {
            "memcheck" => Some(Tool::Memcheck),
            "helgrind" => Some(Tool::Helgrind),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Tool::Memcheck => "memcheck",
            Tool::Helgrind => "helgrind",
        }
    }

    /// The variable naming the directory this tool's runs go to.
    fn variable(self) -> &'static str {
        match self {
            Tool::Memcheck => "BURI_MEMCHECK",
            Tool::Helgrind => "BURI_HELGRIND",
        }
    }

    /// The tool's flags, past the shared ones.
    ///
    /// Memcheck: a definite leak is an error. A leaked Buri value is the heap
    /// check's to report, but a block nothing points to is a bug either way.
    /// Helgrind: a race is `report`'s to judge, because some are ordered by
    /// code it can't see ([`UNSEEN`]).
    ///
    /// Neither sets `--error-exitcode`. An error reaches `report` through the
    /// XML, and the exit status stays the program's, so a test's own failure
    /// is never hidden behind Valgrind's.
    ///
    /// Helgrind also reads a move of the stack pointer by less than
    /// `--max-stackframe` as the stack growing or shrinking, and marks the
    /// whole range between as the moving thread's. A switch between a task's
    /// stack and a thread's mapped near it is such a move, and the range holds
    /// other threads' live frames. A stack pointer sits most of a 512 KiB
    /// thread stack above the mapping below it, so a quarter MiB tells every
    /// switch from a frame.
    fn flags(self) -> &'static [&'static str] {
        match self {
            Tool::Memcheck => &[
                "--leak-check=full",
                "--show-leak-kinds=definite",
                "--errors-for-leak-kinds=definite",
                "--track-origins=yes",
            ],
            Tool::Helgrind => &["--max-stackframe=262144"],
        }
    }

    /// What the runtime's unit tests add. Memcheck doesn't check their leaks:
    /// some tests leave a block behind on purpose, such as an immortal one.
    pub fn unit_tests(self) -> &'static [&'static str] {
        match self {
            Tool::Memcheck => &["--leak-check=no", "--show-leak-kinds=none", "--errors-for-leak-kinds=none"],
            Tool::Helgrind => &[],
        }
    }

    /// Suppressions, each with its reason.
    fn suppressions(self) -> &'static str {
        match self {
            Tool::Memcheck => concat!(env!("CARGO_MANIFEST_DIR"), "/tests/native/memcheck.supp"),
            Tool::Helgrind => concat!(env!("CARGO_MANIFEST_DIR"), "/tests/native/helgrind.supp"),
        }
    }
}

/// Flags every tool runs with.
const FLAGS: &[&str] = &[
    "--num-callers=24",
    // A program spinning on a lock gets the lock's holder scheduled.
    "--fair-sched=yes",
    // A forked child about to exec is not the program.
    "--child-silent-after-fork=yes",
    // Each error's suppression, for an issue that turns out not to be a bug.
    "--gen-suppressions=all",
];

/// The tool `BURI_MEMCHECK` or `BURI_HELGRIND` asks for, and the directory
/// its runs go to.
fn chosen() -> Option<(Tool, PathBuf)> {
    [Tool::Memcheck, Tool::Helgrind]
        .into_iter()
        .find_map(|t| std::env::var_os(t.variable()).filter(|d| !d.is_empty()).map(|d| (t, PathBuf::from(d))))
}

/// Whether `program` is one this domain built.
fn ours(program: &Path) -> bool {
    option_env!("CARGO_TARGET_TMPDIR").is_some_and(|tmp| program.starts_with(tmp))
}

/// How to rerun the running test under `tool`.
fn again_for(tool: Tool) -> String {
    let test = std::thread::current().name().unwrap_or_default().to_string();
    let conformance = std::env::var("BURI_CONFORMANCE_BUILD")
        .map(|b| format!(" BURI_CONFORMANCE_BUILD={b}"))
        .unwrap_or_default();
    format!(
        "nix develop .#perf -c env {var}=/tmp/{name}{conformance} \\\n  \
         cargo test -p buri --features backend-llvm,valgrind --test native -- '{test}' --exact",
        var = tool.variable(),
        name = tool.name(),
    )
}

/// Runs `cmd` under the tool the environment asks for, when `cmd` is a
/// program this domain built. `None` otherwise, so the caller runs it as is.
///
/// `build` names the backend and profile, such as `stencil debug`.
pub fn output(cmd: &Command, build: &str) -> Option<Output> {
    let (tool, dir) = chosen()?;
    if !ours(Path::new(cmd.get_program())) {
        return None;
    }
    let mut under = wrapped(tool, &dir, cmd, build, &again_for(tool), &[]);
    Some(under.output().unwrap_or_else(|e| panic!("cannot start valgrind: {e}")))
}

/// `binary` under the tool the environment asks for, as a command the caller
/// starts and talks to, such as a server. `None` when no tool is asked for.
pub fn command(binary: &Path, build: &str) -> Option<Command> {
    let (tool, dir) = chosen()?;
    ours(binary).then(|| wrapped(tool, &dir, &Command::new(binary), build, &again_for(tool), &[]))
}

/// Runs `cmd` under `tool` with `extra` flags, leaving its report in `dir/runs`.
///
/// `again` is how to rerun it, for the issue.
pub fn run(tool: Tool, dir: &Path, cmd: &Command, build: &str, again: &str, extra: &[&str]) -> Output {
    wrapped(tool, dir, cmd, build, again, extra)
        .output()
        .unwrap_or_else(|e| panic!("cannot start valgrind: {e}"))
}

/// `cmd` under `tool`, its report bound for `dir/runs`.
fn wrapped(tool: Tool, dir: &Path, cmd: &Command, build: &str, again: &str, extra: &[&str]) -> Command {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let runs = dir.join("runs");
    std::fs::create_dir_all(&runs).unwrap();
    let name = format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    let program = Path::new(cmd.get_program());
    let about = format!("{}\n{}\n{build}\n{again}\n", tool.name(), program.display());
    std::fs::write(runs.join(format!("{name}.about")), about).unwrap();
    let flags: Vec<&str> = FLAGS.iter().chain(tool.flags()).copied().collect();
    let mut wrapped = Command::new("valgrind");
    wrapped
        .arg(format!("--tool={}", tool.name()))
        // An `extra` flag replaces the one of its name.
        .args(flags.iter().filter(|f| !extra.iter().any(|e| e.split('=').next() == f.split('=').next())))
        .args(extra)
        .arg(format!("--suppressions={}", tool.suppressions()))
        .arg("--xml=yes")
        .arg(format!("--xml-file={}", runs.join(format!("{name}.xml")).display()))
        .arg(format!("--log-file={}", runs.join(format!("{name}.log")).display()))
        .arg(program)
        .args(cmd.get_args());
    for (key, value) in cmd.get_envs() {
        match value {
            Some(value) => wrapped.env(key, value),
            None => wrapped.env_remove(key),
        };
    }
    if let Some(cwd) = cmd.get_current_dir() {
        wrapped.current_dir(cwd);
    }
    wrapped
}

/// Writes a draft issue for each distinct error in `dir/runs`, and answers
/// how many errors there were.
pub fn report(dir: &Path) -> usize {
    let issues = dir.join("issues");
    std::fs::create_dir_all(&issues).unwrap();
    let mut runs: Vec<PathBuf> = std::fs::read_dir(dir.join("runs"))
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    runs.retain(|p| p.extension().is_some_and(|e| e == "xml"));
    runs.sort();
    let mut count = 0;
    for xml in runs {
        let about = std::fs::read_to_string(xml.with_extension("about")).unwrap_or_default();
        let mut lines = about.splitn(4, '\n');
        let (tool, program, build, again) = (
            lines.next().unwrap_or("?"),
            lines.next().unwrap_or("?"),
            lines.next().unwrap_or("?"),
            lines.next().unwrap_or("?").trim_end(),
        );
        for error in errors(&std::fs::read_to_string(&xml).unwrap_or_default()) {
            if tool == Tool::Helgrind.name() && error.unseen_order().is_some() {
                continue;
            }
            count += 1;
            let path = issues.join(format!("{}.md", error.signature()));
            // The first program to meet a bug names it.
            if !path.exists() {
                std::fs::write(&path, error.draft(tool, program, build, again)).unwrap();
            }
        }
    }
    count
}

/// A draft issue for each test that failed in `log`, a test binary's output
/// under `tool`: the test's own failure, which no Valgrind error causes.
///
/// Each is `(signature, draft)`. The signature is the tool, build and test, so
/// a test failing again is the issue already open.
pub fn failures(log: &str, tool: &str, build: &str, run: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut lines = log.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(name) = line.strip_prefix("---- ").and_then(|l| l.strip_suffix(" stdout ----")) else {
            continue;
        };
        let mut output = Vec::new();
        while let Some(next) = lines.peek() {
            if next.starts_with("---- ") || *next == "failures:" {
                break;
            }
            output.push(*next);
            lines.next();
        }
        let output = output.join("\n");
        let signature = fnv(&format!("test failure\n{tool}\n{build}\n{name}"));
        let draft = format!(
            "{tool}: test failed: {name} ({build}) [{signature}]\n\
             The scheduled {tool} job ran this test and it failed on its own, apart \
             from any Valgrind error.\n\n\
             - **Test:** `{name}`\n\
             - **Build:** {build}\n\
             - **Run:** {run}\n\n\
             ```text\n{output}\n```\n",
            output = output.trim(),
        );
        found.push((signature, draft));
    }
    found
}

/// FNV-1a over `key`, as 12 hex digits: no toolchain update changes it.
fn fnv(key: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:012x}", h >> 16)
}

/// One error: its kind, its report as text, the function names of its stacks
/// in order, innermost first, each stack's frames with their places, and the
/// suppression that would silence it.
#[derive(Debug, PartialEq)]
pub struct Error {
    pub kind: String,
    pub text: String,
    pub frames: Vec<String>,
    pub stacks: Vec<Vec<String>>,
    pub suppression: String,
}

/// Helgrind races whose ordering is code helgrind can't see and this
/// repository can't annotate: a race with an access near one of these is
/// theirs, not ours. Each is matched in a stack's top [`NEAR`] frames, above
/// the first of ours.
const UNSEEN: &[(&str, &str)] = &[
    // Its locks are `std`'s futex mutexes. Tokio is checked under loom.
    ("tokio", "tokio's own locks"),
    ("mio::", "tokio's own locks"),
    // musl's `malloc` and `free`, under a lock helgrind can't see in a static
    // binary, which has no `pthread_mutex_lock` for it to intercept.
    ("__libc_malloc_impl", "musl's malloc"),
    ("__libc_free", "musl's malloc"),
    ("__libc_calloc", "musl's malloc"),
    ("__libc_realloc", "musl's malloc"),
    ("aligned_alloc", "musl's malloc"),
    ("posix_memalign", "musl's malloc"),
    ("alloc_slot", "musl's malloc"),
    ("alloc_group", "musl's malloc"),
    ("free_group", "musl's malloc"),
    ("free_meta", "musl's malloc"),
    ("nontrivial_free", "musl's malloc"),
    ("get_meta", "musl's malloc"),
    ("get_nominal_size", "musl's malloc"),
    ("get_slot_index", "musl's malloc"),
    ("enframe", "musl's malloc"),
    ("set_size", "musl's malloc"),
    ("try_avail", "musl's malloc"),
    ("dequeue", "musl's malloc"),
    // musl's threads, likewise: creation, exit, its thread-list lock and its
    // futex waits, and `posix_spawn`'s.
    ("pthread_create", "musl's threads"),
    ("pthread_exit", "musl's threads"),
    ("__tl_lock", "musl's threads"),
    ("__lock", "musl's threads"),
    ("__wait", "musl's threads"),
    ("a_store", "musl's threads"),
    ("posix_spawn", "musl's threads"),
    // `std`'s threads: what a spawn hands its thread and a join hands back,
    // ordered by the `pthread_create` and `pthread_join` above.
    ("std::thread", "std's threads"),
    ("std::sys::thread", "std's threads"),
    ("thread_local", "std's threads"),
    ("thread_parking", "std's threads"),
    ("JoinInner", "std's threads"),
    ("lifecycle.rs", "std's threads"),
    // `std`'s spawn, inlined into the runtime's: what it allocates bypasses
    // the runtime's allocator, so a block musl reused isn't fresh, and the
    // new thread reads it before any annotation of ours can order it.
    ("buri_rt::rt::start_thread", "std's threads"),
    // `std`'s `Mutex`, `Once` and `OnceLock`, each a futex: the races are on
    // their own words, as tokio and the test harness use them.
    ("std/src/sync/poison", "std's Mutex"),
    ("std::sync::once", "std's Once"),
    ("once_lock", "std's Once"),
    // `std`'s `Barrier`, a futex mutex and condition variable: the runtime's
    // tests line threads up with it.
    ("std::sync::barrier", "std's Barrier"),
    // `std`'s stdout lock, a futex.
    ("std::io::stdio", "std's stdout"),
    // `std`'s channels order a send before its receive with atomics.
    ("mpmc", "std's channels"),
    // The test harness's own bookkeeping, under `std`'s futex mutexes.
    ("test::event", "the test harness"),
    ("set_output_capture", "the test harness"),
    // `std`'s registry of thread stacks, under `std`'s futex mutex.
    ("thread_info", "std's thread registry"),
];

/// How deep in a stack [`UNSEEN`] looks.
const NEAR: usize = 6;

impl Error {
    /// Why helgrind can't order this race, when it is ordered by code it
    /// can't see: [`UNSEEN`], or an `Arc`'s last owner dropping what it holds,
    /// after a decrement whose ordering helgrind doesn't model.
    pub fn unseen_order(&self) -> Option<&'static str> {
        for stack in &self.stacks {
            for frame in stack.iter().take(NEAR) {
                if let Some((_, why)) = UNSEEN.iter().find(|(p, _)| frame.contains(p)) {
                    return Some(why);
                }
                // Below our own code is only where it was called from.
                if frame.contains("buri") {
                    break;
                }
            }
        }
        self.stacks.iter().flatten().any(|f| f.contains("Arc") && f.contains("drop_slow")).then_some("an Arc's last drop")
    }
}

/// Frames that are where memory came from or was copied, not who misused it.
const PLUMBING: &[&str] =
    &["buri_rt::allocator::", "__rustc::__rust_", "buri_rt_alloc", "malloc", "calloc", "realloc", "free", "mem"];

impl Error {
    /// The first frame that isn't [`PLUMBING`]: the access, where its stack
    /// has names, or else where the memory came from.
    pub fn culprit(&self) -> &str {
        self.frames
            .iter()
            .find(|f| !PLUMBING.iter().any(|p| f.starts_with(p)))
            .map_or("an unnamed frame", String::as_str)
    }

    /// The kind and the culprit, hashed. The same bug at another line, from
    /// another caller, build or program has the same signature.
    pub fn signature(&self) -> String {
        fnv(&format!("{}\n{}", self.kind, self.culprit()))
    }

    /// The issue: a title line, then the body.
    pub fn draft(&self, tool: &str, program: &str, build: &str, again: &str) -> String {
        format!(
            "{tool}: {kind} in {culprit} [{signature}]\n\
             The scheduled {tool} job found this.\n\n\
             - **Program:** `{program}`\n\
             - **Build:** {build}\n\n\
             ```text\n{text}```\n\n\
             Reproduce on Linux from the repository root, then read \
             `/tmp/{tool}/runs/*.log`:\n\n\
             ```sh\n{again}\n```\n\n\
             <details><summary>A suppression, for when this isn't a bug</summary>\n\n\
             ```text\n{suppression}\n```\n\n</details>\n",
            kind = self.kind,
            suppression = self.suppression,
            culprit = self.culprit(),
            signature = self.signature(),
            text = self.text,
        )
    }
}

/// The errors in memcheck's XML, in order.
pub fn errors(xml: &str) -> Vec<Error> {
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some((error, after)) = element(rest, "error") {
        rest = after;
        let kind = element(error, "kind").map(|(k, _)| unescape(k)).unwrap_or_default();
        let mut text = String::new();
        let mut frames = Vec::new();
        let mut stacks = Vec::new();
        // The parts that print, in the order they come.
        let mut at = error;
        while let Some((tag, body, after)) = next_of(at, &["what", "xwhat", "auxwhat", "stack"]) {
            at = after;
            match tag {
                "what" => text.push_str(&format!("{}\n", unescape(body))),
                "xwhat" => {
                    let what = element(body, "text").map(|(t, _)| unescape(t)).unwrap_or_default();
                    text.push_str(&format!("{what}\n"));
                }
                "auxwhat" => text.push_str(&format!(" {}\n", unescape(body))),
                _ => {
                    let mut stack = Vec::new();
                    let mut lead = "at";
                    let mut rest = body;
                    while let Some((frame, after)) = element(rest, "frame") {
                        rest = after;
                        let field = |name: &str| element(frame, name).map(|(v, _)| unescape(v));
                        let name = field("fn").map(|f| without_hash(&f));
                        let place = match (field("file"), field("line"), field("obj")) {
                            (Some(file), Some(line), _) => format!("{file}:{line}"),
                            (_, _, Some(obj)) => format!("in {obj}"),
                            _ => String::new(),
                        };
                        text.push_str(&format!("   {lead} {} ({place})\n", name.as_deref().unwrap_or("???")));
                        lead = "by";
                        // The file too: a frame `std` inlined is a bare name.
                        stack.push(format!("{} {place}", name.as_deref().unwrap_or("???")));
                        // Not the `std` inlined into a caller, such as an
                        // atomic's `store`: the caller is the culprit.
                        if !place.starts_with("library/") {
                            frames.extend(name);
                        }
                    }
                    stacks.push(stack);
                }
            }
        }
        let suppression = element(error, "rawtext")
            .map(|(raw, _)| raw.trim().trim_start_matches("<![CDATA[").trim_end_matches("]]>").trim().to_string())
            .unwrap_or_default();
        found.push(Error { kind, text, frames, stacks, suppression });
    }
    found
}

/// The first `<tag>…</tag>` in `xml`: its body, and what follows it.
fn element<'a>(xml: &'a str, tag: &str) -> Option<(&'a str, &'a str)> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = start + xml[start..].find(&close)?;
    Some((&xml[start..end], &xml[end + close.len()..]))
}

/// Whichever of `tags` comes first in `xml`, with its body and what follows.
fn next_of<'a>(xml: &'a str, tags: &[&'static str]) -> Option<(&'static str, &'a str, &'a str)> {
    let (_, tag) = tags.iter().filter_map(|&t| xml.find(&format!("<{t}>")).map(|at| (at, t))).min()?;
    element(xml, tag).map(|(body, after)| (tag, body, after))
}

fn unescape(s: &str) -> String {
    s.trim()
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// A symbol without the hash suffix a build or a type adds: Rust's
/// `::h<16 hex digits>`, and the `.<32 hex digits>` of Buri's glue, so one bug
/// in the glue of many types is one issue.
fn without_hash(name: &str) -> String {
    let hex = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit());
    match (name.rsplit_once("::h"), name.rsplit_once('.')) {
        (Some((head, hash)), _) if hex(hash, 16) => head.to_string(),
        (_, Some((head, hash))) if hex(hash, 32) => head.to_string(),
        _ => name.to_string(),
    }
}

#[test]
fn an_error_reads_back_with_its_frames_and_a_stable_signature() {
    let xml = r#"<valgrindoutput>
<error>
  <unique>0x0</unique>
  <kind>InvalidRead</kind>
  <what>Invalid read of size 8</what>
  <stack>
    <frame><ip>0x1</ip><obj>/p</obj><fn>memcpy</fn></frame>
    <frame><ip>0x2</ip><obj>/p</obj><fn>buri_rt_list_get</fn><dir>/r</dir><file>list.rs</file><line>12</line></frame>
    <frame><ip>0x3</ip><obj>/p</obj><fn>buri::lib::total::h0123456789abcdef</fn></frame>
    <frame><ip>0x4</ip><obj>/p</obj></frame>
  </stack>
  <auxwhat>Address 0x10 is 16 bytes inside a block of size 48 free&apos;d</auxwhat>
  <stack>
    <frame><ip>0x5</ip><obj>/p</obj><fn>buri_rt_free</fn><file>memory.rs</file><line>9</line></frame>
  </stack>
  <suppression>
    <sname>insert_a_suppression_name_here</sname>
    <rawtext>
<![CDATA[
{
   <insert_a_suppression_name_here>
   Memcheck:Addr8
   fun:memcpy
}
]]>
    </rawtext>
  </suppression>
</error>
<error>
  <kind>UninitCondition</kind>
  <what>Conditional jump or move depends on uninitialised value(s)</what>
  <stack><frame><ip>0x6</ip><obj>/p</obj></frame></stack>
  <auxwhat>Uninitialised value was created by a heap allocation</auxwhat>
  <stack>
    <frame><ip>0x7</ip><fn>buri_rt::allocator::system_alloc</fn></frame>
    <frame><ip>0x8</ip><fn>buri.glue.release_elems.dc36b4dd995183993ac57eaf625b0de6</fn></frame>
  </stack>
</error>
<error>
  <kind>Leak_DefinitelyLost</kind>
  <xwhat><text>48 bytes in 1 blocks are definitely lost</text><leakedbytes>48</leakedbytes></xwhat>
  <stack><frame><ip>0x9</ip><fn>malloc</fn></frame></stack>
</error>
</valgrindoutput>"#;
    let found = errors(xml);
    assert_eq!(found.len(), 3);
    assert_eq!(found[0].kind, "InvalidRead");
    assert_eq!(found[0].frames, ["memcpy", "buri_rt_list_get", "buri::lib::total", "buri_rt_free"]);
    assert_eq!(
        found[0].text,
        "Invalid read of size 8\n   at memcpy (in /p)\n   by buri_rt_list_get (list.rs:12)\n   \
         by buri::lib::total (in /p)\n   by ??? (in /p)\n \
         Address 0x10 is 16 bytes inside a block of size 48 free'd\n   at buri_rt_free (memory.rs:9)\n"
    );
    // Past the copy and the allocator, to who misused the memory.
    assert_eq!(found[0].culprit(), "buri_rt_list_get");
    assert_eq!(found[1].culprit(), "buri.glue.release_elems");
    assert_eq!(found[2].culprit(), "an unnamed frame");
    assert_eq!(found[2].text, "48 bytes in 1 blocks are definitely lost\n   at malloc ()\n");
    // Another line, caller or hash suffix is the same bug.
    let moved = xml
        .replace("<line>12</line>", "<line>40</line>")
        .replace("buri::lib::total::h0123456789abcdef", "buri::lib::other::hfedcba9876543210")
        .replace("dc36b4dd995183993ac57eaf625b0de6", "12f94a3a22e7f9c6ec20450fab2a81ab");
    let again = errors(&moved);
    assert_eq!(again[0].signature(), found[0].signature());
    assert_eq!(again[1].signature(), found[1].signature());
    assert_ne!(found[0].signature(), found[1].signature());
    let draft = found[0].draft("memcheck", "/t/program", "stencil debug", "cargo test");
    assert!(draft.starts_with(&format!("memcheck: InvalidRead in buri_rt_list_get [{}]\n", found[0].signature())));
    assert_eq!(found[0].suppression, "{\n   <insert_a_suppression_name_here>\n   Memcheck:Addr8\n   fun:memcpy\n}");
    assert!(draft.contains(&found[0].suppression));
}

#[test]
fn a_race_ordered_by_code_helgrind_cannot_see_is_not_ours() {
    let race = |second: &str| {
        format!(
            "<error><kind>Race</kind><xwhat><text>Possible data race</text></xwhat>\
             <stack><frame><fn>buri_rt::rt::thread_loop</fn></frame></stack>\
             <auxwhat>This conflicts with a previous write</auxwhat>\
             <stack><frame><fn>write</fn></frame>{second}</stack></error>"
        )
    };
    let frame = |name: &str| format!("<frame><fn>{name}</fn></frame>");
    assert_eq!(errors(&race(&frame("buri_rt::rt::push")))[0].unseen_order(), None);
    let tokio = errors(&race(&frame("tokio::sync::notify::Notify::notify_waiters")));
    assert_eq!(tokio[0].unseen_order(), Some("tokio's own locks"));
    // Below our code, tokio or `std`'s threads are where it was called from,
    // not why.
    let called = errors(&race(&(frame("buri_rt::net::serve") + &frame("tokio::runtime::task::raw::poll"))));
    assert_eq!(called[0].unseen_order(), None);
    let spawned = errors(&race(&(frame("buri_rt::rt::tests::a_close")
        + &frame("{closure#0}<std::thread::lifecycle::spawn_unchecked>"))));
    assert_eq!(spawned[0].unseen_order(), None);
    // The `std` inlined into a caller isn't the culprit; the caller is.
    let inlined = errors(
        "<error><kind>Race</kind><stack>\
         <frame><fn>atomic_store&lt;u64&gt;</fn><file>library/core/src/sync/atomic.rs</file><line>9</line></frame>\
         <frame><fn>buri_rt_incref</fn><file>memory.rs</file><line>1926</line></frame></stack></error>",
    );
    assert_eq!(inlined[0].culprit(), "buri_rt_incref");
    let drops: String = (0..10).map(|_| frame("drop_glue")).collect();
    let arc = errors(&race(&(drops + &frame("<alloc::sync::Arc<buri_rt::rt::Task>>::drop_slow"))));
    assert_eq!(arc[0].unseen_order(), Some("an Arc's last drop"));
}

#[test]
fn a_failed_test_is_its_own_draft() {
    let log = "running 2 tests
test conformance::shard_7 ... FAILED
test conformance::shard_6 ... ok

failures:

---- conformance::shard_7 stdout ----

thread 'conformance::shard_7' panicked at cli/tests/native/conformance.rs:1443:5:
1 files failed:
`numbers/integers.buri` exited 1:
assert.equal failed

failures:
    conformance::shard_7

test result: FAILED. 1 passed; 1 failed
";
    let found = failures(log, "memcheck", "llvm release", "https://run");
    assert_eq!(found.len(), 1);
    let (signature, draft) = &found[0];
    assert!(draft.starts_with(&format!(
        "memcheck: test failed: conformance::shard_7 (llvm release) [{signature}]\n"
    )));
    assert!(draft.contains("`numbers/integers.buri` exited 1:\nassert.equal failed\n```"));
    assert!(draft.contains("https://run"));
    assert_ne!(*signature, failures(log, "memcheck", "stencil debug", "https://run")[0].0);
    assert!(failures("test result: ok. 2 passed", "memcheck", "stencil debug", "").is_empty());
}
