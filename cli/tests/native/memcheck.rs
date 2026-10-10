//! Programs under Valgrind's memcheck, and the issues its errors become
//! (cli/tests/README.md, "Memcheck").
//!
//! ```text
//! BURI_MEMCHECK=/tmp/memcheck cargo test -p buri --test native -- conformance::
//! cargo run -p buri --example memcheck -- tests /tmp/memcheck 'runtime unit tests' <test binary> [args]
//! cargo run -p buri --example memcheck -- report /tmp/memcheck
//! ```
//!
//! With `BURI_MEMCHECK` set, `shared` runs every program this domain built
//! under memcheck. Each run leaves `runs/<run>.xml`, memcheck's report, and
//! `runs/<run>.about`: the program, its build, and how to run it again.
//! `report` turns each error into `issues/<signature>.md`, a title line and a
//! body. The signature is the error's kind and its top three function names, so
//! one bug met by a hundred programs is one draft. The scheduled workflow
//! (`.github/workflows/memcheck.yml`) files the drafts.

// The example uses `report`, `run` and `UNIT_TESTS`, and the test binary the rest.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Memcheck's flags. A definite leak is an error: a leaked Buri value is the
/// heap check's to report, but a block nothing points to is a bug either way.
const FLAGS: &[&str] = &[
    "--tool=memcheck",
    "--error-exitcode=1",
    "--leak-check=full",
    "--show-leak-kinds=definite",
    "--errors-for-leak-kinds=definite",
    "--track-origins=yes",
    "--num-callers=24",
    // A program spinning on a lock gets the lock's holder scheduled.
    "--fair-sched=yes",
    // A forked child about to exec is not the program.
    "--child-silent-after-fork=yes",
    // Each error's suppression, for an issue that turns out not to be a bug.
    "--gen-suppressions=all",
];

/// What the runtime's unit tests add: their leaks aren't checked, because
/// some tests leave a block behind on purpose, such as an immortal one.
pub const UNIT_TESTS: &[&str] = &["--leak-check=no", "--show-leak-kinds=none", "--errors-for-leak-kinds=none"];

/// Suppressions for system libraries, each with its reason.
const SUPPRESSIONS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/native/memcheck.supp");

/// Runs `cmd` under memcheck when `BURI_MEMCHECK` is set and `cmd` is a
/// program this domain built. `None` otherwise, so the caller runs it as is.
///
/// `build` names the backend and profile, such as `stencil debug`.
pub fn output(cmd: &Command, build: &str) -> Option<Output> {
    let dir = PathBuf::from(std::env::var_os("BURI_MEMCHECK").filter(|d| !d.is_empty())?);
    if !Path::new(cmd.get_program()).starts_with(option_env!("CARGO_TARGET_TMPDIR")?) {
        return None;
    }
    let test = std::thread::current().name().unwrap_or_default().to_string();
    let conformance = std::env::var("BURI_CONFORMANCE_BUILD")
        .map(|b| format!(" BURI_CONFORMANCE_BUILD={b}"))
        .unwrap_or_default();
    let again = format!(
        "nix develop .#perf -c env BURI_MEMCHECK=/tmp/memcheck{conformance} \\\n  \
         cargo test -p buri --features backend-llvm,memcheck --test native -- '{test}' --exact"
    );
    Some(run(&dir, cmd, build, &again, &[]))
}

/// Runs `cmd` under memcheck with `extra` flags, leaving its report in
/// `dir/runs`.
///
/// `again` is how to rerun it, for the issue.
pub fn run(dir: &Path, cmd: &Command, build: &str, again: &str, extra: &[&str]) -> Output {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let runs = dir.join("runs");
    std::fs::create_dir_all(&runs).unwrap();
    let name = format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    let program = Path::new(cmd.get_program());
    let about = format!("{}\n{build}\n{again}\n", program.display());
    std::fs::write(runs.join(format!("{name}.about")), about).unwrap();
    let mut wrapped = Command::new("valgrind");
    wrapped
        // An `extra` flag replaces the one of its name.
        .args(FLAGS.iter().filter(|f| !extra.iter().any(|e| e.split('=').next() == f.split('=').next())))
        .args(extra)
        .arg(format!("--suppressions={SUPPRESSIONS}"))
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
    wrapped.output().unwrap_or_else(|e| panic!("cannot start valgrind: {e}"))
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
        let mut lines = about.splitn(3, '\n');
        let (program, build, again) =
            (lines.next().unwrap_or("?"), lines.next().unwrap_or("?"), lines.next().unwrap_or("?").trim_end());
        for error in errors(&std::fs::read_to_string(&xml).unwrap_or_default()) {
            count += 1;
            let path = issues.join(format!("{}.md", error.signature()));
            // The first program to meet a bug names it.
            if !path.exists() {
                std::fs::write(&path, error.draft(program, build, again)).unwrap();
            }
        }
    }
    count
}

/// One memcheck error: its kind, its report as text, the function names of
/// its stacks, innermost first, and the suppression that would silence it.
#[derive(Debug, PartialEq)]
pub struct Error {
    pub kind: String,
    pub text: String,
    pub frames: Vec<String>,
    pub suppression: String,
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
        let key = format!("{}\n{}", self.kind, self.culprit());
        // FNV-1a, which no toolchain update changes.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in key.bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{:012x}", h >> 16)
    }

    /// The issue: a title line, then the body.
    pub fn draft(&self, program: &str, build: &str, again: &str) -> String {
        format!(
            "memcheck: {kind} in {culprit} [{signature}]\n\
             The scheduled memcheck job found this.\n\n\
             - **Program:** `{program}`\n\
             - **Build:** {build}\n\n\
             ```text\n{text}```\n\n\
             Reproduce on Linux from the repository root, then read \
             `/tmp/memcheck/runs/*.log`:\n\n\
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
                        frames.extend(name);
                    }
                }
            }
        }
        let suppression = element(error, "rawtext")
            .map(|(raw, _)| raw.trim().trim_start_matches("<![CDATA[").trim_end_matches("]]>").trim().to_string())
            .unwrap_or_default();
        found.push(Error { kind, text, frames, suppression });
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
    let draft = found[0].draft("/t/program", "stencil debug", "cargo test");
    assert!(draft.starts_with(&format!("memcheck: InvalidRead in buri_rt_list_get [{}]\n", found[0].signature())));
    assert_eq!(found[0].suppression, "{\n   <insert_a_suppression_name_here>\n   Memcheck:Addr8\n   fun:memcpy\n}");
    assert!(draft.contains(&found[0].suppression));
}
