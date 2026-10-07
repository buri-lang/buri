const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[4800n,'w-20'];
const $k2=[$k1];
const $k3=[$k2];
const $k4=[5,$k3];
const $k5=[5000n,'h-20'];
const $k6=[$k5];
const $k7=[$k6];
const $k8=[5,$k7];
$ui_sheet='*,*::before,*::after{box-sizing:border-box}\n*,*::before,*::after{border-width:0}\n*{overflow-wrap:break-word}\n:where(body){margin:0}\n:where(div,nav,main,header,footer,aside,article,search,ul,li,hr,form,a,button){display:flex;flex-direction:column}\n@keyframes buri-pulse{50%{opacity:0.5}}\n@keyframes buri-spin{to{rotate:360deg}}\n.w-20{width:20px}\n.h-20{height:20px}\n@media (prefers-reduced-motion:no-preference){.anim-var{animation:var(--buri-anim)}}\n';
$tree_declare_hook=$tree_motion();
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1],$k0[10],$k0[11]];
  const busy_2=[$host_HostUi_signal(ctx_1[2],true)];
  const self_7=$host_HostStdout_println(ctx_1[1],'busy');
  let $t1;
  if(self_7[0]===0){
    $t1=0;
  }else if(self_7[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const config_12=[[$k4,$k8,[62,$host_HostWatch_read(ctx_1[3],busy_2[0])?1:0]],[],void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0];
  const events_13=ui_node$listen$b1px4q(config_12[3],config_12[4],config_12[5],config_12[6],config_12[7],config_12[8],config_12[9],config_12[10]);
  let $t3;
  const $t4=config_12[2];
  if($t4!==void 0){
    $t3=[[4,$t4,$share(config_12[0]),$share(config_12[1]),events_13]];
  }else if($t4===void 0){
    $t3=[[3,$share(config_12[0]),$share(config_12[1]),events_13]];
  }else{
    $abort('no arm matched');
  }
  return $ui_node_mount(ctx_1,$t3,[]);
}
function ui_node$listen$b1px4q(onHover_0,onFocus_1,onScroll_2,onKey_3,onPressOutside_4,onPointerDown_5,onPointerMove_6,onPointerUp_7){
  return [onHover_0,onFocus_1,onScroll_2,onKey_3,onPressOutside_4,onPointerDown_5,onPointerMove_6,onPointerUp_7];
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
