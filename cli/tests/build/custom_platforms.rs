//! A repository platform's artifact, run by the host it was written for.
//!
//! `repositories/custom-platforms/cloudflare_kv` builds the walkthrough and
//! pins what the CLI prints. What a manifest can't do is be the worker
//! runtime: these tests import the `fetch.mjs` the build wrote into a JavaScript
//! runtime, hand its default export a request and a Map-backed KV binding, and
//! read the response.
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
