//! Whole repositories, each provoking one build-system rule.
//!
//! The `reject/` corpus builds every case as a single-package binary with no
//! dependencies, so nothing in it can express a diagnostic *about the graph* —
//! which is most of what the build system checks. Each case here is instead a
//! small repository checked in whole, with a manifest naming what the CLI does
//! in it and what that must print. See `harness/case.rs` for the format.
//!
//! One test per specification document, so cargo runs them on separate threads
//! and a failure names the document to open.
//!
//! ```text
//! BURI_BLESS=1 cargo test -p buri --test build repositories::    # record the goldens
//! BURI_KEEP=1  cargo test -p buri --test build repositories::    # keep the scratch trees
//! ```
use crate::harness::*;
use std::path::{Path, PathBuf};

/// BUILD-FILES.md: what a rule declares, and the diagnostics that fire when
/// the declaration and the code disagree.
#[test]
fn build_file_rules() {
    run_corpus(&tests_dir().join("repositories/build-files"), "build-files", 14);
}

/// LIBRARIES.md: `lib.buri` is a library's entire public surface, and the
/// boundary it draws applies to methods as much as to names.
#[test]
fn library_boundaries() {
    run_corpus(&tests_dir().join("repositories/libraries"), "libraries", 11);
}

/// TAGS.md: a tag is a property of a whole dependency closure, and the two
/// things that follow from one — what may not sit beside it, and where it may
/// be built.
#[test]
fn tag_policy() {
    run_corpus(&tests_dir().join("repositories/tags"), "tags", 7);
}

/// CLI.md: the exit codes, and the commands whose contract is about what they
/// leave on disk rather than what they compute — `gen`, `run`, `clean`,
/// `version`, `init`, `add skills`, the `out/` symlink, and the no-argument
/// forms that mean the whole repository from wherever they are run.
///
/// The five `init_*` cases are the whole of what that command promises about
/// files it did not write: the scaffold into a directory that has none, the
/// three shapes an existing `.gitignore` comes in, and the refusal every other
/// collision still gets.
///
/// `dense_diagnostics` is the flag half of what a diagnostic reads like: every
/// error carries its explanation page under it, and `--dense` is the way to
/// ask for the heading and the fix without the prose. Two runs over one
/// unchanged source, so the diff between the recorded reports is the flag and
/// nothing else.
#[test]
fn cli_contract() {
    run_corpus(&tests_dir().join("repositories/cli"), "cli", 19);
}

/// CLI.md's `query`: what the graph says, asked without building anything.
/// Its own corpus because the answers are the recorded output rather than a
/// diagnostic — a query that has stopped working prints a plausible wrong
/// answer rather than failing.
#[test]
fn graph_queries() {
    run_corpus(&tests_dir().join("repositories/query"), "query", 1);
}

/// PROTO.md: a `.proto` schema is a source that becomes a module. One case for
/// the build-file half — declared, placed by `gen`, keyed on its contents,
/// internal to the rule that declared it — one for the edition the reader
/// requires and the two syntaxes it refuses, and one for each half of what it
/// otherwise refuses: the constructs that are out of scope, and the files that
/// are not schemas at all. The sixth is `google.protobuf.Any`, which is a
/// message like any other here and is resolved by name rather than recognised.
/// The seventh is the platforms: one schema behind a `LINUX`, a `MACOS`, a
/// `JS`, a `WEB` and a `CLOUDFLARE_WORKER` output, because a generated module
/// is compiled once per platform and each of those is a compile that can fail
/// on its own.
///
/// The eighth is the generated *JSON* codec under a plain `buri test`, which is
/// the native backend on a host that has one. A JSON number reaches a decoder
/// as an `F64`, so every integer field converts through `number.F64.toI64`, and
/// the suite reported `1 failed to compile` until `emit.rs` grew the inexact
/// conversions (buri-lang/buri#43).
#[test]
fn proto_schemas() {
    run_corpus(&tests_dir().join("repositories/proto"), "proto", 8);
}

/// BUILD-FILES.md's `generators`: a program the build runs, whose output
/// becomes a module. One case for the rule itself — declared inputs, the
/// `generate` action, the cache, reproducibility, the module's internality, and
/// the two ways a tool can fail — one for what a generator says about its
/// input, and one for a tool built from the target that runs it.
///
/// Two more are the edges of each half. `generator_shapes` is what a tool may
/// hand back: no modules, two of them with one importing the other, an entry
/// with no inputs at all, noise on either stream, an answer followed by a
/// non-zero exit, and the three names a generated module may not take.
/// `generator_tools` is what an entry may *name*: no tool, a generator this
/// toolchain does not ship, a label that is no binary, an input that is not
/// there, and `proto_sources` on a binary.
///
/// `origins_at_the_edges` is the third: an origin is a byte range in a file,
/// and the ends of one are where a caret is easiest to get wrong. Five spans —
/// the first byte, the last byte, one that begins where the file ends, one well
/// past it, and one naming a file the repository does not have — over a
/// two-line input, a one-byte input, and an empty one.
///
/// `the_printer_round_trips` is the fourth, and it is about `core/buri/ast`
/// rather than about the rule: a generator builds a module out of nodes,
/// `core/codegen` prints it, the compiler parses and checks the text, and the
/// program that runs it gets the same answers as a byte-identical module a
/// person wrote by hand. Between the two halves it reaches every construct
/// `std/codegen/proto` never writes, so a node kind the printer wrote wrongly
/// fails as a program that does not compile rather than as a string nobody
/// re-read.
#[test]
fn generators() {
    run_corpus(&tests_dir().join("repositories/generators"), "generators", 7);
}

