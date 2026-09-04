# Enable creating true websites

Routing, UI libraries, SPA/SSR/lazy loading, ideally also resumable

## Where this stands, verified

A `WEB` build writes exactly three files: `main.mjs`, `main.css` when the program
has static styles, and a hardcoded `index.html` shell — a `format!` in
`build/actions.rs:1713-1728` whose only variable is the artifact's basename.
`<title>` is that basename; there is no representation for meta, open-graph, or a
per-route document anywhere in the language or the build file. The vocabulary is
`ui/node`'s seventeen constructors over a closed, opaque `Node<C>` with **no
tag escape hatch** (`ui_node.buri:122`), and `mount(ctx, root, themes)` is the
one entry, always into `document.body`, always building fresh
(`runtime.js:3681`).

There is **no routing of any kind**: nothing in `ui/*` names a URL, a history
entry, a location or a popstate, and `ui.link(dest, children)`
(`ui_node.buri:229`) lowers to a bare `<a href>` with no listener
(`runtime.js:3461`), so a click navigates the document away.
`design/ui-reactivity.md` never mentions SSR, hydration, resumability or
routing — they are not open items there, they were never scoped. There is no
`buri serve` and no `build --watch` (`commands/mod.rs:341-357`); the documented
workflow is "serve the directory". The flagship WEB app,
`cli/tests/example/cmd/basket/`, is one page with no navigation in it.

**Proposed approach.** Four increments, each shipping on its own.

1. **Routing, as one small effect plus an ordinary library.** The view switch is
   already expressible — `ui.computed(fn(scope) => …)` reading a route signal
   (`ui_node.buri:299`), with `each` reconciling by key. What is missing is the
   browser's location, and it cannot be added in user space because `Node` is
   closed. So: a WEB-only `effect Location` in `ui/effect` with
   `locationPath(self): Str` and `locationGo(self, path: Str): ()`, plus a
   host-driven popstate hook — the same inbound-push shape as the client half's
   item 1 below. Then `ui/route` is ordinary Buri: the app declares its own
   `Route` enum, writes `parse: fn(Str) => Route`, holds it in a signal, and
   `match`es exhaustively. Exhaustiveness over the app's own enum is the
   type-safe routing story and it costs no language feature.
2. **UI libraries are blocked only by external repositories.**
   `ui-reactivity.md:611` already says `Signal`, `Prop`, `Node`, `Style` and
   every constructor are ordinary Buri, movable to a real library the day
   external repos land, and `cli/tests/example/lib/kit/` is a working component
   library today. Track this under external repos, not here.
3. **SSR as a render-only grant, on the `EMAIL` pattern.** The design already
   has the shape: a platform whose host exports rendering and nothing
   interactive, where `Const` and `Computed` props resolve and `Cell` has
   nothing behind it (`ui-reactivity.md:537`). A grant row over the same
   renderer gives `render(ctx, root, themes): Str` that a `core/net/server`
   handler answers with, and the extracted stylesheet is already a build
   artifact rather than something the module injects. Hydration is then a
   renderer change — adopt existing nodes instead of creating them — not a
   language one. Ship render first: server HTML plus a client bundle that
   replaces the tree is most of the perceived win and is testable in
   `ui/testing`.
4. **Lazy loading: defer, and say why.** It is a backend feature, not a `ui/*`
   one — there is one `Program`, monomorphized, DCE'd against a root set and
   minified whole (`middle/dce.rs:42`, `backend/js/mod.rs:91`). A second module
   means a second root set, a cross-module symbol contract and a loader.
   Measure a real page first.

**Challenges / risks.**

- **A reactive closure may not wait, and that is load-bearing.** `rc.rs:1519`
  names the exact values handed to JavaScript that cannot await: "a `view` given
  to `mount`, the row callbacks inside `ui.each`, the callback of
  `$list_mapCtx`, a sort comparator". `$ui_run` stores `n.compute([id])`
  synchronously (`runtime.js:2497`), and the only thing keeping a program from
  breaking that is a *type*: a computed thunk is `fn(Scope) => T`, `Scope`
  implements only `Watch`, and `Watch.read` is not on `rc::suspends`' list. So
  there is no Suspense in this model and there should not be — a component
  cannot fetch its own data; data is loaded before the tree is built. That is
  the "explicit over clever" answer and it is worth committing to in writing
  before SSR work starts, not discovering during it.
