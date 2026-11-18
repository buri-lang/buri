//! The Valgrind jobs' tool (cli/tests/README.md, "Valgrind").
//!
//! ```text
//! cargo run -q -p buri --example valgrind -- tests <memcheck|helgrind> <dir> <build> <test binary> [args]
//! cargo run -q -p buri --example valgrind -- report <dir>
//! cargo run -q -p buri --example valgrind -- failed <memcheck|helgrind> <dir> <build> <log>
//! ```
//!
//! `tests` runs a Rust test binary under the tool with the flags the test
//! harness uses, less memcheck's leak check, and exits as it did. `report`
//! writes a report per distinct error in `<dir>/errors`, prints every report
//! there, adds them to `$GITHUB_STEP_SUMMARY` when that is set, and exits 1
//! when there was an error. `failed` reads a test binary's output, writes a
//! report per test that failed on its own, prints each title, and exits 1 when
//! one did.

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

#[path = "../tests/harness/hang.rs"]
mod hang;

use std::path::Path;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["tests", tool, dir, build, program, rest @ ..] => {
            let Some(tool) = valgrind::Tool::named(tool) else {
                return usage();
            };
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
            let out = valgrind::run(
                tool,
                Path::new(dir),
                &cmd,
                build,
                again.trim_end(),
                tool.unit_tests(),
            );
            print!("{}", String::from_utf8_lossy(&out.stdout));
            eprint!("{}", String::from_utf8_lossy(&out.stderr));
            ExitCode::from(u8::try_from(out.status.code().unwrap_or(1)).unwrap_or(1))
        }
        ["report", dir] => {
            let errors = valgrind::report(Path::new(dir));
            let summary = valgrind::summary(Path::new(dir));
            print!("{summary}");
            println!("{errors} Valgrind error(s)");
            if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") {
                use std::io::Write as _;
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(path)
                    .unwrap();
                write!(file, "{summary}").unwrap();
            }
            if errors == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        ["failed", tool, dir, build, log] => {
            let Some(tool) = valgrind::Tool::named(tool) else {
                return usage();
            };
            let reports = Path::new(dir).join("errors");
            std::fs::create_dir_all(&reports).unwrap();
            let log = std::fs::read_to_string(log).unwrap_or_default();
            let found = valgrind::failures(&log, tool.name(), build);
            for (signature, report) in &found {
                println!("{}", report.lines().next().unwrap_or_default());
                std::fs::write(reports.join(format!("{signature}.md")), report).unwrap();
            }
            println!("{} test(s) failed on their own", found.len());
            if found.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: valgrind tests <memcheck|helgrind> <dir> <build> <test binary> [args] | valgrind report <dir> \
         | valgrind failed <memcheck|helgrind> <dir> <build> <log>"
    );
    ExitCode::from(2)
}
