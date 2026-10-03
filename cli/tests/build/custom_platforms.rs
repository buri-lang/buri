//! A repository platform's artifact, run by the host it was written for.
//!
//! `repositories/custom-platforms/cloudflare_kv` builds the walkthrough, and
//! `repositories/custom-platforms/cloudflare_worker` the workers, and both pin
//! what the CLI prints. What a manifest can't do is be the worker runtime:
//! these tests import the `fetch.mjs` the build wrote into a JavaScript
//! runtime, hand its default export a request and the bindings a worker gets,
//! and read the response.
use crate::harness::*;

fn walkthrough() -> Scratch {
    Scratch::copy_of("custom-platform", &tests_dir().join("repositories/custom-platforms/cloudflare_kv/repo"))
}

/// What the worker answers for each token, one `status body` line each.
const CALL: &str = r#"
import worker from "./.buri/out/platform/cloudflare_worker/cmd/site/fetch.mjs";
const store = new Map([["session:t2", "ada"]]);
const kv = {
  get: async (key) => { await new Promise((r) => setTimeout(r, 5)); return store.get(key) ?? null; },
  put: async (key, value) => { store.set(key, value); },
};
for (const token of ["t2", "t1"]) {
  const request = new Request("https://site.example/", { headers: { "x-session": token } });
  const response = await worker.fetch(request, { SESSIONS: kv });
  console.log(`${response.status} ${await response.text()}`);
}
"#;

fn call(scratch: &Scratch) -> String {
    scratch.write("call.mjs", CALL);
    let out = std::process::Command::new(js_runtime())
        .arg(scratch.path("call.mjs"))
        .current_dir(&scratch.root)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "the worker failed:\n{stderr}");
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The default export answers from the KV binding it was handed. The
/// binding's `get` resolves on a later turn, so the greeting arrives only if
/// the program awaited `HostKv.get`, which `fetch.mjs` implements.
#[test]
fn the_worker_answers_from_its_kv_binding_and_awaits_it() {
    let scratch = walkthrough();
    scratch.run(&["build", "//cmd/site"]).ok();
    assert_eq!(call(&scratch), "200 hello ada\n401 \n");
}

/// Editing the platform's `js` file rebuilds the output, with no `buri clean`.
#[test]
fn editing_the_platform_s_js_file_rebuilds_its_outputs() {
    let scratch = walkthrough();
    scratch.run(&["build", "//cmd/site"]).ok();
    assert_eq!(call(&scratch), "200 hello ada\n401 \n");
    scratch.edit(
        "platform/cloudflare_worker/fetch.mjs",
        "return fetch(request);",
        "return fetch(request).then((r) => new Response(\"edited\", { status: r.status }));",
    );
    scratch.run(&["build", "//cmd/site"]).ok();
    assert_eq!(call(&scratch), "200 edited\n401 edited\n");
}

/// `repositories/custom-platforms/cloudflare_worker`: Cloudflare Workers as a
/// repository platform, with its `Vars` effect.
fn workers() -> Scratch {
    Scratch::copy_of("cloudflare-worker", &tests_dir().join("repositories/custom-platforms/cloudflare_worker/repo"))
}

/// Builds `target`, then runs `driver` under the JavaScript runtime from the
/// repository root, answering what it printed. The driver is the worker
/// runtime's half: it imports the artifact's default export and calls `fetch`.
fn drive(scratch: &Scratch, target: &str, driver: &str) -> String {
    scratch.run(&["build", target]).ok();
    scratch.write("drive.mjs", driver);
    let out = std::process::Command::new(js_runtime())
        .arg(scratch.path("drive.mjs"))
        .current_dir(&scratch.root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "the worker failed:\n{stdout}{stderr}");
    stdout
}

/// A worker driven by the runtime's own `Request` and `Response`: each of the
/// seven methods HTTP has, the path routed on, the query string beside it, a
/// header read by name, a header written on the way out, a body echoed, a
/// body that is not text, and the answers that are not `200`, a `204` among
/// them, which a Fetch `Response` refuses any body for. Each is a
/// field of the crossing, and a crossing that lost one would still answer the
/// rest.
#[test]
fn a_worker_answers_the_runtimes_request_with_the_runtimes_response() {
    let said = drive(&workers(), "//cmd/site", SITE);
    assert_eq!(
        said,
        "200 text/plain; charset=utf-8 GET /about\n\
         200 text/plain; charset=utf-8 GET /\n\
         200 text/plain; charset=utf-8 HEAD /\n\
         200 text/plain; charset=utf-8 POST /\n\
         200 text/plain; charset=utf-8 PUT /\n\
         200 text/plain; charset=utf-8 PATCH /\n\
         200 text/plain; charset=utf-8 DELETE /\n\
         200 text/plain; charset=utf-8 OPTIONS /\n\
         200 text/plain; charset=utf-8 hello\n\
         200 text/plain; charset=utf-8 not utf-8\n\
         200 text/plain; charset=utf-8 ref=x&page=2\n\
         200 text/plain; charset=utf-8 please\n\
         200 text/plain; charset=utf-8 nothing\n\
         200 text/plain; charset=utf-8 tagged x-answered=yes\n\
         404 null \n\
         500 null \n\
         204 null \n",
        "the crossing lost a field"
    );
}

