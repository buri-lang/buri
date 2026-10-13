//! Compiling every example in every document.
//!
//! The examples are checked in the *topic* files under `crates/docs/src/docs/`, not in
//! the assembled `SPEC.md`, because the topic is the file somebody edits — a
//! failure that points at a generated file points at the wrong place. Assembly
//! is concatenation, so checking the topics checks the assembled document
//! exactly (`docs/documents.rs::the_assembled_documents_are_not_stale` keeps
//! the two in step). The root `README.md` is hand-written and no topic's copy,
//! so `readme_examples` compiles it where it sits.
//!
//! There is no per-document registration here: the tests walk
//! `topics::TOPICS`, so a new topic is subject to all of this the moment it
//! is registered.
//!
//! There is no way to leave a block out. One that needs more than a single
//! file to mean anything is a `file=` of a repository, and that repository is
//! built (`documentation::examples`).
use crate::shard;
use buri::documentation::{examples, layout, topics};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Where a topic's text lives on disk, for the location a failure reports.
///
/// The registry answers this rather than the id: a `build/*` topic's file is
/// under `reference/build/`, so concatenating the id would name nothing.
fn topic_path(topic: &topics::Topic) -> String {
    topic.path()
}

/// Every markdown document under `dir`, laid out, on a blessing run.
///
/// For the documents this suite enforces that are *not* compiled into the
/// binary: the worked monorepo's own pages, which
/// `documents::a_repository_can_test_its_own_documentation` runs `buri docs
/// test` over exactly as another repository would.
pub fn bless_documents_under(root: &Path, dir: &str) {
    if std::env::var_os("BURI_BLESS").is_none() {
        return;
    }
    let mut found = Vec::new();
    layout::documents_under(&root.join(dir), &mut found);
    for path in found {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        if let Some(out) = layout::format_document(&rel, &text) {
            std::fs::write(&path, out).unwrap();
        }
    }
}

/// One document's text — and, on a blessing run, the document laid out.
///
/// A page's text is `include_str!`d, so what a check sees is what the last
/// build compiled in. `BURI_BLESS=1` is the other direction: read the file as
/// it stands, lay out every fence in it (`documentation::layout`, which is what
/// `buri format` does to a document), write it back, and check *that* — so a
/// blessing run is green and the diff is what gets read.
///
/// Blessing rewrites fence bodies and nothing else. The prose is the author's.
pub fn document(root: &Path, rel: &str, compiled_in: &str) -> String {
    if std::env::var_os("BURI_BLESS").is_none() {
        // The README is not compiled into anything, so it is read where it
        // lives; every other document's text is the one the binary carries.
        if compiled_in.is_empty() {
            return std::fs::read_to_string(root.join(rel)).expect("the document exists");
        }
        return compiled_in.to_string();
    }
    let path = root.join(rel);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return compiled_in.to_string();
    };
    match layout::format_document(rel, &text) {
        Some(out) => {
            std::fs::write(&path, &out).unwrap();
            out
        }
        None => text,
    }
}

/// Where a standard-library module's text lives on disk.
///
/// The entry in `standard_library::MODULES` says what a module *is* and not
/// which file it was read from, and the two do not follow one another:
/// `core/net/http` is `sources/http.buri` and `ui/node` is
/// `sources/ui_node.buri`. Deriving a path from the module path named a file
/// that does not exist for eight of them, so a failure in one pointed at
/// nothing. This asks the only thing that cannot be wrong — the bytes.
fn source_path(root: &Path, text: &str) -> Option<String> {
    // The standard library's sources, and the bundled platforms' `platform.buri`.
    const DIRS: [&str; 2] = [
        "crates/stdlib/src/compiler/standard_library/sources",
        "crates/stdlib/src/platforms",
    ];
    let mut found = None;
    let mut stack: Vec<_> = DIRS.iter().map(|d| root.join(d)).collect();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).ok()? {
            let path = entry.ok()?.path();
            if path.is_dir() {
                stack.push(path);
            } else if std::fs::read_to_string(&path).is_ok_and(|t| t == text) {
                found = Some(path.strip_prefix(root).ok()?.display().to_string());
            }
        }
    }
    found
}

