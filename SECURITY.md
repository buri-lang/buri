# Security

## The promise

The toolchain makes one promise about hostile input: **no input may panic the
compiler.** A malformed source file, build file, schema, flag, or
language-server message produces a diagnostic and a clean exit — never a Rust
panic, never a stack overflow, never a process killed by a signal. The lint set
at the top of [`Cargo.toml`](./Cargo.toml) is what pins it, and
`cli/tests/adversarial.rs` is what tries to break it, through the binary rather
than the library, asserting on *how* the toolchain stops rather than on what it
says.

The second promise the model rests on is that a build is reproducible: the same
commit, on any machine, produces byte-identical artifacts.
`buri docs build/hermeticity` states it and names the checks.

## Reporting

Use **GitHub's private vulnerability reporting** on
[`buri-lang/buri`](https://github.com/buri-lang/buri/security/advisories/new)
for anything with security impact:

- input that crashes, hangs, or exhausts memory in the compiler, the build
  system, or the language server, where the input could plausibly come from
  somewhere the reader does not control — a dependency, a pull request, an
  editor session over untrusted sources;
- a miscompilation whose effect is a security one: emitted code that violates a
  guarantee the language makes, an artifact that embeds something from the
  build machine, or a build that is not reproducible in a way an attacker could
  steer.

Include the input that produced it. A report is answered before it is public,
and credit is given unless you ask otherwise.

## What belongs in a public issue instead

An ordinary crash with no security impact is welcome as a normal
[issue](https://github.com/buri-lang/buri/issues) — the internal-compiler-error
message already asks for exactly that, with the input attached. So is a wrong
diagnostic, a wrong answer, or a compiler that disagrees with the
specification. Filing one of those publicly costs nothing and gets it fixed
faster.

If you are unsure which a finding is, report it privately. Nothing is lost by
being wrong in that direction.