/// CLI.md's lint catalogue: the hygiene rules, which ask about a package's own
/// code rather than about the graph. Each case ends with the edit that makes
/// the finding go away, because a rule nothing can turn off is a rule nobody
/// can check the fix for.
///
/// The `repo_lint_*` cases are the other half: what `REPO.buri`'s `lint` block
/// does to the same finding — when the catalogue runs, how hard a finding
/// lands, and what a misspelled field in the block costs.
///
/// Eleven of them are about a file the front end had something to say about,
/// and together they draw the line the rules stay behind. Six are about the
/// shape of the silence: what is still reported around a file that did not
/// parse, what is rightly not reported inside the declaration that did not,
/// that a package's neighbour going quiet does not quiet it, that a broken
/// file *underneath* a package does not quiet the two packages above it, that
/// a second broken file is not a second reason to stop, and that a *build*
/// file which does not read is the one thing recovery does not read around.
///
/// Nine more are the dead-code family — a type, a field and a variant nothing
/// uses — and four of them are negatives: a type named only by a signature or
/// an alias, a field read only by the module beside it, a type built only by
/// literals that name no type at all, and everything on the library's surface,
/// none of which is reported. The rest are what is: the field elision leaves
/// out of every literal, the variant a `_` arm meets and no shorthand builds,
/// and the two shapes of doubt — an unresolved re-export, which reaches the
/// exported names and no further, and a body that did not check, which reaches
/// the names written inside it and no further.
///
/// The anonymous-literal case is the one that answers a grammar change rather
/// than a rule: a bare `{ … }` builds a struct while naming nothing, so the
/// token half of the census cannot see the construction and the typed tree is
/// the whole of the evidence. It pins both directions at once — the type is
/// alive, including the private one no surface could have exempted, and the
/// field the literal fills is still reported, because filling a field in is
/// not reading it whether or not the literal wears a head.
///
/// The other five say what an error is *not* a reason to go quiet about, which
/// is the harder half and the one that regressed. A declaration the parser
/// recovered whole — an import missing its `;` — hides nothing below it. An
/// error about one declaration the parser read whole — an alias that closes a
/// cycle — hides nothing beside it. A body that did not check hides its own
/// bindings and not its neighbour's. And the two things that genuinely do hide
/// something hide exactly what they cost: a run of declarations the parser
/// skipped hides what it swallowed and nothing that merely sits near it, and a
/// re-export that did not resolve stops `dead-code` from calling the name it
/// meant to reach unreached — one typo, one finding.
///
/// The generated half of that question is `cli/tests/linting.rs`, six hundred
/// of them, with the rate of lost findings pinned as an invariant and the
/// parity between this command and the language server stated alongside it.
///
/// Five more are about **where** the catalogue runs, which is everywhere a
/// package's own code is: a suite and a `testing/` helper are code, and both
/// are held to every rule the library beside them is. Two of them fire the
/// whole general half once inside a test source and once inside a testing
/// source; one shows `--fix` rewriting both kinds of file; one shows an edit to
/// either invalidating the lint record; and the last shows that a lint report
/// carries the checker's errors as well as the catalogue's findings, in all
/// three kinds of source, which is more than `buri build` can say because
/// `buri build` compiles only one of them.
#[test]
fn lint_catalogue() {
    run_corpus(&tests_dir().join("repositories/linting"), "linting", 60);
}

/// TESTING.md: where tests live, what a test source may reach, and what the
/// runner does with a suite — the flags, the timeout, the golden-file update
/// mode, the exact shape of a failure report, and the verdict a suite that
/// never compiled gets.
///
/// `a_suite_over_the_ast` is where "a suite that names no platform runs
/// natively" is a claim about a real program rather than about the runner: a
/// module is a `[ast.Item]`, an `ast.Item` is 448 bytes, and the stencil
/// backend staged a `[T]` element in a fixed 320 — so every suite that reached
/// `core/buri/ast` was a suite that did not compile (buri-lang/buri#48).
#[test]
fn test_suites() {
    run_corpus(&tests_dir().join("repositories/testing"), "testing", 13);
}

/// The concurrency-and-servers surface, driven the way a person drives it: a
/// package in a repository, a suite beside it, and one `buri` command.
///
/// **Why these claims are here as well as in the conformance corpus.** That
/// corpus is where a language semantic lives and it is run on every backend,
/// with `platforms: [JS]` on each package so the reference run stays the
/// reference one — but every one of its readers is a harness of this
/// repository's own. A repository case is the other thing: `buri test`, a real
/// package graph, and on a toolchain with a native backend a linked binary
/// that ran. `cli/tests/README.md`'s "The trust ordering" is the argument, and
/// F-1 is the incident behind it — the tier under an end-to-end claim stayed
/// green for two waves while the claim itself had quietly stopped being made.
///
/// Every case ends with the edit that makes its subject's **failure** visible,
/// so what is recorded is the real answer and not an assertion about one.
#[test]
fn concurrency_and_memory() {
    run_corpus(&tests_dir().join("repositories/concurrency"), "concurrency", 7);
}

/// The host a program is handed, on the platform this toolchain does not build
/// binaries for: node.
///
/// Everything `core/fs`, `core/env` and `core/process` reach has three
/// answers — `core/host/testing`'s doubles in the conformance corpus, a real
/// Linux or macOS in `native::e2e`, and node's own `fs`, `process` and
/// `child_process` here. The third one had no test at all, and a runtime
/// function nothing calls is a runtime function nobody notices going wrong.
///
/// A repository case rather than a conformance one, for
/// `concurrency_and_memory`'s reason and one more: a conformance block binds a
/// double, and the whole claim here is about the host that is *not* a double.
/// `buri run` is the command that hands a program the real one.
#[test]
fn the_host_on_node() {
    run_corpus(&tests_dir().join("repositories/platform"), "platform", 1);
}

/// `buri run` on a page: the flags that belong to the server it starts, and
/// the build failure that means there is no server at all.
///
/// The listening half cannot be a step here — a manifest step runs a command
/// to completion, and a server runs until it is stopped — so it is
/// `build::serving`, which spawns `buri run` against this same `repo/` and
/// talks to it. What is left for a manifest is what finishes: the artifact the
/// server answers from, the two refusals, and the page that will not compile.
#[test]
fn serving_a_page() {
    run_corpus(&tests_dir().join("repositories/serving"), "serving", 1);
}

