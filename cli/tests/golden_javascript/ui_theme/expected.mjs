const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[1200n,'lay-col'];
const $k2=[6200n,'bg-t_app_bg'];
const $k3=[6400n,'fg-t_app_fg'];
const $k4=[$k1,$k2,$k3];
const $k5=[$k4];
const $k6=[5,$k5];
const $k7=[$k6];
const $k8=[0,220n,38n,38n];
const $k9=[0,24n,24n,27n];
const $k10=[0,240n,240n,245n];
const $k11=[0,255n,255n,255n];
const $k12=[0,false];
const $k13=[1200n,'lay-row'];
const $k14=[4200n,'px-r0_5'];
const $k15=[7400n,'r-6'];
const $k16=[6200n,'bg-t_cardlib_surface'];
const $k17=[6400n,'fg-t_cardlib_onSurface'];
const $k18=[6205n,'hover_bg-t_cardlib_danger'];
const $k19=[$k13,$k14,$k15,$k16,$k17,$k18];
const $k20=[$k19];
const $k21=[5,$k20];
const $k22=[$k21];
$ui_sheet='*,*::before,*::after{box-sizing:border-box}\n*,*::before,*::after{border-width:0}\n*{overflow-wrap:break-word}\n:where(body){margin:0}\n:where(div,nav,main,header,footer,aside,article,search,ul,li,hr,form,a,button){display:flex;flex-direction:column}\n:where(h1,h2,h3,h4,h5,h6){font-size:inherit;font-weight:inherit;margin:0}\n:where(button){appearance:none;background:none;border:0;padding:0;font:inherit;color:inherit}\n.lay-col{display:flex;flex-direction:column}\n.lay-row{display:flex;flex-direction:row}\n.px-r0_5{padding-inline:0.5rem}\n.bg-t_app_bg{background-color:var(--app-bg)}\n.bg-t_cardlib_surface{background-color:var(--cardlib-surface)}\n.hover_bg-t_cardlib_danger:hover{background-color:var(--cardlib-danger)}\n.fg-t_app_fg{color:var(--app-fg)}\n.fg-t_cardlib_onSurface{color:var(--cardlib-onSurface)}\n.r-6{border-radius:6px}\n';
$ui_theme_hook=$ui_theme_install;
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1],$k0[10],$k0[11]];
  const dark_2=[$host_HostUi_signal(ctx_1[2],false)];
  const self_7=$host_HostStdout_println(ctx_1[1],'mounted');
  let $t1;
  if(self_7[0]===0){
    $t1=0;
  }else if(self_7[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const label_11='clicks';
  const count_12=ui_signal$signal$hlck94(ctx_1,0n);
  const $t4=ui_node$stack$b1px4q([$k7,[ui_node$button$b1px4q([[0,label_11],[],void 0,c_13=>ui_signal$Signal_update$5m6v0z(count_12,c_13,n_14=>n_14+1n),void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0]),__cmd_x_main_buri$badge$b1px4q([0,label_11],[1,count_12])],void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0]);
  const $t3=ui_theme$themed([[__cmd_x_main_buri$Card_color(0),__cmd_x_main_buri$cardTheme(0)],[__cmd_x_main_buri$Card_color(1),__cmd_x_main_buri$cardTheme(1)],[__cmd_x_main_buri$Card_color(2),__cmd_x_main_buri$cardTheme(2)]]);
  const whenTrue_17=__cmd_x_main_buri$appThemed(__cmd_x_main_buri$night);
  const whenFalse_18=__cmd_x_main_buri$appThemed(__cmd_x_main_buri$day);
  return $ui_node_mount(ctx_1,$t4,[$t3,[[1,[1,dark_2],[whenTrue_17],[whenFalse_18]]]]);
}
function __cmd_x_main_buri$cardTheme(t_0){
  switch(t_0){
    case 0:
      {
        return __cmd_x_main_buri$App_color(0);
      }
    case 1:
      {
        return __cmd_x_main_buri$App_color(1);
      }
    case 2:
      {
        return $k8;
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
function __cmd_x_main_buri$night(t_0){
  if(t_0===0){
    return $k9;
  }else if(t_0===1){
    return $k10;
  }else{
    $abort('no arm matched');
  }
}
function __cmd_x_main_buri$appThemed(f_0){
  const bindings_1=[[__cmd_x_main_buri$App_color(0),f_0(0)],[__cmd_x_main_buri$App_color(1),f_0(1)]];
  return [[0,bindings_1]];
}
function __cmd_x_main_buri$day(t_0){
  if(t_0===0){
    return $k11;
  }else if(t_0===1){
    return $k9;
  }else{
    $abort('no arm matched');
  }
}
function __cmd_x_main_buri$App_color(self_0){
  if(self_0===0){
    return [2,['app','bg']];
  }else if(self_0===1){
    return [2,['app','fg']];
  }else{
    $abort('no arm matched');
  }
}
function ui_theme$themed(bindings_0){
  return [[0,bindings_0]];
}
function __cmd_x_main_buri$Card_color(self_0){
  switch(self_0){
    case 0:
      {
        return [2,['cardlib','surface']];
      }
    case 1:
      {
        return [2,['cardlib','onSurface']];
      }
    case 2:
      {
        return [2,['cardlib','danger']];
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
function ui_signal$signal$hlck94(ctx_0,initial_1){
  return [$host_HostUi_signal(ctx_0[2],initial_1)];
}
function ui_signal$Signal_update$5m6v0z(self_0,ctx_1,f_2){
  return $host_HostUi_write(ctx_1[2],self_0[0],f_2($host_HostUi_read(ctx_1[2],self_0[0])));
}
function ui_node$button$b1px4q(config_0){
  const events_1=ui_node$listen$b1px4q(config_0[8],config_0[9],config_0[10],config_0[11],config_0[12],config_0[13],config_0[14],config_0[15]);
  const $t1=config_0[4];
  if($t1!==void 0&&$t1===1){
    const $t5=$share(config_0[1]);
    let $t2;
    const $t3=config_0[5];
    if($t3!==void 0){
      $t2=$t3;
    }else if($t3===void 0){
      $t2=$k12;
    }else{
      $abort('no arm matched');
    }
    return [[14,config_0[0],$t5,$t2,events_1]];
  }else{
    const $t9=$share(config_0[1]);
    const children_4=$share(config_0[2]);
    let $t6;
    if(children_4!==void 0){
      $t6=$share(children_4);
    }else if(children_4===void 0){
      $t6=[];
    }else{
      $abort('no arm matched');
    }
    let $t10;
    const $t11=config_0[3];
    if($t11!==void 0){
      $t10=(c_8,_event_9)=>$t11(c_8);
    }else if($t11===void 0){
      $t10=(_c_10,_event_11)=>0;
    }else{
      $abort('no arm matched');
    }
    let $t12;
    const $t13=config_0[5];
    if($t13!==void 0){
      $t12=$t13;
    }else if($t13===void 0){
      $t12=$k12;
    }else{
      $abort('no arm matched');
    }
    let $t14;
    const $t15=config_0[6];
    if($t15!==void 0){
      $t14=$t15;
    }else if($t15===void 0){
      $t14=$k12;
    }else{
      $abort('no arm matched');
    }
    let $t16;
    const $t17=config_0[7];
    if($t17!==void 0){
      $t16=$t17;
    }else if($t17===void 0){
      $t16=$k12;
    }else{
      $abort('no arm matched');
    }
    return [[5,config_0[0],$t9,$t6,$t10,$t12,$t14,$t16,events_1]];
  }
}
function __cmd_x_main_buri$badge$b1px4q(title_0,count_1){
  return ui_node$stack$b1px4q([$k22,[ui_node$text$b1px4q([title_0,void 0,void 0]),ui_node$text$b1px4q([[2,c_2=>{
    let $t1;
    if(count_1[0]===0){
      $t1=count_1[1];
    }else if(count_1[0]===1){
      $t1=$effect_Scope_read(c_2,count_1[1][0]);
    }else if(count_1[0]===2){
      $t1=count_1[1](c_2);
    }else{
      $abort('no arm matched');
    }
    return String($t1);
  }],void 0,void 0])],void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0]);
}
function ui_node$stack$b1px4q(config_0){
  const events_1=ui_node$listen$b1px4q(config_0[3],config_0[4],config_0[5],config_0[6],config_0[7],config_0[8],config_0[9],config_0[10]);
  const $t1=config_0[2];
  if($t1!==void 0){
    return [[4,$t1,$share(config_0[0]),$share(config_0[1]),events_1]];
  }else if($t1===void 0){
    return [[3,$share(config_0[0]),$share(config_0[1]),events_1]];
  }else{
    $abort('no arm matched');
  }
}
function ui_node$listen$b1px4q(onHover_0,onFocus_1,onScroll_2,onKey_3,onPressOutside_4,onPointerDown_5,onPointerMove_6,onPointerUp_7){
  return [onHover_0,onFocus_1,onScroll_2,onKey_3,onPressOutside_4,onPointerDown_5,onPointerMove_6,onPointerUp_7];
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
