//! `web`'s `Storage`, through the IndexedDB adapter in its `main.mjs`.
//!
//! `repositories/platform/storage_on_a_page` runs the page's `visit` against
//! `web/storage/testing`'s store. What a manifest can't be is a browser: these
//! tests build the page, hand the JavaScript runtime a fake `indexedDB`, and
//! import the built `main.mjs`, which runs `main` as a page load does. Importing
//! it again under another query string is a reload: a fresh module over the
//! same database.
use crate::harness::*;

fn page() -> Scratch {
    let scratch = Scratch::copy_of(
        "web-storage",
        &tests_dir().join("repositories/platform/storage_on_a_page/repo"),
    );
    scratch.run(&["build", "//cmd/page"]).ok();
    scratch
}

/// An in-memory IndexedDB: the calls `main.mjs` makes, each answering on a
/// later turn, and a transaction that commits only if its values fit the
/// quota. `refuse` fails the open the way a private window does.
const FAKE: &str = r#"
const later = (f) => setTimeout(f, 1);

export function install({ quota = Infinity, refuse } = {}) {
  const stores = new Map();
  const db = {
    createObjectStore: (name) => stores.set(name, new Map()),
    transaction(name, mode) {
      const pending = new Map(stores.get(name));
      const tx = { error: null, oncomplete: null, onabort: null };
      const answer = (result) => ({ result, error: null });
      const writable = () => {
        if (mode !== "readwrite") throw new DOMException("read only", "ReadOnlyError");
      };
      tx.objectStore = () => ({
        get: (key) => answer(pending.has(key) ? new Uint8Array(pending.get(key)) : undefined),
        put: (value, key) => (writable(), pending.set(key, new Uint8Array(value)), answer(key)),
        delete: (key) => (writable(), pending.delete(key), answer(undefined)),
        getAllKeys: (range) =>
          answer(
            [...pending.keys()]
              .filter((k) => range === undefined || (k >= range.lower && k < range.upper))
              .sort(),
          ),
      });
      later(() => {
        const size = [...pending.values()].reduce((n, v) => n + v.byteLength, 0);
        if (size > quota) {
          tx.error = new DOMException("the quota is spent", "QuotaExceededError");
          tx.onabort();
          return;
        }
        stores.set(name, pending);
        tx.oncomplete();
      });
      return tx;
    },
  };
  globalThis.IDBKeyRange = {
    bound: (lower, upper, lowerOpen, upperOpen) => {
      if (lowerOpen || !upperOpen) throw new Error("main.mjs asked for a range the fake lacks");
      return { lower, upper };
    },
  };
  globalThis.indexedDB = {
    open(name, version) {
      const request = {};
      later(() => {
        if (refuse !== undefined) {
          request.error = new DOMException(refuse, "SecurityError");
          request.onerror();
          return;
        }
        request.result = db;
        if (stores.size === 0) request.onupgradeneeded();
        request.onsuccess();
      });
      return request;
    },
  };
}
"#;

/// Installs the fake with `options`, then loads the page `loads` times,
/// answering what it printed.
fn load(scratch: &Scratch, options: &str, loads: usize) -> String {
    scratch.write("fake.mjs", FAKE);
    let mut driver = format!("import {{ install }} from \"./fake.mjs\";\ninstall({options});\n");
    for n in 0..loads {
        driver.push_str(&format!("await import(\"./.buri/out/web/cmd/page/main.mjs?load={n}\");\n"));
    }
    scratch.write("load.mjs", &driver);
    let out = std::process::Command::new(js_runtime())
        .arg(scratch.path("load.mjs"))
        .current_dir(&scratch.root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "the page failed:\n{stdout}{stderr}");
    stdout
}

/// Each load reads what the last one wrote: the count, the keys under a
/// prefix, and the key the last load deleted gone. The fake answers on a later
/// turn, so a count arrives only if the program awaited every call.
#[test]
fn a_reloaded_page_reads_what_the_last_load_wrote() {
    assert_eq!(
        load(&page(), "{}", 3),
        "visit 1, keeping visit/1\n\
         visit 2, keeping visit/1 visit/2\n\
         visit 3, keeping visit/2 visit/3\n",
    );
}

/// A write past the quota is `.QuotaExceeded`, which the page prints, rather
/// than an abort.
#[test]
fn a_full_quota_is_an_error_the_page_handles() {
    assert_eq!(load(&page(), "{ quota: 3 }", 1), "storage is full\n");
}

/// An open the browser refuses is `.Refused` with its reason, and so is a
/// runtime with no IndexedDB at all.
#[test]
fn refused_storage_is_an_error_the_page_handles() {
    let scratch = page();
    assert_eq!(
        load(&scratch, "{ refuse: \"site data is blocked\" }", 1),
        "storage refused: site data is blocked\n",
    );
    scratch.write("bare.mjs", "await import(\"./.buri/out/web/cmd/page/main.mjs\");\n");
    let out = std::process::Command::new(js_runtime())
        .arg(scratch.path("bare.mjs"))
        .current_dir(&scratch.root)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "storage refused: this browser has no IndexedDB\n");
}
