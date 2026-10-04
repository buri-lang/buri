//! Pages and files the commands embed directly: each command's reference page,
//! the skills `buri add skills` installs, and the repository `buri init`
//! writes. They live here because a crate can only embed files inside its own
//! directory.

/// `reference/cli/<command>.md`, which `buri <command> --help` prints.
pub mod cli {
    pub const INIT: &str = include_str!("../docs/reference/cli/init.md");
    pub const BUILD: &str = include_str!("../docs/reference/cli/build.md");
    pub const TEST: &str = include_str!("../docs/reference/cli/test.md");
    pub const RUN: &str = include_str!("../docs/reference/cli/run.md");
    pub const FORMAT: &str = include_str!("../docs/reference/cli/format.md");
    pub const LINT: &str = include_str!("../docs/reference/cli/lint.md");
    pub const GEN: &str = include_str!("../docs/reference/cli/gen.md");
    pub const QUERY: &str = include_str!("../docs/reference/cli/query.md");
    pub const DOCS: &str = include_str!("../docs/reference/cli/docs.md");
    pub const ADD: &str = include_str!("../docs/reference/cli/add.md");
    pub const LSP: &str = include_str!("../docs/reference/cli/lsp.md");
    pub const CLEAN: &str = include_str!("../docs/reference/cli/clean.md");
    pub const VERSION: &str = include_str!("../docs/reference/cli/version.md");
}

/// `reference/skills/<name>.md`.
pub mod skills {
    pub const BURI_LANGUAGE: &str = include_str!("../docs/reference/skills/buri-language.md");
    pub const BURI_TYPES: &str = include_str!("../docs/reference/skills/buri-types.md");
    pub const BURI_BUILD: &str = include_str!("../docs/reference/skills/buri-build.md");
    pub const BURI_TESTING: &str = include_str!("../docs/reference/skills/buri-testing.md");
    pub const BURI_CLI: &str = include_str!("../docs/reference/skills/buri-cli.md");
}

/// `init/`, the repository `buri init` writes.
pub mod init {
    pub const REPO: &str = include_str!("../docs/init/REPO.buri");
    pub const GITIGNORE: &str = include_str!("../docs/init/gitignore");
    pub const GREETING_BUILD: &str = include_str!("../docs/init/libs/greeting/BUILD.buri");
    pub const GREETING_LIB: &str = include_str!("../docs/init/libs/greeting/lib.buri");
    pub const GREETING: &str = include_str!("../docs/init/libs/greeting/greeting.buri");
    pub const GREETING_TEST: &str = include_str!("../docs/init/libs/greeting/test/greeting.buri");
    pub const HELLO_BUILD: &str = include_str!("../docs/init/apps/hello/BUILD.buri");
    pub const HELLO_MAIN: &str = include_str!("../docs/init/apps/hello/main.buri");
}
