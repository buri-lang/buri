//! The scheduled memcheck job's tool (cli/tests/README.md, "Memcheck").
//!
//! ```text
//! cargo run -q -p buri --example memcheck -- tests <dir> <build> <test binary> [args]
//! cargo run -q -p buri --example memcheck -- report <dir>
//! ```
//!
//! `tests` runs a Rust test binary under memcheck with the flags the test
//! harness uses, less the leak check, and exits as it did. `report` writes a
//! draft issue per distinct error in `<dir>/issues`, prints each title, and
//! exits 1 when there was an error.

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

#[path = "../tests/native/memcheck.rs"]
mod memcheck;

use std::path::Path;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["tests", dir, build, program, rest @ ..] => {
            let mut cmd = Command::new(program);
            cmd.args(rest);
            // The runtime's tests are the one binary this runs; a build of them
            // with the `memcheck` feature is the one to run.
            let again = format!(
                "nix develop .#perf -c cargo test -p buri-rt-tests --features memcheck --no-run\n\
                 nix develop .#perf -c cargo run -p buri --example memcheck -- tests /tmp/memcheck '{build}' \
                 {program} {}",
                rest.join(" ")
            );
            let out = memcheck::run(Path::new(dir), &cmd, build, again.trim_end(), memcheck::UNIT_TESTS);
            print!("{}", String::from_utf8_lossy(&out.stdout));
            eprint!("{}", String::from_utf8_lossy(&out.stderr));
            ExitCode::from(u8::try_from(out.status.code().unwrap_or(1)).unwrap_or(1))
        }
        ["report", dir] => {
            let errors = memcheck::report(Path::new(dir));
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
        _ => {
            eprintln!("usage: memcheck tests <dir> <build> <test binary> [args] | memcheck report <dir>");
            ExitCode::from(2)
        }
    }
}