const SITE: &str = r#"
import worker from "./.buri/out/platform/cloudflare_worker/cmd/site/fetch.mjs";

const say = async (request) => {
  const answer = await worker.fetch(request);
  const tag = answer.headers.get("x-answered");
  const said = `${answer.status} ${answer.headers.get("content-type")} ${await answer.text()}`;
  console.log(tag === null ? said : `${said} x-answered=${tag}`);
};

await say(new Request("https://example.com/about?ref=x#top"));

// Every method, on a path that answers with the one it was called by. A `GET`
// and a `HEAD` may carry no body, so none of these do.
for (const method of ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"]) {
  await say(new Request("https://example.com/", { method }));
}

await say(new Request("https://example.com/echo", { method: "POST", body: "hello" }));
// A body that is not text at all: the bridge carries octets, so what comes back
// is the entry's own excuse rather than a crash.
await say(
  new Request("https://example.com/echo", { method: "POST", body: new Uint8Array([0xff, 0xfe]) }),
);
await say(new Request("https://example.com/query?ref=x&page=2#top"));
await say(new Request("https://example.com/header", { headers: { "X-Asked": "please" } }));
await say(new Request("https://example.com/header"));
await say(new Request("https://example.com/tagged"));
await say(new Request("https://example.com/missing"));
await say(new Request("https://example.com/broken"));
await say(new Request("https://example.com/empty"));
"#;

/// A worker reads its vars and secrets through the repository's `Vars`
/// effect, which `fetch.mjs` implements from the `env` the runtime passes
/// beside the request.
///
/// The driver hands `fetch` an `env` the way a worker runtime does: string
/// bindings, which are what a `[vars]` entry and a secret both arrive as, and
/// one binding that is not a string, which is what a KV namespace or any
/// other resource arrives as. A variable the `env` does not carry is `.None`,
/// and the name is one the JavaScript runtime's own process environment does
/// not have either, so the value can only have come from the argument.
#[test]
fn a_worker_reads_its_variables_from_the_env_the_runtime_passes() {
    let said = drive(&workers(), "//cmd/vars", VARS);
    assert_eq!(
        said,
        "200 hello from a var\n\
         200 s3cret\n\
         404 \n\
         404 \n\
         200 BURI_WORKER_SECRET,GREETING\n",
        "the worker did not read the env it was handed"
    );
}

const VARS: &str = r#"
import worker from "./.buri/out/platform/cloudflare_worker/cmd/vars/fetch.mjs";

const env = {
  GREETING: "hello from a var",
  BURI_WORKER_SECRET: "s3cret",
  STORE: { get() {} },
};

const say = async (path) => {
  const answer = await worker.fetch(new Request(`https://example.com${path}`), env, {});
  console.log(`${answer.status} ${await answer.text()}`);
};

await say("/GREETING");
await say("/BURI_WORKER_SECRET");
await say("/BURI_WORKER_NOT_BOUND");
await say("/STORE");
await say("/all");
"#;

/// **A `core/lazy` chunk is fetched only by the request that reaches the
/// `load`.** A worker is called per request, so one process takes two paths
/// through one artifact.
///
/// The chunk file is deleted between the runs, which is the only evidence a
/// program that merely did not print cannot fake. With no chunk on disk, `/`
/// answers and `/heavy` fails; with a chunk that is not a module, the same;
/// with the chunk there, both answer.
#[test]
fn a_chunk_is_fetched_only_where_the_worker_asks_for_it() {
    let scratch = workers();
    let first = drive(&scratch, "//cmd/lazy", LAZY);
    assert_eq!(first, "/home\nheavy /heavy\n", "with the chunk there, both answer");

    let chunk = scratch.path(".buri/out/platform/cloudflare_worker/cmd/lazy/fetch.0.mjs");
    let held = std::fs::read(&chunk).expect("the build wrote a chunk beside the module");
    let run = || {
        let out = std::process::Command::new(js_runtime())
            .arg(scratch.path("drive.mjs"))
            .current_dir(&scratch.root)
            .output()
            .expect("the javascript runtime runs");
        String::from_utf8_lossy(&out.stdout).to_string()
    };

    std::fs::remove_file(&chunk).expect("the chunk is removable");
    assert_eq!(
        run(),
        "/home\nno chunk\n",
        "a request that never reaches the `load` needs no chunk, and one that does needs it"
    );

    // A file that is there and is not a module: half a download, or a
    // truncated deploy. Neither may cost the requests that never reach the
    // `load`.
    std::fs::write(&chunk, b"export const $bind = (").expect("the chunk is writable");
    assert_eq!(run(), "/home\nno chunk\n", "a chunk that will not load must cost only the request that needed it");

    std::fs::write(&chunk, &held).expect("the chunk goes back");
    assert_eq!(run(), "/home\nheavy /heavy\n", "and with the chunk there, both answer");
}

