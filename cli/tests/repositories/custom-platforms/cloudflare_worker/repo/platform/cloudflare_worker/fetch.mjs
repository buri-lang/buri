// platform/cloudflare_worker/fetch.mjs
import { fetch } from "buri:program";

// The `env` the runtime passes beside each request. Every request an isolate
// serves gets the same bindings, so one variable holds them.
let bindings = {};

export default {
  fetch(request, env) {
    bindings = env ?? {};
    return fetch(request);
  },
};

// A var and a secret both arrive as a string; a KV namespace or any other
// resource arrives as an object, and is no variable.
export const HostVars = {
  get: (self, name) => (typeof bindings[name] === "string" ? bindings[name] : undefined),
  all: (self) => Object.entries(bindings).filter(([, value]) => typeof value === "string"),
};
