const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[1200n,'lay-col'];
const $k2=[6200n,'bg-t_both_bg'];
const $k3=[$k1,$k2];
const $k4=[$k3];
const $k5=[5,$k4];
const $k6=[$k5];
const $k7=[1200n,'lay-row'];
const $k8=[$k7];
const $k9=[$k8];
const $k10=[5,$k9];
const $k11=[0,255n,255n,255n];
$ui_sheet='*,*::before,*::after{box-sizing:border-box}\n*,*::before,*::after{border-width:0}\n*{overflow-wrap:break-word}\n:where(body){margin:0}\n:where(div,nav,main,header,footer,aside,article,search,ul,li,hr,form,a,button){display:flex;flex-direction:column}\n.lay-col{display:flex;flex-direction:column}\n.lay-row{display:flex;flex-direction:row}\n.w-var{width:var(--buri-w)}\n.bg-t_both_bg{background-color:var(--both-bg)}\n';
$tree_declare_hook=$tree_declare;
$ui_theme_hook=$ui_theme_install;
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1],$k0[10],$k0[11]];
  const width_2=[$host_HostUi_signal(ctx_1[2],40n)];
  const self_8=$host_HostStdout_println(ctx_1[1],'both');
  let $t1;
  if(self_8[0]===0){
    $t1=0;
  }else if(self_8[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const $t3=ui_node$stack$gs3xpc([$k6,[ui_node$stack$gs3xpc([[$k10,[4,scope_3=>[[24,[0,$effect_Scope_read(scope_3,width_2[0])]]]]],[],void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0])],void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0]);
  const bindings_14=[[__cmd_x_main_buri$Token_color(0),__cmd_x_main_buri$light(0)]];
  return $ui_node_mount(ctx_1,$t3,[[[0,bindings_14]]]);
}
function ui_node$stack$gs3xpc(config_0){
  const events_1=[config_0[3],config_0[4],config_0[5],config_0[6],config_0[7],config_0[8],config_0[9],config_0[10]];
  const $t1=config_0[2];
  if($t1!==void 0){
    return [[4,$t1,$share(config_0[0]),$share(config_0[1]),events_1]];
  }else if($t1===void 0){
    return [[3,$share(config_0[0]),$share(config_0[1]),events_1]];
  }else{
    $abort('no arm matched');
  }
}
function __cmd_x_main_buri$light(t_0){
  return $k11;
}
function __cmd_x_main_buri$Token_color(self_0){
  return [2,['both','bg']];
}
const $buri$program={main:async ()=>{
  const $r=await __cmd_x_main_buri$main();
  $host.flush();
  return void $crossThrow($r);
}};
const $buri$ui={signal:(v)=>$host_HostUi_signal(null,v),write:(id,v)=>{$host_HostUi_write(null,id,v)}};
var $buri$host;
const $buri$platform = await (async () => {
$buri$host = () => ({ HostLocation: HostLocation, IndexedDb: IndexedDb });
// web's entry adapter. It starts the page's `main` when the module loads,
// implements `HostLocation`, the address bar, over the browser's own
// `location` and `history`, and `IndexedDb`, under `HostStorage`, over
// `indexedDB`.
//
// A JavaScript host that is not a browser has neither, so each is asked for
// before it is used: the page still runs to its first paint under bun, at `/`,
// with nowhere to push an address.
const { main } = $buri$program;
const { signal, write } = $buri$ui;

// The signal holding the path, made on first ask so a page that never routes
// registers nothing, and written from `popstate`, which is what the browser
// fires when the reader goes back or forward.
let address;

function current() {
  if (typeof location === "undefined" || location === null) return "/";
  return location.pathname || "/";
}

function hasHistory() {
  return typeof history !== "undefined" && history !== null;
}

// `ui/web`'s `navigate` and `replace` call `push` or `replace` and then write
// the signal themselves, so the graph and the address bar move together
// however the address was reached.
const HostLocation = {
  path: (self) => {
    if (address === undefined) {
      address = signal(current());
      if (typeof addEventListener === "function") {
        addEventListener("popstate", () => write(address, current()));
      }
    }
    return address;
  },
  push: (self, path) => {
    if (hasHistory()) history.pushState({}, "", path);
  },
  replace: (self, path) => {
    if (hasHistory()) history.replaceState({}, "", path);
  },
};

// `IndexedDb`, the store behind `HostStorage`: one object store, `values`, in
// a database named `buri`. Every method answers a promise, which the program
// awaits, and a failure throws the browser's reason. A full quota throws
// exactly `QuotaExceededError`, which `platform.buri` tells apart from a refusal.
let opened;

function database() {
  opened ??= new Promise((resolve, reject) => {
    if (typeof indexedDB === "undefined" || indexedDB === null) {
      throw new Error("this browser has no IndexedDB");
    }
    const request = indexedDB.open("buri", 1);
    request.onupgradeneeded = () => request.result.createObjectStore("values");
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  return opened;
}

function reason(e) {
  if (e && e.name === "QuotaExceededError") return new Error("QuotaExceededError");
  return new Error((e && (e.message || e.name)) || String(e));
}

// One request in a transaction of its own, answering its result once the
// transaction commits: a quota is only enforced at the commit.
async function transact(mode, act) {
  try {
    const db = await database();
    return await new Promise((resolve, reject) => {
      const tx = db.transaction("values", mode);
      const request = act(tx.objectStore("values"));
      tx.oncomplete = () => resolve(request.result);
      tx.onabort = () => reject(request.error ?? tx.error);
    });
  } catch (e) {
    throw reason(e);
  }
}

// The keys starting with `prefix`: from `prefix` up to the first string past
// every one of them, or every key for an empty prefix.
function startingWith(prefix) {
  for (let i = prefix.length - 1; i >= 0; i--) {
    const c = prefix.charCodeAt(i);
    if (c < 0xffff) {
      return IDBKeyRange.bound(prefix, prefix.slice(0, i) + String.fromCharCode(c + 1), false, true);
    }
  }
  return undefined;
}

const IndexedDb = {
  get: (self, key) => transact("readonly", (store) => store.get(key)),
  put: (self, key, value) => transact("readwrite", (store) => store.put(value, key)),
  delete: (self, key) => transact("readwrite", (store) => store.delete(key)),
  keys: (self, prefix) => transact("readonly", (store) => store.getAllKeys(startingWith(prefix))),
};

// `.Err(msg)` arrives as a thrown `Error`, and the message goes to the console.
// Under a host with a process, such as bun, the process exits 1.
await main().catch((e) => {
  console.error(e && e.message ? e.message : String(e));
  if (typeof process !== "undefined") process.exitCode = 1;
});
return { "HostLocation": HostLocation, "IndexedDb": IndexedDb };
})();