- **`Event` is opaque with zero accessors** (`ui_effect.buri:97`) — no target, no
  modifier keys, no `preventDefault`. A router must intercept clicks on links,
  and "cmd-click opens a new tab" is a modifier-key question. Routing therefore
  forces the first widening of `Event`, and that shape is a permanent surface.
- **Resumability is a whole-backend commitment; recommend not.** Resuming means
  serializing the reactive graph *and* its closures, and a Buri closure is an
  arrow printed where it stands with no name and no module (`generate.rs:2104`).
  Qwik pays with per-closure chunks and a symbol registry — code-splitting at
  every handler — which forecloses the single-artifact model, whole-program
  minification, and the DCE that makes the page small. SSR plus hydration
  reaches the same first-paint number without touching the backend.
- **Nothing drives a real browser.** `ui-reactivity.md:672` and
  `ui_testing.buri:66-86` both flag it, and a repo-wide search for
  playwright/puppeteer/webdriver finds nothing. Routing is precisely a
  browser-behaviour feature — back button, scroll restoration, focus after
  navigation — so a router landing without that suite is a feature whose
  failures are invisible to CI. This is the deterministic-tests bar and the one
  this item is most likely to fail.

**Possibly stale.** The line itself is a wish, not a claim, so nothing in it is
stale — but two premises behind it have moved. A page now grants `Net`
(`standard_library/mod.rs:439`), so a route change can load data, and an event
handler may suspend (`runtime.js:2584`), so a navigation can await. The SPA half
is closer than the line implies. Separately, `ui-reactivity.md`'s own tables are
stale in two places worth a pass: the module table (L604-614) still lists a
`Fetch` effect that was deleted, and the "Open" list still says `Sockets` is
granted but unreachable and that `serve` runs one handler at a time — both
shipped.

# Unblock buri client library