const LAZY: &str = r#"
import worker from "./.buri/out/platform/cloudflare_worker/cmd/lazy/fetch.mjs";

const say = async (path) => {
  try {
    const answer = await worker.fetch(new Request("https://example.com" + path));
    console.log(await answer.text());
  } catch (e) {
    console.log("no chunk");
  }
};

await say("/home");
await say("/heavy");
"#;

/// A worker that dials a socket while answering a request, five times, and
/// each dial ends a different way.
///
/// The driver is the runtime's half, the worker's default export with a real
/// `Request` and `Response`, and a `WebSocket` double. A worker parks on the
/// dial, so `fetch` answers only once the socket has ended.
///
/// Five requests, because a browser's `WebSocket` has five endings:
///
/// | The event | Before the socket opened | After |
/// |---|---|---|
/// | `close` with a code | the handshake failed: `.Err` | that code's `CloseReason` |
/// | `close` with no code | the handshake failed: `.Err` | 1005, which is `.Abnormal` |
/// | `error` | the handshake failed: `.Err` | 1006, which is `.Abnormal` |
///
/// No hook runs for a socket that never opened, which is the absence of a
/// `sent` line in the log.
#[test]
fn a_worker_dials_a_socket_while_it_answers_a_request() {
    let said = drive(&workers(), "//cmd/relay", RELAY);
    assert_eq!(
        said,
        "200 ended .Normal 3\n\
         200 ended .Abnormal 0\n\
         200 ended .Abnormal 0\n\
         200 no socket: the connection failed before the handshake finished\n\
         200 no socket: the socket closed before the handshake finished\n\
         dialled wss://example.test/relay\n\
         sent asking /rooms/9\n\
         sent heard text 4:pong\n\
         sent heard text 0:\n\
         sent heard binary 3\n\
         dialled wss://example.test/relay\n\
         sent asking /b\n\
         dialled wss://example.test/relay\n\
         sent asking /c\n\
         dialled wss://example.test/relay\n\
         dialled wss://example.test/relay\n",
        "a worker did not dial, lost what it heard, or read an ending as the wrong one"
    );
}

const RELAY: &str = r#"
import worker from "./.buri/out/platform/cloudflare_worker/cmd/relay/fetch.mjs";

// Five dials, five endings, one script each. Everything is on a timer, so the
// worker parks on each step and nothing here runs unless the event loop is
// free — a `connect` that held it would not reach the open, let alone the
// close.
const scripts = [
  // A socket that opened, carried three messages of three shapes, and closed
  // normally. The empty text is a message like any other; the binary one
  // arrives as the `ArrayBuffer` the runtime asked `binaryType` for.
  (ws) => {
    setTimeout(() => ws.onopen({}), 5);
    setTimeout(() => ws.onmessage({ data: "pong" }), 10);
    setTimeout(() => ws.onmessage({ data: "" }), 15);
    setTimeout(() => ws.onmessage({ data: new Uint8Array([1, 2, 3]).buffer }), 20);
    setTimeout(() => ws.onclose({ code: 1000, wasClean: true }), 25);
  },
  // A close carrying no code at all, which RFC 6455 calls 1005.
  (ws) => {
    setTimeout(() => ws.onopen({}), 5);
    setTimeout(() => ws.onclose({ wasClean: true }), 10);
  },
  // An error on a socket that was open, which is the connection breaking:
  // 1006, and there is no close frame to say otherwise.
  (ws) => {
    setTimeout(() => ws.onopen({}), 5);
    setTimeout(() => ws.onerror({}), 10);
  },
  // The same two events before the handshake finished, which are not endings
  // at all — they are a socket that never opened.
  (ws) => {
    setTimeout(() => ws.onerror({}), 5);
  },
  (ws) => {
    setTimeout(() => ws.onclose({ code: 1006, wasClean: false }), 5);
  },
];

const log = [];
let at = 0;
globalThis.WebSocket = class {
  constructor(url) {
    log.push(`dialled ${url}`);
    this.protocol = "";
    this.extensions = "";
    scripts[at++](this);
  }
  send(data) {
    log.push(`sent ${data}`);
  }
  close(code) {
    log.push(`closed ${code}`);
  }
};

for (const path of ["/rooms/9", "/b", "/c", "/d", "/e"]) {
  const answer = await worker.fetch(new Request(`https://example.com${path}`));
  console.log(`${answer.status} ${await answer.text()}`);
}
console.log(log.join("\n"));
"#;
