const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[1200n,'lay-col'];
const $k2=[4000n,'p-r1'];
const $k3=[3400n,'gap-8'];
const $k4=[4002n,'md_p-r2'];
const $k5=[3402n,'md_gap-16'];
const $k6=[5403n,'lg_maxw-r64'];
const $k7=[7406n,'sm_hover_r-4'];
const $k8=[$k1,$k2,$k3,$k4,$k5,$k6,$k7];
const $k9=[$k8];
const $k10=[5,$k9];
const $k11=[$k10];
const $k12=[1200n,'lay-row'];
const $k13=[4200n,'px-r0_5'];
const $k14=[7400n,'r-6'];
const $k15=[6200n,'bg-f0f0f5'];
const $k16=[6400n,'fg-18181b'];
const $k17=[6205n,'hover_bg-18181b'];
const $k18=[6405n,'hover_fg-f0f0f5'];
const $k19=[$k12,$k13,$k14,$k15,$k16,$k17,$k18];
const $k20=[$k19];
const $k21=[5,$k20];
const $k22=[$k21];
const $k23=[2400n,'grow-1'];
const $k24=[$k12,$k13,$k23];
const $k25=[$k24];
const $k26=[5,$k25];
const $k27=[$k26];
const $k28=[$k27,[],void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0];
const $k30=[2600n,'shrink-0'];
const $k31=[$k30];
const $k32=[$k31];
const $k33=[5,$k32];
const $k34=[$k23];
const $k35=[$k34];
const $k36=[5,$k35];
const $k37=[$k36];
const $k38=[void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0];
$ui_sheet='*,*::before,*::after{box-sizing:border-box}\n*,*::before,*::after{border-width:0}\n*{overflow-wrap:break-word}\n:where(body){margin:0}\n:where(div,nav,main,header,footer,aside,article,search,ul,li,hr,form,a,button){display:flex;flex-direction:column}\n:where(h1,h2,h3,h4,h5,h6){font-size:inherit;font-weight:inherit;margin:0}\n.lay-col{display:flex;flex-direction:column}\n.lay-row{display:flex;flex-direction:row}\n.grow-1{flex-grow:1}\n.shrink-0{flex-shrink:0}\n.gap-8{gap:8px}\n.p-r1{padding:1rem}\n.px-r0_5{padding-inline:0.5rem}\n.w-var{width:var(--buri-w)}\n.h-var{height:var(--buri-h)}\n.bg-f0f0f5{background-color:rgb(240,240,245)}\n.hover_bg-18181b:hover{background-color:rgb(24,24,27)}\n.fg-18181b{color:rgb(24,24,27)}\n.hover_fg-f0f0f5:hover{color:rgb(240,240,245)}\n.r-6{border-radius:6px}\n@media (min-width:40rem){\n.sm_hover_r-4:hover{border-radius:4px}\n}\n@media (min-width:48rem){\n.md_gap-16{gap:16px}\n.md_p-r2{padding:2rem}\n}\n@media (min-width:64rem){\n.lg_maxw-r64{max-width:64rem}\n}\n';
$tree_declare_hook=$tree_declare;
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1],$k0[10],$k0[11]];
  const self_4=$host_HostStdout_println(ctx_1[1],'styled');
  let $t1;
  if(self_4[0]===0){
    $t1=0;
  }else if(self_4[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const $t5=ui_node$stack$b1px4q([$k22,[ui_node$text$b1px4q([[0,'one'],void 0,void 0])],void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0]);
  const $t6=ui_node$stack$b1px4q($k28);
  let $t3;
  const $t4=void 0;
  if($t4!==void 0){
    $t3=[[3,[[24,$t4],[25,$t4],$k33],[],ui_node$noEvents$b1px4q()]];
  }else if($t4===void 0){
    $t3=[[3,$k37,[],ui_node$noEvents$b1px4q()]];
  }else{
    $abort('no arm matched');
  }
  return $ui_node_mount(ctx_1,ui_node$stack$b1px4q([$k11,[$t5,$t6,$t3],void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0]),[]);
}
function ui_node$stack$b1px4q(config_0){
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
function ui_node$noEvents$b1px4q(){
  return $k38;
}
function ui_node$text$b1px4q(config_0){
  const $t1=config_0[1];
  if($t1!==void 0){
    const styles_2=$share(config_0[2]);
    let $t2;
    if(styles_2!==void 0){
      $t2=$share(styles_2);
    }else if(styles_2===void 0){
      $t2=[];
    }else{
      $abort('no arm matched');
    }
    return [[2,$t1,$t2,config_0[0]]];
  }else if($t1===void 0){
    return [[1,config_0[0]]];
  }else{
    $abort('no arm matched');
  }
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