/// The user interface at tier 2: the reactive graph, and a tree painted to a
/// PNG and compared byte for byte.
///
/// A group of its own rather than a case under `testing/`, because what it
/// asserts is not what a suite reports — it is what the *renderer* produced,
/// and the assertion is a picture checked into the repository. That picture is
/// the only claim anywhere that `cli/runtime/paint.rs` paints the same bytes on
/// Linux and on macOS: a golden that did not would go red on one CI host and
/// green on the other.
///
/// It is a repository case rather than a conformance one for the reason
/// `concurrency_and_memory` is. The conformance corpus runs on every backend
/// and its `ui` package says `platforms: [JS]`, and a snapshot has no
/// JavaScript answer at all — there is no painter there. So the only tier that
/// can ask this question is the one that links a real binary and runs it.
///
/// Ten cases pin the machinery. Six are one axis of the snapshot each: the whole lifecycle over
/// one picture; the range of every axis a snapshot has over twenty-one; every
/// way a comparison cannot be made; the invocation — `buri test` with no target
/// at all — that puts two packages' suites in one binary; the platform, where a
/// `platforms: [JS]` suite runs the graph and is refused the picture; and the
/// theme, where one card is painted light and dark and swapping the two lists
/// fails both comparisons.
///
/// A seventh is a tree rather than a picture: `describe` under both backends,
/// with JavaScript as the oracle, over a tree that exists only inside the
/// closure `ui.computed` captured. It is here because it is the same corpus's
/// subject — a `ui/node` tree, through `buri test` — and because the tier
/// below it cannot ask the question: `conformance/lib/ui` is `platforms: [JS]`,
/// so a divergence between the two backends is invisible there.
///
/// The seventh is the graph rather than the painter: a `Signal<[T]>`, whose
/// value is a list like any other and whose two readings of "which type is `T`"
/// both backends used to get wrong.
///
/// The eighth is what a snapshot is built on, and the only case here that
/// declares no `platforms` at all: the reactive graph and `describe`'s tree
/// walk, run natively, plus the diagnostic a suite gets when it reaches a
/// `ui/testing` facility the native backend has no body for. Both bugs it pins
/// reported as an abort with no message, which is a shape no lower tier can
/// see — the binary linked, the front end was happy, and the process died.
///
/// The ninth is the icon: a PNG data URI in a colour type the painter's own
/// encoder does not write, and one SVG glyph in both forms a page writes one
/// in, painted at their own colours. Its last step moves one glyph's stroke
/// from red to blue, which is the record that a golden with an icon in it can
/// now fail on the icon rather than only on where the box around it sat.
///
/// The tenth is the **page** the other nine are painted on, and it is the one
/// case whose assertion is a picture's size rather than its pixels: a button
/// that comes out 800x40, forty rows that come out 800x1592 with the fortieth
/// in the picture, a translate and a pinned dock that widen the canvas to 880
/// while the layout inside stays 800 wide, a scroll container measured at its
/// own box, and one tree at two widths through `snapshotWide` and `snapshot`,
/// where `At(.Small)` applies at 800 and not at 390.
///
/// Beside them are the **sweeps**, which ask a different question: not whether
/// the machinery works, but whether each property in the vocabulary paints
/// what the stylesheet promises. `sweep_layout_*` is nine cases and forty-four
/// pictures, one per layout property, each a labelled grid of every value that
/// property has on the eight-hundred-wide page the viewport is.
/// `sweep_layout_bleed` is the one that reads as two components rather than as
/// a grid, because that is what its property is for: the full-width rule
/// inside a padded menu, and the avatar group whose children lap the one
/// before them. `sweep_layout_reverse` is the two reversed stacks against the
/// two plain ones — the same four children in every box, so what moves down a
/// picture is only the paint, and the alignments run the other way with them.
/// `sweep_layout_clip` is the one whose finding is two pictures that *agree*:
/// `Clip(true)` and `Scroll(.Both)` paint the same box, and what the newer
/// property buys is not a different picture but not becoming a scroll
/// container a keyboard can land in. They are
/// generated rather than written, so an enumeration is the whole of a
/// `ui/style` enum by construction, and every picture was read pixel by pixel
/// against the CSS its classes lower to. Two pictures hold a row recorded as
/// painted and known wrong; each case's own doc names the issue and says to
/// re-record when it closes.
///
/// `sweep_length_em` and `sweep_paint_translate` are the two that sweep a
/// value rather than a property. The em pictures are each an em row over a rem
/// row — half an em of padding at four text sizes, five tracking values, a
/// `FontSize` in em nested four deep, and the six other properties a length
/// reaches — because a unit that follows the element's own type is only visible
/// beside one that does not. The translate pictures draw every box inside the
/// dashed outline of the slot the layout gave it, which is the only way a shift
/// that moves nothing else can be read: every unit, the three neighbours that
/// stay where they were against the same row padded instead, and the button
/// that sinks a pixel, at rest and held.
///
/// The `sweep_themes_*` cases are the theme, breakpoint and reactive sweep:
/// the four colour slots a design token can fill on five primitives under five
/// theme lists, the four breakpoints on the page width `snapshot` takes,
/// every derived tier painted either side of a write, what an image paints
/// when nothing may be fetched, and the ends of every axis the tree has.
///
/// `a_border_and_a_ring_land_where_css_puts_them` is four pictures and three
/// rules a browser follows and this painter did not: a border of any width
/// sits inside its box, a border with no colour of its own is the foreground,
/// and an outer shadow is painted outside the box that cast it. One row per
/// rule, because none of the three is visible in a picture of something else —
/// a one-pixel border reads as a grey smudge, a black border reads as a
/// choice, and a ring reads as a fill. The shadow rule gets two pictures: the
/// spread-only ring, and Tailwind's `shadow-xs` on a transparent box, because
/// the lift a design system ships has a blur and an offset and no spread at
/// all, and a picture of the loudest shape does not say what happens to the
/// quietest.
///
/// `sweep_paint_box_sizing` is the box model, in two pictures: a blue box
/// inside a grey frame of a known width, plain, padded, bordered and both, so
/// any blue to the right of the grey is a box that measured its content rather
/// than itself. It is what closes the gap between the painter's border box and
/// CSS's initial `content-box`.
///
/// The `sweep_states_*` cases are the states sweep: every `State` against every
/// interactive primitive, six pictures to a case — the resting one and one per
/// state — each a labelled grid of `Background`, `Foreground`, `Border`,
/// `Opacity` and `Shadow`, then all five at once, then a control that answers to
/// nothing. Beside every picture is a `describe` assertion naming the classes
/// that state applied, and every case's last step moves one number in the hover
/// palette so exactly one of its six goldens fails. That is the tier-2 record
/// that a state golden is not a copy of the resting one.
/// `sweep_states_focus_within` is the sixth state and the odd one, because it
/// is the container's rather than the element's: an input group, the same
/// wrapper ringed on `Focus` instead, and a card, in four pictures — so the row
/// that answers in one picture is the row that stays quiet in the next.
///
/// `sweep_states_invalid` is the same grid for the state a *program* enters
/// rather than the platform: `field` and `toggle` take an `invalid`, and it
/// writes the `aria-invalid` the rule hangs off. Its third picture is the
/// failing tree painted hovered, and it comes out byte for byte the resting
/// one — which is the claim that a state is read back off the conflict slot
/// and not off the tree.
///
/// **The `sweep_properties_*` cases are the matrix under those ten**: every
/// `ui/style` property that paints something, at three to five values each,
/// against every node primitive — a case per primitive, six pictures in it,
/// one per family of properties, and a seventh under `field` for the six kinds
/// a field can be. A picture is a labelled grid, so a property is a row and its
/// values read across it, and each case ends with the edit that has to fail.
/// `text` and `image` take no styles of their own, so their column styles the
/// container, which is where a property on either of them goes anyway.
///
/// A property that stops being painted turns its row into a row of identical
/// cells rather than going quiet, which is what makes a grid worth more here
/// than a picture per value.
///
/// The `sweep_widgets_*` cases ask the question a property sweep cannot: what a
/// widget *is*, rather than what a style does to it. `button_children` is what a
/// button holds — none, one, a mark beside a word, a subtree — and one rail
/// painted resting and hovered, because the claim is that a wash covers the mark
/// and the word together now that they are one element. `control_wrapper` is the
/// two boxes a labelled control is: every layout property that only works on the
/// `<label>`, tried there and then on the control instead, where it does
/// nothing. `toggle_marks` is the tick and the thumb the widget draws for
/// itself, each one off and on, so a picture says which state a toggle is in
/// rather than only what colour it is. `range` is the third widget that draws
/// itself: the bar and the thumb a slider is, at every value, at every size,
/// and under everything that paints them — including the values a range cannot
/// hold, which HTML sanitizes into the bounds rather than refusing.
#[test]
fn snapshots() {
    run_corpus(&tests_dir().join("repositories/ui"), "ui", 58);
}

