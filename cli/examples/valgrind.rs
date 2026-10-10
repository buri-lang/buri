//! The scheduled Valgrind jobs' tool (cli/tests/README.md, "Valgrind").
//!
//! ```text
//! cargo run -q -p buri --example valgrind -- tests <memcheck|helgrind> <dir> <build> <test binary> [args]
//! cargo run -q -p buri --example valgrind -- report <dir>
//! cargo run -q -p buri --example valgrind -- failed <memcheck|helgrind> <dir> <build> <log> <run url>
//! ```
//!
//! `tests` runs a Rust test binary under the tool with the flags the test
//! harness uses, less memcheck's leak check, and exits as it did. `report`
//! writes a draft issue per distinct error in `<dir>/issues`, prints each
//! title, and exits 1 when there was an error. `failed` reads a test binary's
//! output, writes a draft per test that failed on its own, prints each title,
//! and exits 1 when one did.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects,
    reason = "a CI tool: what it prints is its output, and it stops on a bad argument"
)]

#[path = "../tests/native/valgrind.rs"]
mod valgrind;

use std::path::Path;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["tests", tool, dir, build, program, rest @ ..] => {
            let Some(tool) = valgrind::Tool::named(tool) else { return usage() };
            let mut cmd = Command::new(program);
            cmd.args(rest);
            // The runtime's tests are the one binary this runs; a build of them
            // with the `valgrind` feature is the one to run.
            let again = format!(
                "nix develop .#perf -c cargo test -p buri-rt-tests --features valgrind --no-run\n\
                 nix develop .#perf -c cargo run -p buri --example valgrind -- tests {tool} /tmp/{tool} \
                 '{build}' {program} {}",
                rest.join(" "),
                tool = tool.name(),
            );
            let out = valgrind::run(tool, Path::new(dir), &cmd, build, again.trim_end(), tool.unit_tests());
            print!("{}", String::from_utf8_lossy(&out.stdout));
            eprint!("{}", String::from_utf8_lossy(&out.stderr));
            ExitCode::from(u8::try_from(out.status.code().unwrap_or(1)).unwrap_or(1))
        }
        ["report", dir] => {
            let errors = valgrind::report(Path::new(dir));
            let issues = std::fs::read_dir(Path::new(dir).join("issues")).unwrap();
            let mut titles: Vec<String> = issues
                .filter_map(|e| std::fs::read_to_string(e.unwrap().path()).ok())
                .map(|body| body.lines().next().unwrap_or_default().to_string())
                .collect();
            titles.sort();
            for title in &titles {
                println!("{title}");
            }
            println!("{errors} error(s), {} distinct", titles.len());
            if errors == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        ["failed", tool, dir, build, log, run] => {
            let Some(tool) = valgrind::Tool::named(tool) else { return usage() };
            let issues = Path::new(dir).join("issues");
            std::fs::create_dir_all(&issues).unwrap();
            let log = std::fs::read_to_string(log).unwrap_or_default();
            let found = valgrind::failures(&log, tool.name(), build, run);
            for (signature, draft) in &found {
                println!("{}", draft.lines().next().unwrap_or_default());
                std::fs::write(issues.join(format!("{signature}.md")), draft).unwrap();
            }
            println!("{} test(s) failed on their own", found.len());
            if found.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: valgrind tests <memcheck|helgrind> <dir> <build> <test binary> [args] | valgrind report <dir> \
         | valgrind failed <memcheck|helgrind> <dir> <build> <log> <run url>"
    );
    ExitCode::from(2)
}
