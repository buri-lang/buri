const $k0=[1200n,'lay-col'];
const $k1=[$k0];
const $k2=[$k1];
const $k3=[5,$k2];
const $k4=[$k3];
const $k5=[1200n,'lay-row'];
const $k6=[$k5];
const $k7=[$k6];
const $k8=[5,$k7];
const $k9=[6200n,'bg-dc2626'];
const $k10=[$k9];
const $k11=[$k10];
const $k12=[5,$k11];
const $k13=[$k12];
const $k14=[6200n,'bg-16a34a'];
const $k15=[$k14];
const $k16=[$k15];
const $k17=[5,$k16];
const $k18=[$k17];
const $k19=[[],[],[],[],[],[],[],[],[],[],[],[],[]];
$ui_sheet='*,*::before,*::after{box-sizing:border-box}\n*,*::before,*::after{border-width:0}\n*{overflow-wrap:break-word}\n:where(body){margin:0}\n:where(div,nav,main,header,footer,aside,article,search,ul,li,hr,form,a,button){display:flex;flex-direction:column}\n.lay-col{display:flex;flex-direction:column}\n.lay-row{display:flex;flex-direction:row}\n.grow-var{flex-grow:var(--buri-grow)}\n.w-var{width:var(--buri-w)}\n.bg-16a34a{background-color:rgb(22,163,74)}\n.bg-dc2626{background-color:rgb(220,38,38)}\n';
$tree_declare_hook=$tree_declare;
function __cmd_x_main_buri$main$withHost(host_0){
  const ctx_1=[host_0[0],host_0[1],host_0[10],host_0[11]];
  const lit_2=[$host_HostUi_signal(ctx_1[2],false)];
  const width_3=[$host_HostUi_signal(ctx_1[2],120n)];
  const self_11=$host_HostStdout_println(ctx_1[1],'dynamic');
  let $t1;
  if(self_11[0]===0){
    $t1=0;
  }else if(self_11[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const $t3=ui_node$stack$u3rqgv([[$k8,[3,[1,lit_2],$k13,$k18]],[],void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0]);
  const $t4=ui_node$stack$u3rqgv([[$k8,[4,scope_4=>[[24,[0,$effect_Scope_read(scope_4,width_3[0])]]]]],[],void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0]);
  return $ui_node_mount(ctx_1,ui_node$stack$u3rqgv([$k4,[$t3,$t4,ui_node$stack$u3rqgv([[$k8,[12,$host_HostWatch_read(ctx_1[3],width_3[0])]],[],void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0])],void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0]),[]);
}
function __cmd_x_main_buri$main(){
  return __cmd_x_main_buri$main$withHost($k19);
}
function ui_node$stack$u3rqgv(config_0){
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
const $buri$program={main:async ()=>{
  const $r=await __cmd_x_main_buri$main();
  $host.flush();
  return void $crossThrow($r);
}};
const $buri$ui={signal:(v)=>$host_HostUi_signal(null,v),write:(id,v)=>{$host_HostUi_write(null,id,v)}};
var $buri$host;
const $buri$platform = await (async () => {
$buri$host = () => ({ HostLocation: HostLocation });
// web's entry adapter. It starts the page's `main` when the module loads, and
// implements `HostLocation`, the address bar, over the browser's own
// `location` and `history`.
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

// `.Err(msg)` arrives as a thrown `Error`, and the message goes to the console.
// Under a host with a process, such as bun, the process exits 1.
await main().catch((e) => {
  console.error(e && e.message ? e.message : String(e));
  if (typeof process !== "undefined") process.exitCode = 1;
});
return { "HostLocation": HostLocation };
})();