/// The language server. Each case is a recorded session: requests in, decoded
/// responses out, so a change to what the server says shows up as a diff
/// rather than as an editor behaving differently.
#[test]
fn language_server() {
    run_corpus(&tests_dir().join("repositories/lsp"), "lsp", 95);
}

/// Every method a 3.17 client can send is answered by the dispatch, and is
/// sent by at least one recorded session.
///
/// The LSP page used to carry a table of every request with the claim that
/// "there is no third column of things left for later", and nothing held it to
/// that until this test: the claim was prose, the enumeration was prose, and
/// `$/progress` sat with neither an arm nor a golden while the table read
/// complete. The list is
/// `language_server::CLIENT_TO_SERVER`, beside the dispatch it describes.
///
/// The answer has to come from a **running server**. The first version of this
/// test looked the names up in the text of `cli/src/language_server/` — the
/// directory `CLIENT_TO_SERVER` is itself written in — so the list was its own
/// witness: the `$/progress` arm could be deleted with every name still found.
///
/// So each name is asked of one `buri lsp` over a small repository:
///
///  * **a request is sent and answered**, and anything but `-32601` is an arm.
///    A `-32601` counts only where a golden records that refusal naming the
///    method, which is how `documentLink/resolve` and `workspaceSymbol/resolve`
///    are decisions written down rather than omissions;
///  * **a notification is sent** and the request behind it still answers, and
///    its name has to appear in a *pattern* of `dispatch`'s match. That last
///    half is the one thing a running server cannot show: a notification the
///    server handled and one that fell through the catch-all both say nothing;
///  * **a recorded session sends it**, so the answer is a thing that ran rather
///    than a branch nobody has taken.
#[test]
fn the_protocol_surface_is_covered() {
    let surface = buri::language_server::CLIENT_TO_SERVER;
    let (sessions, refusals, cases) = recorded_sessions();
    assert!(cases > 50, "found {cases} lsp cases; the walk is broken");
    let source = std::fs::read_to_string(repo_root().join("cli/src/language_server/mod.rs"))
        .expect("the dispatch's own module");
    let arms = dispatch_arms(&source);
    assert!(arms.len() > 40, "{} arms read out of the dispatch; the scan is broken", arms.len());
    for method in NOTIFICATIONS {
        assert!(
            surface.contains(method),
            "`{method}` is not a method a client sends; the notification list is stale"
        );
    }
    let (notifications, requests): (Vec<&str>, Vec<&str>) =
        surface.iter().copied().partition(|m| NOTIFICATIONS.contains(m));

    let scratch = Scratch::repo("lsp-surface");
    scratch.binary_package("cmd/app", SURFACE_PROGRAM);
    let mut editor = Editor::open(&scratch.root);
    let params = surface_params(&editor.uri("cmd/app/main.buri"), SURFACE_PROGRAM);
    let mut missing = Vec::new();

    // The notifications first, `exit` excepted: that one ends the session. One
    // of them closes the buffer the requests below need, so the file is opened
    // again once they have all been sent.
    for &method in notifications.iter().filter(|m| **m != "exit") {
        editor.notify(method, &params);
        let alive = editor.ask("textDocument/linkedEditingRange", &params);
        assert!(
            alive.contains(r#""result""#),
            "the server stopped answering after the `{method}` notification: {alive}"
        );
        if !arms.iter().any(|arm| arm == method) {
            missing.push(format!("  {method}: no arm in the dispatch's match"));
        }
    }
    editor.notify("textDocument/didOpen", &params);

    // Then every request, `shutdown` last for the reason `exit` is not here.
    for &method in requests.iter().filter(|m| **m != "shutdown") {
        missing.extend(unanswered(&mut editor, method, &params, &refusals));
    }
    missing.extend(unanswered(&mut editor, "shutdown", &params, &refusals));
    if !arms.iter().any(|arm| arm == "exit") {
        missing.push("  exit: no arm in the dispatch's match".to_string());
    }
    editor.notify("exit", &params);
    let stopped = editor.process.wait().expect("the language server did not stop");
    assert!(stopped.success(), "the server left the whole surface badly: {stopped}");

    for method in surface {
        let sent = sessions.contains(&format!("{q}method{q}: {q}{method}{q}", q = '"'))
            || sessions.contains(&format!("{q}method{q}:{q}{method}{q}", q = '"'));
        if !sent {
            missing.push(format!("  {method}: no recorded session sends it"));
        }
    }
    assert!(
        missing.is_empty(),
        "{} of {} client methods are not covered:\n{}\n\nEither answer them in the \
         dispatch and record a session under the lsp corpus, or take them out of \
         `language_server::CLIENT_TO_SERVER` with the reason — but do not leave the \
         `lsp` reference page claiming a surface no case exercises.",
        missing.len(),
        surface.len(),
        missing.join("\n")
    );
    eprintln!("protocol surface: {} client methods, all dispatched and all sent", surface.len());
}

/// One request put to a live server, and what is wrong with the answer.
///
/// `-32601` is the catch-all, so it is a hole unless a golden session records
/// that refusal by name — which is what makes a refusal a decision.
fn unanswered(editor: &mut Editor, method: &str, params: &str, refusals: &str) -> Option<String> {
    let answer = editor.ask(method, params);
    let refused = answer.contains(r#""code":-32601"#);
    let recorded = refusals.contains(&format!("`{method}` is not implemented"));
    (refused && !recorded)
        .then(|| format!("  {method}: answered -32601, and no golden records that refusal"))
}

/// The methods a client sends as notifications rather than as requests.
///
/// Written out because nothing in a name says which it is —
/// `textDocument/willSave` is a notification and `willSaveWaitUntil` is a
/// request — and the test holds every entry to being a method
/// `CLIENT_TO_SERVER` lists.
const NOTIFICATIONS: &[&str] = &[
    "initialized",
    "exit",
    "$/cancelRequest",
    "$/progress",
    "$/setTrace",
    "workspace/didChangeWorkspaceFolders",
    "workspace/didChangeConfiguration",
    "workspace/didChangeWatchedFiles",
    "workspace/didCreateFiles",
    "workspace/didRenameFiles",
    "workspace/didDeleteFiles",
    "textDocument/didOpen",
    "textDocument/didChange",
    "textDocument/willSave",
    "textDocument/didSave",
    "textDocument/didClose",
    "notebookDocument/didOpen",
    "notebookDocument/didChange",
    "notebookDocument/didSave",
    "notebookDocument/didClose",
    "window/workDoneProgress/cancel",
];

/// The program the surface is driven against: one file, one import and one
/// call, so that a position request has something under it.
const SURFACE_PROGRAM: &str = r#"from "core/effect" import { Allocator, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

fn answer(): Int { 41 }

export fn main(): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "answer=${answer()}").ignore();
  .Ok(())
}
"#;

