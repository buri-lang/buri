// platform/cloudflare_worker/fetch.mjs
import { fetch } from "buri:program";
let bindings = {};

export default { fetch(request, env) { bindings = env; return fetch(request); } };
export const HostKv = {
  get: async (self, namespace, key) => (await bindings[namespace].get(key)) ?? undefined,
  put: (self, namespace, key, value) => bindings[namespace].put(key, value),
};