/// The same, for a source file whose examples are written in its `///` and
/// `//!` comments.
fn source_text(root: &Path, rel: &str, compiled_in: &str) -> String {
    if std::env::var_os("BURI_BLESS").is_none() {
        return compiled_in.to_string();
    }
    let path = root.join(rel);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return compiled_in.to_string();
    };
    match layout::format_doc_comments(&text) {
        Some(out) => {
            std::fs::write(&path, &out).unwrap();
            out
        }
        None => text,
    }
}

/// `buri docs test` over `documents`, run from the checkout's root the way
/// someone checking the toolchain's own pages runs it.
///
/// Fails with every failure at once — a fence at a time would make fixing a
/// document a dozen round trips.
pub fn docs_test(root: &Path, documents: &[String]) {
    let mut args = vec!["docs", "test", "--color=never"];
    args.extend(documents.iter().map(String::as_str));
    let out = crate::documents::ran(root, &args);
    assert!(
        out.status.success(),
        "\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Compiles every block of every topic of one kind.
fn check_kind(kind: topics::Kind) {
    let root = repo_root();
    let mut paths = Vec::new();
    for t in topics::TOPICS.iter().filter(|t| t.kind == kind) {
        let path = topic_path(t);
        document(&root, &path, t.text);
        paths.push(path);
    }
    assert!(!paths.is_empty(), "no topics of this kind");
    docs_test(&root, &paths);
}

#[test]
fn getting_started_examples() {
    check_kind(topics::Kind::GettingStarted);
}

#[test]
fn language_reference_examples() {
    check_kind(topics::Kind::Language);
}

#[test]
fn build_system_examples() {
    check_kind(topics::Kind::Build);
}

#[test]
fn reference_examples() {
    check_kind(topics::Kind::Reference);
}

#[test]
fn guide_examples() {
    check_kind(topics::Kind::Guide);
}

/// The root `README.md`. It is hand-written rather than assembled, so nothing
/// above reaches its examples and this is what keeps them true.
#[test]
fn readme_examples() {
    let root = repo_root();
    let text = document(&root, "README.md", "");
    // A README whose every fence stopped extracting would pass the assertion
    // below over nothing at all.
    let compiled = examples::extract("README.md", &text).blocks.len();
    assert!(
        compiled > 0,
        "no example extracts from README.md; this test has gone vacuous"
    );
    docs_test(&root, &["README.md".to_string()]);
}

/// The per-command pages. Their prose is hand-written, so it gets the same
/// treatment as everything else.
#[test]
fn cli_reference_examples() {
    let root = repo_root();
    let mut paths = Vec::new();
    for c in buri::commands::COMMANDS {
        let path = format!("crates/docs/src/docs/reference/cli/{}.md", c.name);
        document(&root, &path, c.doc);
        paths.push(path);
    }
    docs_test(&root, &paths);
}

/// The skill pages `buri add skills` installs, which the site serves under
/// `reference/skills/`. An agent copies their examples first.
#[test]
fn skill_examples() {
    let root = repo_root();
    let mut paths = Vec::new();
    let mut compiled = 0;
    for s in buri::commands::add::skills::SKILLS {
        let path = format!("crates/docs/src/docs/reference/skills/{}.md", s.name);
        let text = document(&root, &path, s.text);
        compiled += examples::extract(&path, &text).blocks.len();
        paths.push(path);
    }
    assert!(
        compiled > 0,
        "no example extracts from the skill pages; this test has gone vacuous"
    );
    docs_test(&root, &paths);
}

/// A census, so that a harness regression shows up as a changed count rather
/// than as every suite passing vacuously over zero blocks.
#[test]
fn the_examples_are_actually_extracted() {
    let (mut compiled, mut files) = (0, 0);
    for t in topics::TOPICS {
        let found = examples::extract(&topic_path(t), t.text);
        compiled += found.blocks.len();
        files += found.files.len();
    }
    eprintln!("{compiled} Buri example(s), {files} of them or of textproto files of a repository");
    assert!(
        compiled > 40,
        "only {compiled} examples are compiled; is the harness running?"
    );
    assert!(
        files > 3,
        "only {files} fences are files of a repository; is the harness running?"
    );
}

/// Every example written in a `///` or `//!` comment in the standard library.
///
/// The prose pages were already compiled by the tests above; this closes the
/// other half, because a documentation comment is documentation and an example
/// in one has the same claim on being true. `doc_comments` turns a source file
/// into a document with the source's own line numbers, so a failure points at
/// the `.buri` line the example is written on rather than at an offset into
/// something synthetic.
///
/// The modules are four tests, `standard_library_doc_comments::shard_0` to
/// `shard_3`, so nextest can run them side by side (`harness/shard.rs`).
fn doc_comments_shard(at: usize, count: usize) {
    let root = repo_root();
    let all = documented_modules(&root);
    let mine = shard::of(&all, at, count);
    let mut paths = Vec::new();
    let mut blocks = 0;
    for (rel, _, found) in mine.iter().copied() {
        blocks += found;
        paths.push(rel.clone());
    }
    let modules = mine.len();

    docs_test(&root, &paths);
    // A corpus that discovers nothing passes every assertion. The whole corpus
    // owes eight examples across four modules, and each shard its share of
    // that, rounded up.
    let (least_blocks, least_modules) = (8usize.div_ceil(count), 4usize.div_ceil(count));
    assert!(
        blocks >= least_blocks && modules >= least_modules,
        "shard {at} of {count} has only {blocks} example(s) across {modules} module(s), \
         under its share of {least_blocks} across {least_modules}; the extractor is missing them"
    );
}

shards! {
    standard_library_doc_comments(doc_comments_shard, documented_module_count) =
        shard_0 shard_1 shard_2 shard_3;
}

/// Every standard library module with an example in its doc comments: where it
/// lives, the document its comments make, and how many examples are compiled.
fn documented_modules(root: &Path) -> Vec<(String, String, usize)> {
    let mut out = Vec::new();
    for module in buri::compiler::standard_library::MODULES {
        // The source is a field of the entry rather than a second table keyed
        // by path, so a listed module with no source is unrepresentable.
        let compiled_in = module.source;
        if !examples::has_examples(compiled_in) {
            continue;
        }
        // The name a failure reports: where the module actually lives, so the
        // line number is one an editor can open.
        let Some(rel) = source_path(root, compiled_in) else {
            continue;
        };
        let text = examples::doc_comments(&source_text(root, &rel, compiled_in));
        let extracted = examples::extract(&rel, &text);
        let found = extracted.blocks.len();
        // A fence that does not extract is a failure to report, not a module
        // with nothing in it.
        if found == 0 && extracted.failures.is_empty() {
            continue;
        }
        out.push((rel, text, found));
    }
    out
}

fn documented_module_count() -> usize {
    documented_modules(&repo_root()).len()
}

/// How many fences the formatter is allowed to have nothing to say about.
///
/// A page about a syntax error shows the syntax error, and the formatter
/// refuses text it could not read whole — so a silence is expected and a
/// *growing* number of them is not. The ceiling is one number: it may be
/// lowered and not raised, and raising it is a line in the same diff as the
/// fence that needed it.
const MAX_UNCHECKED_LAYOUTS: usize = 90;

/// **Every example is laid out the way `buri format` lays out source**, and the
/// ones that cannot be are counted.
///
/// The enforcement itself is inside `examples::run_file_at`, so every test
/// above already fails on a fence that has drifted, and so does `buri docs
/// test` in any repository. What this adds is the census: a check that passes
/// because it checked nothing is the failure mode a layout rule has, and the
/// floor and the ceiling here are what rule it out.
#[test]
fn every_example_is_laid_out_the_way_the_formatter_writes_source() {
    let root = repo_root();
    let mut clean = 0;
    let mut silent = Vec::new();
    let mut drifted = Vec::new();

    // On a blessing run this is also where the catalog pages are laid out: the
    // error pages are the only ones another test reads, and the lint pages are
    // read by this census alone.
    let mut census = |file: &str, text: &str| {
        for block in examples::extract(file, text).blocks {
            match layout::verdict(&block) {
                layout::Verdict::Clean => clean += 1,
                layout::Verdict::Silent(why) => silent.push(format!("{}: {why}", block.origin)),
                layout::Verdict::Drifted(_) => drifted.push(block.origin.to_string()),
            }
        }
        // A blessing run rewrites a page with `format_document`, textproto
        // fences included, so a page it would rewrite is one this run fails.
        if file.ends_with(".md") && layout::format_document(file, text).is_some() {
            drifted.push(format!("{file}: a fence `buri format` would rewrite"));
        }
    };

    for t in topics::TOPICS {
        census(&topic_path(t), &document(&root, &topic_path(t), t.text));
    }
    for c in buri::commands::COMMANDS {
        let rel = format!("crates/docs/src/docs/reference/cli/{}.md", c.name);
        census(&rel, &document(&root, &rel, c.doc));
    }
    // The catalogs. Their pages are markdown like any other, and the program on
    // an error page is the one a reader copies to reproduce the error.
    for e in buri::documentation::errors::ERRORS {
        let rel = format!("crates/docs/src/docs/reference/errors/{}.md", e.code);
        census(&rel, &document(&root, &rel, e.text));
    }
    for l in buri::documentation::lints::LINTS {
        let rel = format!("crates/docs/src/docs/reference/lints/{}.md", l.code);
        census(&rel, &document(&root, &rel, l.text));
    }
    census("README.md", &document(&root, "README.md", ""));
    for module in buri::compiler::standard_library::MODULES {
        if !examples::has_examples(module.source) {
            continue;
        }
        let rel = source_path(&root, module.source).expect("the module's file");
        census(&rel, &examples::doc_comments(module.source));
    }

    assert!(
        drifted.is_empty(),
        "{} example(s) are not laid out the way `buri format` lays out source:\n  {}\n\
         `BURI_BLESS=1 cargo test -p buri --test docs` rewrites the fence bodies, \
         and nothing else on the page.",
        drifted.len(),
        drifted.join("\n  ")
    );
    assert!(
        clean > 190,
        "only {clean} example(s) were laid out and checked; the census has gone vacuous"
    );
    assert!(
        silent.len() <= MAX_UNCHECKED_LAYOUTS,
        "{} example(s) have no canonical layout, and the ceiling is \
         {MAX_UNCHECKED_LAYOUTS}:\n  {}",
        silent.len(),
        silent.join("\n  ")
    );
    eprintln!(
        "{clean} example(s) laid out, {} the formatter has nothing to say about",
        silent.len()
    );
}

/// Runs a synthetic document and returns its failures, rendered.
fn check(doc: &str) -> String {
    // The repository root, as the docs' own `repo=` paths are written, and
    // not the working directory, which depends on how the test is run.
    examples::report(&examples::run_file_at(&repo_root(), "test.md", doc))
}

#[test]
fn a_whole_module_compiles() {
    let doc = "```buri\n\
               export fn double(n: Int): Int {\n    n * 2\n}\n\
               ```\n";
    assert_eq!(check(doc), "");
}

#[test]
fn a_statement_fragment_compiles_with_a_context() {
    let doc = "```buri wrap=body\n\
               let a = 5;\n\
               let _ = io.println(ctx, \"a is ${a}\").ignore();\n\
               ```\n";
    assert_eq!(check(doc), "");
}

#[test]
fn a_signature_list_compiles() {
    let doc = "```buri sig\n\
               export fn allocate(bytes: Int): Int;\n\
               ```\n";
    assert_eq!(check(doc), "");
}

/// A method signature needs its `impl`, which a document can supply with
/// hidden lines rather than a fourth wrapper mode.
#[test]
fn a_method_signature_list_compiles_inside_a_hidden_impl() {
    let doc = "```buri sig\n\
               # export struct Ring(export Int);\n\
               #\n\
               # impl Ring {\n\
               \x20   export fn size(self): Int;\n\
               # }\n\
               ```\n";
    assert_eq!(check(doc), "");
}

/// A block documenting a method on a *built-in* cannot be compiled where
/// it is written: a type's operations may only be declared in its defining
/// module (SPEC 6.7.3). Those blocks belong in the generated standard
/// library reference, which renders them from `crates/stdlib/src/compiler/standard_library/sources/*.buri`; until
/// a document is converted, they carry `ignore` and a reason. This test
/// pins the rule so the reason stays true.
#[test]
fn a_builtin_impl_cannot_be_documented_as_a_compilable_block() {
    let doc = "```buri sig\n\
               # impl<T> [T] {\n\
               export fn length(self): Int;\n\
               # }\n\
               ```\n";
    assert!(check(doc).contains("defining module of `[T]` is `core/list`"));
}

#[test]
fn a_broken_example_is_reported_at_its_own_line() {
    let doc = "prose\n\nmore prose\n\n```buri wrap=body\n\
               let a: Int = \"not an int\";\n\
               ```\n";
    let out = check(doc);
    assert!(out.contains("does not compile"), "{out}");
    // The fence body starts on line 6 of the document.
    assert!(
        out.contains("test.md:6:"),
        "location should point at the fence:\n{out}"
    );
}

#[test]
fn a_fail_block_must_actually_fail() {
    let good = "```buri fail wrap=body\n\
                let a: Int = \"not an int\";\n\
                ```\n\
                ```error\n\
                expected `I64`, found `Str`\n\
                ```\n";
    assert_eq!(check(good), "");

    let bad = "```buri fail wrap=body\n\
               let a = 5;\n\
               ```\n\
               ```error\n\
               expected `I64`, found `Str`\n\
               ```\n";
    assert!(
        check(bad).contains("compiles"),
        "a compiling `fail` block must be caught"
    );
}

#[test]
fn an_error_annotation_is_compiled_on_its_own() {
    let doc = "```buri wrap=body\n\
               let ok: Int = 5;\n\
               let bad: Int = \"nope\"; // ERROR: expected `I64`, found `Str`\n\
               let alsoOk = ok + 1;\n\
               ```\n";
    assert_eq!(
        check(doc),
        "",
        "the annotated line should satisfy its own claim"
    );
}

#[test]
fn an_error_annotation_that_does_not_hold_is_caught() {
    let doc = "```buri wrap=body\n\
               let ok: Int = 5;                // ERROR: expected `I64`, found `Str`\n\
               ```\n";
    let out = check(doc);
    assert!(out.contains("should not compile"), "{out}");
}

#[test]
fn a_named_block_is_reusable() {
    let doc = "```buri name=pt\n\
               export struct Point {\n    export x: Int,\n    export y: Int,\n}\n\
               ```\n\
               ```buri use=pt wrap=body\n\
               let p = Point { x: 1, y: 2 };\n\
               let _ = io.println(ctx, \"${p.x}\").ignore();\n\
               ```\n";
    assert_eq!(check(doc), "");
}

#[test]
fn a_harness_is_reusable() {
    let doc = "```buri use=shapes wrap=body\n\
               let a = area(.Circle(2.0));\n\
               let _ = io.println(ctx, \"${a}\").ignore();\n\
               ```\n";
    assert_eq!(check(doc), "");
}

/// Every example is compiled, so there is no mode that leaves one out.
#[test]
fn there_is_no_way_to_leave_an_example_out() {
    let doc = "```buri ignore\nwhatever\n```\n";
    assert!(
        check(doc).contains("`ignore` is not a mode"),
        "{}",
        check(doc)
    );
    let doc = "```textproto ignore\nname: \"api\"\n```\n";
    assert!(
        check(doc).contains("`ignore` is not a mode"),
        "{}",
        check(doc)
    );
    let doc = "```textproto\nname: \"api\"\n```\n";
    assert!(
        check(doc).contains("does not say what it is"),
        "{}",
        check(doc)
    );
}

/// The fences labelled as files make one repository, and it is built: a
/// build file naming an entry is what makes that function an entry.
#[test]
fn a_page_s_files_are_built_as_one_repository() {
    let build = "```textproto schema=build file=cmd/hello/BUILD.buri\n\
                 binary {\n    outputs: [\n        { platform: \"node\", entries: [{ name: \"main\", function: \"start\" }] },\n    ]\n}\n```\n";
    let main = "```buri file=cmd/hello/main.buri\n\
                from \"node\" import { NodeHost };\n\n\
                export fn start(host: NodeHost): Result<(), Str> {\n    .Ok(())\n}\n```\n";
    assert_eq!(check(&format!("{build}\n{main}")), "");

    let wrong = main.replace("export fn start", "export fn begin");
    let out = check(&format!("{build}\n{wrong}"));
    assert!(out.contains("this repository does not build"), "{out}");
}

/// A `fail` fence is written in on its own and must fail the build with
/// its code; one that builds is caught.
#[test]
fn a_failing_file_must_fail_with_its_code() {
    let broken = "```textproto fail code=textproto-syntax repo=cli/tests/docs/repositories/deploy \
                  file=lib/deploy/server.txtpb\n\
                  # proto-file: server.proto\n# proto-message: Server\nports: [80 443]\n```\n";
    assert_eq!(check(broken), "");

    let fine = broken.replace("[80 443]", "[80, 443]");
    let out = check(&fine);
    assert!(
        out.contains("does not fail the build with `textproto-syntax`"),
        "{out}"
    );
}

/// A fence whose info string does not parse used to be silently downgraded
/// to "a fence in no language", which meant it was neither compiled nor
/// reported — the worst of both. It is now a failure whatever it claims to
/// be.
#[test]
fn a_malformed_info_string_is_reported_rather_than_swallowed() {
    let doc = "```buri file=\"unterminated\nwhatever\n```\n";
    assert!(check(doc).contains("unterminated"), "{}", check(doc));

    // Not only for `buri`: nothing else can be trusted to notice.
    let doc = "```textproto schema=build schema=repo\nx: 1\n```\n";
    assert!(check(doc).contains("given twice"), "{}", check(doc));

    // And a well-formed fence in a language nobody compiles stays silent.
    assert_eq!(check("```text\nfree prose\n```\n"), "");
}

#[test]
fn a_run_block_must_pin_its_output() {
    let doc = "```buri run wrap=body\nlet _ = io.println(ctx, \"hi\").ignore();\n```\n";
    assert!(check(doc).contains("```stdout"));
}

/// The end-to-end case: compile, execute under the JS runtime, and
/// compare stdout with what the document promises.
#[test]
fn a_run_block_executes_and_its_output_is_pinned() {
    let good = "```buri run wrap=body\n\
                let _ = io.println(ctx, \"basket total: $36.50\").ignore();\n\
                ```\n\
                ```stdout\n\
                basket total: $36.50\n\
                ```\n";
    assert_eq!(check(good), "");

    let wrong = "```buri run wrap=body\n\
                 let _ = io.println(ctx, \"one thing\").ignore();\n\
                 ```\n\
                 ```stdout\n\
                 another thing\n\
                 ```\n";
    assert!(
        check(wrong).contains("prints something else"),
        "a wrong transcript must be caught"
    );
}