/// One params object carrying every field any client method reads, so that a
/// single envelope drives the whole surface: a handler that wants a field finds
/// it, and one that does not ignores it.
///
/// The values are minimal rather than meaningful — position `0:0`, an empty
/// query, a command no server implements. What is being asked is whether the
/// method reaches an arm, and an arm answers a request it cannot serve with
/// something other than `-32601`.
fn surface_params(uri: &str, text: &str) -> String {
    let at = r#"{"line":0,"character":0}"#;
    let range = format!(r#"{{"start":{at},"end":{at}}}"#);
    // What a hierarchy item and a resolvable item both carry: the round trip
    // is through `data`, so that is the whole of either.
    let data = format!(r#"{{"uri":"{uri}","position":{at}}}"#);
    let fields = [
        format!(
            r#""textDocument":{{"uri":"{uri}","languageId":"buri","version":2,"text":{}}}"#,
            quoted(text)
        ),
        format!(r#""position":{at},"positions":[{at}],"range":{range}"#),
        r#""context":{"diagnostics":[],"includeDeclaration":true}"#.to_string(),
        format!(r#""contentChanges":[{{"text":{}}}]"#, quoted(text)),
        format!(r#""item":{{"data":{data}}},"data":{data}"#),
        format!(r#""files":[{{"uri":"{uri}","oldUri":"{uri}","newUri":"{uri}"}}]"#),
        r#""event":{"added":[],"removed":[]},"changes":[]"#.to_string(),
        r#""query":"","command":"buri.notACommand","arguments":[]"#.to_string(),
        r#""newName":"renamed","previousResultIds":[],"settings":{}"#.to_string(),
        // `off` so that the `$/setTrace` does not fill the rest of the session
        // with `$/logTrace`, and an id this client never sends for the cancel.
        r#""value":"off","token":"buri/surface","id":0"#.to_string(),
    ];
    format!("{{{}}}", fields.join(","))
}

/// The method names in the *pattern* position of `dispatch`'s match — the arms
/// the server has, rather than the strings its sources mention.
///
/// A scan rather than a parse: `//` comments and string bodies are stepped
/// over, brackets are counted, and a name counts when it is read before its
/// arm's `=>` at the match's own depth.
fn dispatch_arms(source: &str) -> Vec<String> {
    const OPENS: &str = "match (method, id) {";
    let start = source.find(OPENS).expect("dispatch's match on the method") + OPENS.len();
    let body: Vec<char> = source[start..].chars().collect();
    let mut arms = Vec::new();
    let mut depth = 0i32;
    let mut in_pattern = true;
    let mut i = 0;
    while i < body.len() {
        match body[i] {
            '/' if body.get(i + 1) == Some(&'/') => {
                while i < body.len() && body[i] != '\n' {
                    i += 1;
                }
            }
            '"' => {
                let mut literal = String::new();
                i += 1;
                while i < body.len() && body[i] != '"' {
                    if body[i] == '\\' {
                        i += 1;
                    }
                    if let Some(c) = body.get(i) {
                        literal.push(*c);
                    }
                    i += 1;
                }
                if in_pattern {
                    arms.push(literal);
                }
            }
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                // Below the match's own depth is the brace that closes it.
                if depth < 0 {
                    break;
                }
                if depth == 0 {
                    in_pattern = true;
                }
            }
            '=' if depth == 0 && body.get(i + 1) == Some(&'>') => {
                in_pattern = false;
                i += 1;
            }
            ',' if depth == 0 => in_pattern = true,
            _ => {}
        }
        i += 1;
    }
    arms
}

/// Every recorded lsp session's text, every golden's text, and the number of
/// cases the two were read from.
fn recorded_sessions() -> (String, String, usize) {
    let dir = tests_dir().join("repositories/lsp");
    let mut sessions = String::new();
    let mut refusals = String::new();
    let mut cases = 0;
    for entry in std::fs::read_dir(&dir).expect("the lsp corpus").filter_map(Result::ok) {
        cases += 1;
        // A case may record several sessions — `session.jsonl` and the
        // `session_*.jsonl` beside it, each a different client.
        let files = std::fs::read_dir(entry.path()).into_iter().flatten().filter_map(Result::ok);
        for file in files {
            let path = file.path();
            let named = path.file_name().is_some_and(|n| {
                let n = n.to_string_lossy();
                n.starts_with("session") && n.ends_with(".jsonl")
            });
            if named {
                sessions.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
            }
        }
        if let Ok(text) = std::fs::read_to_string(entry.path().join("expected/session.txt")) {
            refusals.push_str(&text);
        }
    }
    (sessions, refusals, cases)
}

// ---------------------------------------------------------------------------
// The language server's budget
// ---------------------------------------------------------------------------

/// What an editor request must answer inside.
///
/// Fifty milliseconds is the number a keystroke can hide behind: below it the
/// squiggle arrives with the character that caused it, above it the editor is
/// visibly waiting. The corpus above pins the *work* each request does, which
/// is what the speed is made of and is the same number on every machine; this
/// is the same claim in the unit a reader cares about.
const LANGUAGE_SERVER_BUDGET: std::time::Duration = std::time::Duration::from_millis(50);

/// The bar this run holds a request to: the 50 ms above, widened by
/// `BURI_PERF_BUDGET_SCALE` on a machine slower than the one the number was
/// taken on. A developer's machine sets nothing; CI sets what it measured.
fn language_server_budget() -> std::time::Duration {
    let scale = std::env::var("BURI_PERF_BUDGET_SCALE")
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|scale| (1.0..=100.0).contains(scale))
        .unwrap_or(1.0);
    LANGUAGE_SERVER_BUDGET.mul_f64(scale)
}

/// One session's timings, measured again while any of them is over the bar,
/// every request kept at the fastest time it was seen in.
///
/// A budget test is a claim about what a request *costs*, and a wall clock on
/// a shared runner sometimes answers a different question: a request that lost
/// the core between its send and its answer reads as ten times its own work
/// while nothing about the server changed. That is not a number to fail on,
/// and it is not a reason to widen the bar either — a bar moved to fit the
/// unluckiest timeslice stops having an opinion about the work. So one run is
/// not the verdict. A run holding a request over the bar is taken again, from
/// a fresh server against a fresh copy of the repository, and each request is
/// then held to the best of its attempts.
///
/// Which leaves the 50 ms exactly where it was. A request that got slower is
/// slower in every attempt and still fails; a preempted one is not, and the
/// extra session is paid for only by the runs that would otherwise have been a
/// red X nobody could reproduce. It is the *measurement* that repeats and not
/// the assertion: the bar is applied once, to the best readings.
///
/// The fastest attempt rather than the middle one because the distribution is
/// one-sided — a machine can only make a run slower — so the shortest reading
/// is the least noisy reading of the same quantity, which is the view
/// `design/PERFORMANCE.md` §2's protocol takes of the benchmark suite's
/// samples, where the fastest of them is reported beside the median for
/// exactly that reason.
fn best_of(
    budget: std::time::Duration,
    mut session: impl FnMut() -> Vec<(String, std::time::Duration)>,
) -> Vec<(String, std::time::Duration)> {
    /// How many sessions one run may spend. Three: the first, and two more for
    /// an unlucky request to fail to reproduce itself in. A request descheduled
    /// in all three of them is not what being descheduled looks like.
    const ATTEMPTS: u32 = 3;

    let mut best = session();
    for attempt in 2..=ATTEMPTS {
        let over = best.iter().filter(|(_, took)| *took > budget).count();
        if over == 0 {
            break;
        }
        // In the log on the way past, because a run that needed a second
        // session is a run whose runner was busy, and that is worth seeing
        // beside a pass.
        eprintln!(
            "language server: {over} request(s) over the {}ms bar, so the session is measured again (attempt {attempt} of {ATTEMPTS})",
            budget.as_millis()
        );
        let again = session();
        assert_eq!(
            again.len(),
            best.len(),
            "the session measured a different number of requests the second time, so its attempts cannot be compared"
        );
        for (kept, (what, took)) in best.iter_mut().zip(again) {
            assert_eq!(
                kept.0, what,
                "the session made its requests in a different order the second time, so its attempts cannot be compared"
            );
            kept.1 = kept.1.min(took);
        }
    }
    best
}

/// Every request an editor makes around a keystroke, against the worked
/// monorepo, timed.
///
/// Off unless `BURI_PERF` is set, and meaningless without `--release`: a debug
/// build is an order slower, so an assertion about milliseconds in one would
/// fail on the runner rather than on the change.
///
/// ```text
/// BURI_PERF=1 cargo test --release -p buri --test build repositories::language_server_speed
/// ```
///
/// The session has three parts. First the restore: an editor coming back to a
/// project opens every tab it had, so every `.buri` file in the repository is
/// opened one after another and **each open is held to the budget** — what an
/// open costs is the target it opened, and the buffers already open are a hash
/// each. Then the cold `workspace/diagnostic`, which is **not** measured: a
/// cold sweep is one compilation per target and it is paid at startup rather
/// than under a person's hands. Then the interactive loop — a keystroke, then
/// the pulls and the position requests an editor sends after one — every
/// request of which is held to the budget, with all those buffers still open.
///
/// The whole session is what `best_of` above measures, so a request that comes
/// back over the bar is timed again in a fresh one rather than failed on a
/// single reading.
#[test]
fn language_server_speed() {
    if std::env::var("BURI_PERF").is_err() {
        crate::harness::ci::deferred_to(
            "language server budget",
            "language server budget (arm64)",
            "BURI_PERF is unset, and a millisecond in a debug profile is a fact about the \
             profile",
        );
        return;
    }
    let budget = language_server_budget();
    let mut timings = best_of(budget, || {
        let scratch = Scratch::copy_of("lsp-speed", &example_repo());
        let mut editor = Editor::open(&scratch.root);

        let mut timings = Vec::new();
        for file in sources_of(&scratch.root) {
            timings.push(editor.opened(&file));
        }
        let watched = ["lib/money/cents.buri", "lib/store/codec.buri", "cmd/server/routes.buri"];
        editor.timed("workspace/diagnostic", r#"{"previousResultIds":[]}"#);

        for round in 1..=3u32 {
            editor.typed("lib/money/cents.buri", round);
            for file in watched {
                timings.push(editor.pull(file));
            }
            timings.push(editor.timed("workspace/diagnostic", r#"{"previousResultIds":[]}"#));
            for method in [
                "textDocument/documentHighlight",
                "textDocument/documentLink",
                "textDocument/documentColor",
                "textDocument/codeAction",
            ] {
                timings.push(editor.at(method, "lib/money/cents.buri"));
            }
        }
        editor.close();
        timings
    });

    timings.sort_by_key(|(_, took)| std::cmp::Reverse(*took));
    let over: Vec<_> = timings.iter().filter(|(_, took)| *took > budget).cloned().collect();
    let five = listed(&timings[..timings.len().min(5)]);
    // Printed on a pass too: a run that stayed under the bar is the record a
    // later recalibration reads.
    eprintln!(
        "language server: {} requests under a {}ms bar; the five slowest:\n{five}",
        timings.len(),
        budget.as_millis()
    );
    assert!(
        over.is_empty(),
        "these answers took longer than the {}ms an editor request has:\n{}\n\nthe five slowest of the run:\n{five}",
        budget.as_millis(),
        listed(&over),
    );
}

/// How much slower the second half of a restore may be than the first.
///
/// The budget above is a constant and this is the *shape*: an open that pays
/// for the buffers already open fails this on any machine, because both halves
/// are measured on the same one. Three times, plus a floor so that a run whose
/// opens are all a millisecond does not fail on jitter.
const RESTORE_DRIFT: u32 = 3;

/// The same restore, against a repository the size a person opens a hundred
/// tabs in.
///
/// `cli/tests/example` is 2.3k lines in eight targets: enough to hold the
/// budget honest and not enough to show what an open costs when the buffer
/// count is what an editor really restores. This one is generated from a
/// template so its size is stated rather than measured — twenty-four libraries
/// of four modules of eighty-six functions, 24,768 lines across 145 files — and
/// every one of those files is opened, under the same 50 ms an editor request
/// has. Before the findings of a target were kept per target the last open here
/// was 58 ms and the first was 4 ms; both halves of that are what fails now.
///
/// The restore is measured through `best_of` like the session above is, which
/// is what the two medians are taken over as well: the shape of a restore is a
/// claim about opens, not about which of them the runner happened to interrupt.
///
/// ```text
/// BURI_PERF=1 cargo test --release -p buri --test build repositories::language_server_open_cost
/// ```
#[test]
fn language_server_open_cost() {
    if std::env::var("BURI_PERF").is_err() {
        crate::harness::ci::deferred_to(
            "language server budget",
            "language server budget (arm64)",
            "BURI_PERF is unset, and a millisecond in a debug profile is a fact about the \
             profile",
        );
        return;
    }
    let budget = language_server_budget();
    let timings = best_of(budget, || {
        let scratch = generated_repository("lsp-open-scale", 24, 4, 86);
        let mut editor = Editor::open(&scratch.root);
        let mut timings = Vec::new();
        for file in sources_of(&scratch.root) {
            timings.push(editor.opened(&file));
        }
        editor.close();
        timings
    });

    let over: Vec<_> = timings.iter().filter(|(_, took)| *took > budget).cloned().collect();
    let mut slowest = timings.clone();
    slowest.sort_by_key(|(_, took)| std::cmp::Reverse(*took));
    let five = listed(&slowest[..slowest.len().min(5)]);
    let half = timings.len() / 2;
    let first = median(&timings[..half]);
    let last = median(&timings[half..]);
    // Printed on a pass too: the two medians are the shape, and the five
    // slowest are what a later recalibration reads.
    eprintln!(
        "language server: {} opens under a {}ms bar, {}ms then {}ms per open; the five slowest:\n{five}",
        timings.len(),
        budget.as_millis(),
        first.as_millis(),
        last.as_millis()
    );
    assert!(
        over.is_empty(),
        "these opens took longer than the {}ms an open has:\n{}\n\nthe five slowest of the run:\n{five}",
        budget.as_millis(),
        listed(&over),
    );
    let allowed =
        first.saturating_mul(RESTORE_DRIFT).saturating_add(std::time::Duration::from_millis(10));
    assert!(
        last <= allowed,
        "the restore's second half costs {}ms an open against its first half's {}ms, \
         so an open is still paying for the buffers already open:\n{}",
        last.as_millis(),
        first.as_millis(),
        five
    );
}

/// The middle time of a run, which is what a ratio between two halves of a
/// restore has to be taken over: one slow open would carry a mean.
fn median(rows: &[(String, std::time::Duration)]) -> std::time::Duration {
    let mut times: Vec<std::time::Duration> = rows.iter().map(|(_, took)| *took).collect();
    times.sort();
    times.get(times.len() / 2).copied().unwrap_or_default()
}

/// One timing per line, for the message a failure prints.
fn listed(rows: &[(String, std::time::Duration)]) -> String {
    rows.iter()
        .map(|(what, took)| format!("  {what}: {}ms", took.as_millis()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `.buri` file in a repository, relative to its root and sorted — which
/// is every tab an editor could have had open in it.
fn sources_of(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // What a build wrote is not a tab anyone had open.
                if !matches!(path.file_name().and_then(|n| n.to_str()), Some(".buri" | "out")) {
                    stack.push(path);
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some("buri") {
                if let Ok(rel) = path.strip_prefix(root) {
                    found.push(rel.display().to_string().replace('\\', "/"));
                }
            }
        }
    }
    found.sort();
    found
}

/// A repository of a stated size, written from a template.
///
/// `packages` libraries of `modules` modules each, every module `functions`
/// functions that compile and depend on nothing — the size is the point, and a
/// dependency between two of them would only put a second target's compilation
/// inside the first's open.
fn generated_repository(
    name: &str,
    packages: usize,
    modules: usize,
    functions: usize,
) -> Scratch {
    let scratch = Scratch::empty(name);
    scratch.write(
        "REPO.buri",
        "# Generated by repositories.rs for the language server's open budget.\n",
    );
    for package in 0..packages {
        let dir = format!("lib/p{package}");
        let sources: Vec<String> = (0..modules).map(|m| format!("\"m{m}.buri\"")).collect();
        scratch.write(
            &format!("{dir}/BUILD.buri"),
            &format!(
                "library {{\n    sources: [{}]\n    visibility: [\"//visibility:public\"]\n}}\n",
                sources.join(", ")
            ),
        );
        let mut exports = String::new();
        for module in 0..modules {
            let mut body = String::new();
            for f in 0..functions {
                body.push_str(&format!(
                    "/// Adds {f}, in whole units.\n\
                     export fn p{package}m{module}f{f}(n: I64): I64 {{ n + {f} }}\n\n"
                ));
            }
            scratch.write(&format!("{dir}/m{module}.buri"), &body);
            // Every name re-exported: a library's surface is `lib.buri`, and a
            // function nothing reaches is a `dead-code` finding rather than the
            // clean repository this is meant to time an open against.
            let names: Vec<String> =
                (0..functions).map(|f| format!("p{package}m{module}f{f}")).collect();
            exports.push_str(&format!(
                "from \"//lib/p{package}/m{module}.buri\" export {{ {} }};\n",
                names.join(", ")
            ));
        }
        scratch.write(&format!("{dir}/lib.buri"), &exports);
    }
    scratch
}

/// A client for the timed session: one `buri lsp` on a pipe, and the framing
/// around it.
///
/// Deliberately not the recorded-session harness. That one writes every
/// message and reads the answers afterwards, which is the right shape for a
/// golden and the wrong one for a clock: what is being measured here is the
/// time between one request going out and its own answer coming back.
struct Editor {
    process: std::process::Child,
    dir: PathBuf,
    root: String,
    next_id: u64,
}

impl Editor {
    fn open(root: &Path) -> Editor {
        let process = buri_command()
            .arg("lsp")
            .current_dir(root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the language server did not start");
        let mut editor = Editor {
            process,
            dir: root.to_path_buf(),
            root: format!("file://{}", root.display()),
            next_id: 0,
        };
        let params = format!(r#"{{"rootUri":"{}"}}"#, editor.root);
        editor.timed("initialize", &params);
        editor.notify("initialized", "{}");
        editor
    }

    /// The uri of one file in the repository.
    fn uri(&self, rel: &str) -> String {
        format!("{}/{}", self.root, rel)
    }

    /// One buffer opened, and what the open cost.
    ///
    /// A notification has no answer to time, so the clock is stopped by a
    /// `linkedEditingRange` behind it — a request this server answers `null`
    /// without reading anything, so what the pair measures is the open.
    fn opened(&mut self, rel: &str) -> (String, std::time::Duration) {
        let text = std::fs::read_to_string(self.dir.join(rel)).unwrap();
        let params = format!(
            r#"{{"textDocument":{{"uri":"{}","languageId":"buri","version":1,"text":{}}}}}"#,
            self.uri(rel),
            quoted(&text)
        );
        let started = std::time::Instant::now();
        self.notify("textDocument/didOpen", &params);
        self.at("textDocument/linkedEditingRange", rel);
        (format!("didOpen {rel}"), started.elapsed())
    }

    /// A keystroke: a comment appended, so that nothing above it moves and the
    /// file still compiles.
    fn typed(&mut self, rel: &str, round: u32) {
        let mut text = std::fs::read_to_string(self.dir.join(rel)).unwrap();
        text.push_str(&format!("\n// A keystroke, number {round}.\n"));
        let params = format!(
            r#"{{"textDocument":{{"uri":"{}","version":{}}},"contentChanges":[{{"text":{}}}]}}"#,
            self.uri(rel),
            round.saturating_add(1),
            quoted(&text)
        );
        self.notify("textDocument/didChange", &params);
    }

    fn pull(&mut self, rel: &str) -> (String, std::time::Duration) {
        let params = format!(r#"{{"textDocument":{{"uri":"{}"}}}}"#, self.uri(rel));
        self.timed("textDocument/diagnostic", &params)
    }

    /// One of the requests an editor sends for the cursor's position.
    fn at(&mut self, method: &str, rel: &str) -> (String, std::time::Duration) {
        let params = format!(
            r#"{{"textDocument":{{"uri":"{}"}},"position":{{"line":0,"character":0}},"range":{{"start":{{"line":0,"character":0}},"end":{{"line":0,"character":0}}}},"context":{{"diagnostics":[]}}}}"#,
            self.uri(rel)
        );
        self.timed(method, &params)
    }

    /// Sends one request and waits for its own answer, which is what the clock
    /// is on. Notifications the server sends meanwhile are read and dropped.
    fn timed(&mut self, method: &str, params: &str) -> (String, std::time::Duration) {
        let started = std::time::Instant::now();
        self.ask(method, params);
        (method.to_string(), started.elapsed())
    }

    /// Sends one request and returns its own answer, error replies included.
    /// Notifications the server sends meanwhile are read and dropped.
    fn ask(&mut self, method: &str, params: &str) -> String {
        self.next_id = self.next_id.saturating_add(1);
        let id = self.next_id;
        self.write(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params}}}"#
        ));
        // The keys are sorted, so `"jsonrpc"` follows the id in every answer —
        // behind `"error"` in a refusal, which is why this is not a prefix.
        // The ids of the requests the *server* sends are strings, so a number
        // here can only be this client's.
        let wanted = format!(r#""id":{id},"jsonrpc""#);
        loop {
            let message = self.read();
            if message.contains(&wanted) {
                return message;
            }
        }
    }

    fn notify(&mut self, method: &str, params: &str) {
        self.write(&format!(r#"{{"jsonrpc":"2.0","method":"{method}","params":{params}}}"#));
    }

    fn close(&mut self) {
        self.timed("shutdown", "null");
        self.notify("exit", "null");
        let _ = self.process.wait();
    }

    fn write(&mut self, body: &str) {
        use std::io::Write;
        let stdin = self.process.stdin.as_mut().expect("the server's stdin is a pipe");
        write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        stdin.flush().unwrap();
    }

    /// One framed message, read a byte at a time through the headers so the
    /// body that follows them is not swallowed.
    fn read(&mut self) -> String {
        use std::io::Read;
        let stdout = self.process.stdout.as_mut().expect("the server's stdout is a pipe");
        let mut headers = String::new();
        while !headers.ends_with("\r\n\r\n") {
            let mut byte = [0u8; 1];
            assert_eq!(stdout.read(&mut byte).unwrap(), 1, "the server closed the stream");
            headers.push(byte[0] as char);
        }
        let length: usize = headers
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .expect("a message with no Content-Length")
            .trim()
            .parse()
            .unwrap();
        let mut body = vec![0u8; length];
        stdout.read_exact(&mut body).unwrap();
        String::from_utf8(body).unwrap()
    }
}

/// One JSON string literal, which is all the escaping a source file needs.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(2));
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
