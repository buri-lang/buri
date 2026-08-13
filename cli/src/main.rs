//! The `buri` toolchain.
//!
//! One binary. It builds, runs, tests, formats, lints, generates build files,
//! and answers questions about the graph. There is no second tool to install,
//! no package manager, no task runner, and no configuration of the CLI itself
//! beyond `REPO.buri`.

use buri::build;
use buri::cli::{self, Flags, Session};
use buri::workspace::RuleKind;
use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match cli::parse(&argv) {
        Ok(a) => a,
        Err(msg) => {
            if msg.is_empty() {
                print!("{}", cli::USAGE);
                return ExitCode::from(if argv.is_empty() { 2 } else { 0 });
            }
            eprintln!("error: {msg}");
            eprintln!();
            eprint!("{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };

    let code = match args.command.as_str() {
        "build" => cmd_build(&args),
        "run" => cmd_run(&args),
        "test" => buri::testrun::cmd_test(&args),
        "format" => buri::tools::cmd_format(&args),
        "lint" => buri::tools::cmd_lint(&args),
        "gen" => buri::tools::cmd_gen(&args),
        "query" => buri::tools::cmd_query(&args),
        "clean" => cmd_clean(&args),
        "version" => cmd_version(&args),
        "lsp" => {
            eprintln!("error: `buri lsp` is not implemented in this toolchain");
            eprintln!("  = the analysis it would serve is the same one `buri build` runs");
            2
        }
        "help" | "--help" | "-h" => {
            print!("{}", cli::USAGE);
            0
        }
        other => {
            eprintln!("error: there is no command `{other}`");
            eprintln!();
            eprint!("{}", cli::USAGE);
            2
        }
    };
    ExitCode::from(code as u8)
}

fn open_or_exit(flags: &Flags) -> Result<Session, u8> {
    match cli::open(flags) {
        Ok(s) => Ok(s),
        Err(msg) => {
            eprintln!("error: {msg}");
            Err(2)
        }
    }
}

fn cmd_build(args: &cli::Args) -> i32 {
    let mut s = match open_or_exit(&args.flags) {
        Ok(s) => s,
        Err(c) => return c as i32,
    };
    // An unparseable build file is a problem with the invocation, not with the
    // code, so it exits 2.
    if s.report() {
        return 2;
    }
    let targets = match s.resolve_targets(&args.targets) {
        Ok(t) => t,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };

    let mut built = 0;
    let mut failed = false;
    for target in targets {
        // Only a binary produces an artifact; a library is checked, which is
        // what `buri build //lib/money` means.
        if target.kind == RuleKind::Library {
            let mut diags = buri::diag::Diagnostics::new();
            build::check_policy(&s, target, buri::buildfile::Platform::Js, &mut diags);
            if !diags.has_errors() {
                let unit = buri::compile::Unit {
                    target: Some(target),
                    platform: buri::buildfile::Platform::Js,
                    with_tests: false,
                };
                let analysis = buri::driver::analyze(Some(&s.ws), &mut s.map, &unit);
                diags.extend(analysis.diags.items);
            }
            failed |= s.print(&diags);
            continue;
        }
        for output in build::selected_outputs(&s, target, &args.flags) {
            match build::build_target(&mut s, target, &output, &args.flags) {
                Ok(a) => {
                    built += 1;
                    let rel = a.path.strip_prefix(&s.root).unwrap_or(&a.path);
                    let note = if a.cached { ", cached" } else { "" };
                    println!("{} ({} bytes{note})", rel.display(), a.bytes);
                }
                Err(diags) => {
                    failed |= s.print(&diags);
                }
            }
        }
    }
    if failed {
        return 1;
    }
    if built == 0 && args.flags.verbose {
        println!("nothing to build");
    }
    0
}

fn cmd_run(args: &cli::Args) -> i32 {
    let mut s = match open_or_exit(&args.flags) {
        Ok(s) => s,
        Err(c) => return c as i32,
    };
    if s.report() {
        return 2;
    }
    let targets = match s.resolve_targets(&args.targets) {
        Ok(t) => t,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };
    let binaries: Vec<_> = targets.iter().copied().filter(|t| t.kind == RuleKind::Binary).collect();
    if binaries.len() != 1 {
        eprintln!(
            "error: `buri run` takes exactly one binary, and this matched {}",
            binaries.len()
        );
        return 2;
    }
    let target = binaries[0];
    let outputs = build::selected_outputs(&s, target, &args.flags);
    // Builds it for the host configuration and executes it.
    let output = match outputs.iter().find(|o| {
        o.platform.as_ref().map(|p| p.value) == Some(buri::buildfile::Platform::Js)
    }) {
        Some(o) => o.clone(),
        None => {
            eprintln!("error: {} declares no JS output to run", s.ws.label(target));
            eprintln!("  = this toolchain emits JavaScript; add `{{ platform: JS }}` to outputs");
            return 2;
        }
    };

    let artifact = match build::build_target(&mut s, target, &output, &args.flags) {
        Ok(a) => a,
        Err(diags) => {
            s.print(&diags);
            return 1;
        }
    };

    // Outside the sandbox, with the real environment and the real filesystem.
    // That is the point of `run`: it is the one command that produces a
    // program with authority.
    let status = std::process::Command::new(buri::testrun::js_runtime())
        .arg(&artifact.path)
        .args(&args.passthrough)
        .status();
    match status {
        Ok(st) => st.code().unwrap_or(1),
        Err(e) => {
            eprintln!("error: cannot execute the artifact: {e}");
            eprintln!("  = `buri run` needs a JavaScript runtime; install bun, or set BURI_JS");
            2
        }
    }
}

fn cmd_clean(args: &cli::Args) -> i32 {
    let s = match open_or_exit(&args.flags) {
        Ok(s) => s,
        Err(c) => return c as i32,
    };
    let mut removed = Vec::new();
    let out = s.root.join(".buri/out");
    if out.exists() {
        let _ = std::fs::remove_dir_all(&out);
        removed.push(".buri/out");
    }
    if !args.flags.outputs_only {
        let cache = s.root.join(".buri/cache");
        if cache.exists() {
            let _ = std::fs::remove_dir_all(&cache);
            removed.push(".buri/cache");
        }
    }
    let _ = std::fs::remove_file(s.root.join("out"));
    if removed.is_empty() {
        println!("nothing to clean");
    } else {
        println!("dropped {}", removed.join(" and "));
    }
    // Reaching for `buri clean` to fix a build is worth reporting: the cache is
    // keyed on content, so a stale entry is a bug rather than a fact of life.
    0
}

fn cmd_version(args: &cli::Args) -> i32 {
    println!("buri {}", cli::VERSION);
    if args.flags.self_check {
        let mut map = buri::diag::SourceMap::new();
        let analysis = buri::driver::analyze_stdlib(&mut map);
        let mut errors = 0;
        for d in &analysis.diags.items {
            eprint!("{}", map.render(d, false));
            errors += 1;
        }
        if errors > 0 {
            eprintln!("the bundled standard library does not check");
            return 1;
        }
        println!("standard library: {} modules, checked", analysis.loaded.modules.len());
    }
    match cli::open(&args.flags) {
        Ok(s) => {
            let t = &s.ws.repo.toolchain;
            if t.version.is_empty() {
                println!("REPO.buri pins no toolchain version");
            } else {
                println!("REPO.buri pins {}", t.version);
                if t.version != cli::VERSION {
                    eprintln!(
                        "error: this repository pins {} but this toolchain is {}",
                        t.version,
                        cli::VERSION
                    );
                    eprintln!("  = an exact version, never a range: two checkouts of the same commit must not build with two different compilers");
                    return 2;
                }
            }
            0
        }
        // Outside a repository there is nothing to pin against, which is fine.
        Err(_) => 0,
    }
}
