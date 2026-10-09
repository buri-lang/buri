const $k0=[1200n,'lay-row'];
const $k1=[3400n,'gap-8'];
const $k2=[$k0,$k1];
const $k3=[$k2];
const $k4=[5,$k3];
const $k5=[$k4];
const $k6=[0,'Loading'];
const $k7=[4800n,'w-20'];
const $k8=[5000n,'h-20'];
const $k9=[12400n,'anim-spin'];
const $k10=[$k7,$k8,$k9];
const $k11=[$k10];
const $k12=[5,$k11];
const $k13=[$k12];
const $k14=[$k6,$k13,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0];
const $k15=[12405n,'hover_anim-spin'];
const $k16=[12402n,'md_anim-spin'];
const $k17=[$k7,$k15,$k16];
const $k18=[$k17];
const $k19=[5,$k18];
const $k20=[$k19];
const $k21=[$k20,[],void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0];
const $k22=[[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k23=[0,false];
$ui_sheet='*,*::before,*::after{box-sizing:border-box}\n*,*::before,*::after{border-width:0}\n*{overflow-wrap:break-word}\n:where(body){margin:0}\n:where(div,nav,main,header,footer,aside,article,search,ul,li,hr,form,a,button){display:flex;flex-direction:column}\n@keyframes buri-spin{to{rotate:360deg}}\n.lay-row{display:flex;flex-direction:row}\n.gap-8{gap:8px}\n.w-20{width:20px}\n.h-20{height:20px}\n@media (prefers-reduced-motion:no-preference){.anim-spin{animation:buri-spin 1s linear infinite}}\n@media (prefers-reduced-motion:no-preference){.hover_anim-spin:hover{animation:buri-spin 1s linear infinite}}\n@media (min-width:48rem){\n@media (prefers-reduced-motion:no-preference){.md_anim-spin{animation:buri-spin 1s linear infinite}}\n}\n';
function __cmd_x_main_buri$main$withHost(host_0){
  const ctx_1=[host_0[0],host_0[1],host_0[10],host_0[11]];
  const self_4=$host_HostStdout_println(ctx_1[1],'spinning');
  let $t1;
  if(self_4[0]===0){
    $t1=0;
  }else if(self_4[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const events_8=ui_node$listen$1dndy7(void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0);
  const $t7=$share($k13);
  const children_9=$fromShared($k14,void 0);
  let $t3;
  if(children_9!==void 0){
    $t3=$share(children_9);
  }else if(children_9===void 0){
    $t3=[];
  }else{
    $abort('no arm matched');
  }
  return $ui_node_mount(ctx_1,ui_node$stack$1dndy7([$k5,[[[17,$k6,void 0,$t7,$t3,events_8]],ui_node$stack$1dndy7($k21)],void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0]),[]);
}
function __cmd_x_main_buri$main(){
  return __cmd_x_main_buri$main$withHost($k22);
}
function ui_node$stack$1dndy7(config_0){
  const events_1=ui_node$listen$1dndy7(config_0[7],config_0[8],config_0[9],config_0[10],config_0[11],config_0[12],config_0[13],config_0[14]);
  let $t1;
  const $t2=config_0[3];
  if($t2!==void 0){
    $t1=$t2;
  }else if($t2===void 0){
    $t1=$k23;
  }else{
    $abort('no arm matched');
  }
  const current_2=$t1;
  const focus_3=[config_0[5],config_0[6]];
  const $t3=config_0[2];
  if($t3!==void 0){
    return [[4,$t3,$share(config_0[0]),$share(config_0[1]),current_2,focus_3,events_1]];
  }else if($t3===void 0){
    let $t4;
    const $t5=config_0[4];
    if($t5!==void 0){
      $t4=$t5;
    }else if($t5===void 0){
      $t4=false;
    }else{
      $abort('no arm matched');
    }
    const decorative_6=$t4;
    return [[3,$share(config_0[0]),$share(config_0[1]),current_2,decorative_6,focus_3,events_1]];
  }else{
    $abort('no arm matched');
  }
}
function ui_node$listen$1dndy7(onHover_0,onFocus_1,onScroll_2,onKey_3,onPressOutside_4,onPointerDown_5,onPointerMove_6,onPointerUp_7){
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