- No WebSocket client. core/net/server only accepts sockets, and only on Linux/macOS. Net is one-shot HTTP fetch. So the client's whole connection layer has no transport: connect/resume, reconnect, and above all server-pushed subscription updates, which need an inbound message to arrive without the program asking.
- No callbacks, timers, or background work. Every effect is call-and-answer. Clock.sleep blocks, and Tasks is not granted on WEB. Reconnect backoff, flushing the offline write queue when the socket returns, and reacting to a pushed frame all need "run this later or when something arrives", and nothing in the platform can express it.
- No library output or JS interop. buri build emits one self-contained ES module whose only entry is main, with no export statements, and there is no foreign-function mechanism. A Buri client could not be consumed from TypeScript or React (createClient, useQuery), and could not call browser APIs or npm packages. It would have to be a whole Buri page built with ui/*, not a library other apps import.
- No schema-derived typing. About 6,600 of the client's 10,300 lines are mapped and conditional types that derive document shapes from a schema value and are checked by type tests. Buri has no type-level computation, no associated types, and no anonymous records. A Buri client would expose triple- and Value-shaped APIs, or need per-schema code generation, which today exists only for .proto.

## 1. Transport — a client socket

**Proposed approach.** The server half is fully built and it is the template.
`Listen.listenUpgrade` mints a socket and `listenReceive` waits on it
(`effect.buri:530`, `:535`); `effect Sockets` pushes on one (`:583`); and
`core/net/server` wraps it in `Socket`, `Message`, `CloseReason` and a
`WebSocket<C, S>` of three hooks threading per-socket state
(`server.buri:899-915`). Every one of those types is direction-neutral already.

So: **one new effect, `Connect`, and one new module, `core/net/socket`.**
`Connect` is `Listen`'s mirror and nothing more — `connectOpen(self, url: Str,
plan: [Serve]): Result<Int, ConnectError>` and `connectReceive(self, socket:
Int): Result<Received, ConnectError>`. Writing stays `Sockets`, unchanged, which
is the whole point of `Sockets` having been split out: a socket somebody else
accepted and a socket this program opened are pushed on by the same authority.
`core/net/socket` then exports `connect(ctx, Client { url, onOpen, onMessage,
onClose })` with `WebSocket<C, S>`'s exact hook shape and reuses `Message`,
`CloseReason` and `Socket` verbatim.

Grant it on `LINUX, MACOS, JS, WEB` — all four, and this is the row that differs
from `Listen`'s. A page is served rather than serving, but a page most certainly
*connects*, and the withholding reason ("its host has no way to accept a
connection") simply does not apply. Increment: land `LINUX, MACOS` first against
the acceptor's own `tungstenite`, which already speaks RFC 6455, then `JS`/`WEB`
over the platform `WebSocket`.

**Challenges / risks.**

- **Something has to drive the receive loop, and on WEB nothing can.** Natively,
  `core/net/socket` writes the same tail-recursive pump `core/net/server` does,
  fanned out with `Tasks`. On WEB there is no `Tasks`, so the hooks must be
  *installed* and called by the host — which is exactly what `onPress` and
  `onSubmit` already are (`runtime.js:3453`, `:3505`). Two platforms, one hook
  contract, two mechanisms. Write the hook struct as the contract and the loop
  as an implementation detail, the way `Listen` deliberately carries no handler.
- **A hook that suspends is fine; a hook the graph calls is not.** `$ui_flush`
  already tolerates a returned promise and rethrows its rejection from a fresh
  task (`runtime.js:2596`). But a socket hook must never be invoked from inside
  `$ui_run`, and nothing structurally prevents a program arranging that. Make it
  a written rule: socket hooks are host-called, never graph-called.
- **`Received` crosses the boundary as a struct with a `Frame` tag whose
  declaration order is ABI** (`effect.buri:833`). A client sharing it pins that
  shape for a second runtime table — cheap now, permanent later.
- **What it forecloses:** granting `Connect` on WEB gives a page a long-lived
  outbound connection. That is a real authority increase and the grant table's
  `because` column has to say so honestly.

**Possibly stale.** "core/net/server only accepts sockets" is CURRENT and now
*fully built* rather than aspirational — `ui-reactivity.md:658` still says
"`Sockets` is granted but unreachable; nothing performs a WebSocket upgrade",
false since F7. "Net is one-shot HTTP fetch" is CURRENT, but `Net` is now granted
on **every** platform including WEB (`standard_library/mod.rs:439`) because
`fetch` stopped blocking; the callback-shaped `ui/effect::Fetch` that stood in
for it is gone.

## 2. Callbacks, timers, background work

**Proposed approach.** Most of this is already here and the line undersells the
platform. What is genuinely missing is one thing — a detached start on WEB — and
the recommendation is to not add one.

- *Timers:* `time.sleep(ctx, d)` calls `Clock.sleepMillis`, which is on
  `rc::suspends`' seed list (`middle/rc.rs:1338`) and emits
  `await new Promise(wake => setTimeout(wake, n))` (`runtime.js:2141`). `Clock`
  is granted on every platform. A reconnect backoff is a tail-recursive function
  that sleeps, and it compiles today.
- *Callbacks:* `Ui.watch(run: fn(Scope) => ())` is a computation the runtime
  re-runs when its dependencies change (`ui_effect.buri:65`), and
  `onPress`/`onSubmit` are closures the host calls. **A handler may wait** —
  `$ui_flush`'s header argues the transaction is the handler's *synchronous* run
  precisely because a press may now fetch and write again on the answer
  (`runtime.js:2584`).
- *Background work:* the honest gap. `main` on WEB can mount and then loop —
  `await main()` is module top level, so a suspending loop after `mount` keeps
  the page live — but that is one loop, invisible to the graph, and an abort in
  it lands in the module's top-level `catch` (`generate.rs:517`). The
  offline-queue flush wants to be woken by a socket, not by a poll.

So the increment is **install-a-hook, not spawn-a-task.** Item 1's `Connect`
hooks *are* "react to a pushed frame"; `Location`'s popstate subscription is the
same shape. Neither needs `Tasks` on WEB, and both stay inside the one
concurrency model a page has.

**Challenges / risks.**

- **Granting `Tasks` on WEB is the tempting shortcut and it is the wrong one.**
  The grant reason is exact: `parallel` returns only when the last task has
  finished, which freezes a page (`standard_library/mod.rs:489`). A detached
  `start` dodges the freeze and breaks the other promise instead — "a task
  cannot outlive the context that granted it", which is what keeps *a program
  that never names `host.fs` cannot read a file* true of a program's lifetime
  and not only its call graph (`effect.buri:254`). Detached spawn is the single
  change in this document with the widest blast radius.
- **`core/actor` is `Tasks`-bound and so is off the table on WEB**
  (`actor.buri:264`), which removes the natural home for a client's connection
  state machine. Per-socket state threaded through hooks — `WebSocket<C, S>`'s
  `S` — is the substitute, and the server half already chose it.
- **Async coloring is inferred, not declared, and is worth protecting.**
  `Parking` is a post-monomorphization fixpoint over the call graph
  (`rc.rs:1500-1575`), so `async`/`await` never appears in Buri source and
  `async` stays on `non-goals.md:32`'s deferred list. Every new suspending host
  operation must join `rc::suspends` on the day it lands or the artifact is
  silently wrong — a `Connect` slice is exactly the change that forgets.

**Possibly stale.** "Clock.sleep blocks" — **STALE**: it suspends on JS and WEB
and `Clock` is granted everywhere. "Every effect is call-and-answer" — **STALE**
for WEB: `Ui.watch` registers a re-running computation, press and submit
handlers are host-called closures, and those handlers may suspend. "Tasks is not
granted on WEB" — CURRENT (`standard_library/mod.rs:489`), and should stay so.

## 3. Library output and JS interop

**Proposed approach.** Two requests that deserve opposite answers.

**Library output: yes, and it is small.** `ProgramRoots` has exactly two cases,
`Main` and `Tests` (`middle/monomorphize.rs:174`), and every consumer switches on
it in one place — `dce.rs:42` for the root set, `generate.rs:493` for the
epilogue, `javascript::minify(stmts, &roots, release)` for name preservation. A
third case, `Exports(Vec<(Name, FuncIdx)>)`, plus an `exports:` field on the rule
and an `export { … }` statement instead of the `try{main()}` epilogue, is a
contained change. Emit the module first; worry about `.d.ts` after.

The hard part is the **boundary**, and two commitments there are not mechanical:

- *Where the context comes from.* Every Buri function takes a `ctx` and
  JavaScript has none. SPEC 11.3 builds a context only in `main`'s body, a test,
  or a test-only module — so an exported entry has to be a new context-building
  site: `export fn createClient(config: Config): Client` binding `host.*` inside
  itself. That is a language rule change, and the right one: the export is where
  a program's authority is declared, exactly as `main` is.
- *What crosses.* Buri values on the JS backend are BigInt `Int`s, arrays for
  structs, tagged arrays for enums. A `Client` handed to React is a foreign
  object with no callable methods. The honest first version exports functions
  over JSON strings — `derive ToJson`/`FromJson` already produce them — and
  calls marshalling a follow-up.

**FFI: no, and the refusal is the feature.** An FFI hole ends "the context
declares everything a program can do": a module that can reach `window` or an npm
package has every authority the page has, and the grant table becomes decoration.
There is already a mechanism for "Buri calls something Buri did not write" — an
effect declared by a platform module whose implementation is the toolchain's
(`language/effects.md:83`) — and the bar is deliberately high, the same bar
`dependencies_stay_behind_the_bar` holds natively (`native/DECISIONS.md:58`).
"Call an npm package" should be answered with "which capability, and what is its
effect declaration"; for most browser APIs a client needs, that is a handful of
methods on one or two new effects.

**Challenges / risks.**

- **A user-declared platform module is the compromise somebody will propose, and
  it fails the same test.** If any package may declare an effect and supply JS
  for it, any dependency can mint authority and reading a context stops telling
  you what a program can do. "Declared in `BUILD.buri` with an explicit
  `foreign_sources:` and surfaced in the grant table" is a real design worth
  writing — but it is a *governance* design, not a syntax one, and it must not be
  smuggled in under "interop".
- **Exports fight minification.** `roots` is what survives renaming
  (`backend/js/mod.rs:91`); a public name that gets minified is a broken package,
  and a public name that is preserved is one DCE cannot delete. A wide public
  surface makes every artifact bigger for every consumer.
- **What it forecloses:** once a JS-facing surface ships, its value
  representation is an ABI. Today that representation is entirely private and the
  backend changes it freely.

**Possibly stale.** "one self-contained ES module whose only entry is main, with
no export statements" — CURRENT (`monomorphize.rs:174`, `generate.rs:493`), with
one correction: a WEB build additionally writes `main.css` and a static
`index.html` shell (`build/actions.rs:1688`). "no foreign-function mechanism" —
CURRENT for user code; the FFI in `native/VALUE-MODEL.md:627` is the compiler's
own boundary to `cli/runtime` and is unreachable from Buri.

## 4. Schema-derived typing

**Proposed approach.** Per-schema code generation, following `.proto` exactly,
and **no type-level computation**.

The precedent is complete and it works: a `.proto` listed in `proto_sources:`
becomes a module at an import path that is the file's own name — `from
"//libs/wire/point.proto" import { Point }` — with nothing written to the source
tree and no generation step to forget (`docs/guides/proto.md`). The generated
code is ordinary Buri, "checked by the real checker, optimised by the real
optimiser, and needs no intrinsic and no runtime privilege" (`proto.buri:15`). A
`//db/schema.<ext>` in a new `schema_sources:` field, producing concrete structs,
a query type per collection, and `derive FromJson`/`ToJson` on each, is the same
machine with a different front end.

Two increments: first the codegen path with a deliberately small schema
language — collections, field types, references — which covers document shapes;
then the query surface, where the discipline actually pays, so `find(ctx, Users,
…)` answers a concrete `User` rather than a `Value`.

**Challenges / risks.**

- **Associated types are the "principled" answer and they are refused for a
  reason that is not aesthetic.** `non-goals.md:32` defers blanket impls,
  associated types, `where` clauses, supertraits and foreign impls *together*,
  because each turns trait resolution from a lookup into a search
  (`language/types.md:665`), and open question 7 names the real risk: "the
  difficulty of refusing the next request", in a compiler whose architecture
  assumes constant-time resolution. A schema-typing feature is exactly the
  reasonable-looking request that argument was written to refuse.
- **The schema has to leave Buri source, and that is the decision to make
  first.** The TypeScript client derives types from a schema *value* written in
  TypeScript. A codegen path derives them from a *file* in a schema language, so
  every "the schema is just code" ergonomic — composing it, parameterizing it,
  computing it — is lost. The compensation is real types with real errors instead
  of a conditional-type tower whose failures are unreadable. Say which trade is
  being taken, out loud, before writing a parser.
- **A new schema language is a new artifact** with a version, a formatter, a
  language-server story and an error catalogue; `.proto` got all four. Budget for
  them, or reuse `.proto` itself as the schema language — a genuinely serious
  option that costs nothing new.
- **No anonymous records means every projection is a named type**
  (`language/types.md:161`). `select { name, email }` produces a struct the
  generator must name, and the naming scheme is a permanent public surface. This
  is the one place the type system's simplicity really does cost the API
  something, and there is no cheap fix: a `derive` that generates a *new type* is
  a language change, and `non-goals.md` calls it load-bearing well beyond this.

**Possibly stale.** All four claims are CURRENT: no type-level computation, no
associated types (`non-goals.md:32`, `types.md:665`), no anonymous records
(`types.md:161`), and `.proto` is the only per-schema generation. One addition
worth knowing: `derive FromJson`/`ToJson` exist and are *only* ever derived
(`types.md:622`), and `json.decode` already takes its type from an annotation and
dispatches on a compiler-emitted descriptor (`native/VALUE-MODEL.md:620`) — so
generated code has real machinery to lean on. The 6,600-of-10,300 line count is
about the TypeScript client and is not checkable here, but the shape of the claim
is right: those lines do at the type level what codegen does at build time, so
they are deleted rather than ported.

## Deterministic tests with multi-threading

Now that actors and tasks allow for multiple threads, I want to be able to do deterministic simulation testing which involves deterministically figuring out the order which these async things are done in. I don't think this is possible with the current language design.

**Proposed approach.** More of this exists than the line assumes, and the missing
piece is narrower and more tractable than "a language design problem".

What already ships: `host_testing.TestTasks` is a schedulable double.
`anyOrder()` runs one seeded order, `everyOrder()` runs the whole `test` body
once per completion order (`n!`, refused above six), `seed(n)` replays a named
one, and `faults(plan)` injects task failures with a promise that every planned
one is reached (`host_testing.buri:1491-1542`). A failure report prints the order
and the `tasks().seed(k)` that replays it (`runtime.js:4375`). `TestClock`,
`TestRand` (seeded xorshift32, *the same sequence on both backends*),
`TestSockets` and the `Fs`/`Net` doubles are all there. That is a real DST
harness, and the effect model is why: a test binds a different `Tasks`, and the
program under test cannot tell.

Three gaps, in the order I would close them.

1. **Interleaving granularity — the real one.** `TestTasks.parallel` runs each
   task **to completion, one at a time**, in a chosen permutation
   (`runtime.js:4362-4369`). It chooses an order of *tasks*, never an order of
   *steps*, so "A runs to its first await, B runs, A resumes" is not a schedule
   this harness can produce — and that interleaving is where the bugs DST exists
   to find actually live. The instrumentation point already exists and is
   precise: `middle::rc`'s `Parking` knows per instantiation exactly which calls
   suspend (`rc.rs:1500`), and on JS every one of them is a literal `await`. A
   deterministic scheduler is a `TestTasks` that resolves those awaits in a
   seeded order instead of the engine's. That is runtime and backend work, not a
   language change.
2. **Actors are not interceptable.** `actor.*` are nine intrinsic *free
   functions* with a `C: Tasks` bound (`actor.buri:508`, `:524`;
   `monomorphize.rs:2215-2223`), not methods of an effect — so binding
   `TestTasks` gates the authority and redirects nothing. Mailbox interleaving is
   the runtime's, and a test cannot choose it. The fix is the one `Listen`
   already models: make the operations an effect so a double can implement them,
   at the cost of nine more effect methods.
3. **No logical clock orders waits across tasks.** `TestClock.sleepMillis`
   advances a per-handle counter and returns immediately
   (`runtime.js:4711`) — it is not on `rc::suspends` (`rc.rs:5311`), so it does
   not even park, and two tasks holding two clocks have two independent times.
   FDB-style DST is built on a single virtual clock that orders every wait; that
   is what would let "task A's 50ms timer fires before task B's 100ms fetch" be a
   schedule rather than an accident.

**Challenges / risks.**

- **Determinism claims have to survive the release backend, and today they do not
  need to.** Tests bind `TestTasks`, so nothing about the real scheduler is
  asserted; the native release backend fans tasks onto carriers on real
  processors (`tasks.buri:44`). A DST harness that finds a bug proves it about
  the *double*. That is still valuable — it is how FDB works — but only if the
  double's interleaving points are exactly the real one's, which is the argument
  for seeding the scheduler off `Parking` rather off a hand-written list.
- **A latent hole worth checking before building on this.**
  `host_testing.TestTasks.parallel` is *not* on `rc::suspends`' seed list
  (`rc.rs:5311`), and `$host_testing_TestTasks_parallel` calls `f(ctx, …)`
  without awaiting (`runtime.js:4366`). No test in the tree runs a task that
  sleeps or fetches under the double, so a waiting task body under `TestTasks`
  looks like untested territory — and it is precisely the case a DST harness is
  made of.
- **`everyOrder` is `n!` and refuses above six.** Exhaustive exploration does not
  scale to interleaving-level schedules; the answer there is seeded random search
  with a replayable seed, which the harness already has the reporting for.
- **What it forecloses:** making `actor.*` an effect widens the effect surface by
  nine methods and commits their signatures, and every one of them was
  deliberately kept out of `Listen`'s style of table. Worth it only if actor
  ordering is genuinely something tests must choose.

**Possibly stale.** "Actors and tasks allow for multiple threads" — CURRENT, and
the mechanism is real: `crosses_tasks` marks the whole program so shared blocks
take atomic counts (`rc.rs:1370`), and a suspended task is a saved stack pointer
on a carrier pool (`native/DECISIONS.md:76`). "I don't think this is possible
with the current language design" — **mostly STALE, and importantly so**:
ordering *is* choosable today at task granularity, exhaustively, replayably, with
fault injection (`host_testing.buri:1491`), because effects-as-capabilities made
the scheduler a value a test can swap. What is not possible is choosing an order
of *steps within* tasks, and choosing actor mailbox order at all — and neither
needs a language change, only a scheduler and one effect-ification.
