//! `cargo run -p website`. The work is in the library beside this file, so
//! that the tests drive the same code the command does.

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(website::run(&arguments));
}
