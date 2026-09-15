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
$ui_sheet='*,*::before,*::after{box-sizing:border-box}\n*,*::before,*::after{border-width:0}\n*{overflow-wrap:break-word}\n:where(body){margin:0}\n:where(div,nav,main,header,footer,aside,article,search,ul,li,hr,form,a,button){display:flex;flex-direction:column}\n.lay-col{display:flex;flex-direction:column}\n.lay-row{display:flex;flex-direction:row}\n.grow-var{flex-grow:var(--buri-grow)}\n.w-var{width:var(--buri-w)}\n.bg-16a34a{background-color:rgb(22,163,74)}\n.bg-dc2626{background-color:rgb(220,38,38)}\n';
$tree_declare_hook=$tree_declare;
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[],[],[]];
  const lit_1=[$host_HostUi_signal(ctx_0[2],false)];
  const width_2=[$host_HostUi_signal(ctx_0[2],120n)];
  const self_10=$host_HostStdout_println(ctx_0[1],'dynamic');
  let $t1;
  if(self_10[0]===0){
    $t1=0;
  }else if(self_10[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const $t3=ui_node$stack$u3rqgv([[$k8,[3,[1,lit_1],$k13,$k18]],[],void 0,void 0,void 0,void 0,void 0,void 0]);
  const $t4=ui_node$stack$u3rqgv([[$k8,[4,scope_3=>[[24,[0,$ui_effect_Scope_read(scope_3,width_2[0])]]]]],[],void 0,void 0,void 0,void 0,void 0,void 0]);
  return $ui_node_mount(ctx_0,ui_node$stack$u3rqgv([$k4,[$t3,$t4,ui_node$stack$u3rqgv([[$k8,[12,$host_HostWatch_read(ctx_0[3],width_2[0])]],[],void 0,void 0,void 0,void 0,void 0,void 0])],void 0,void 0,void 0,void 0,void 0,void 0]),[]);
}
function ui_node$stack$u3rqgv(config_0){
  const events_1=[config_0[3],config_0[4],config_0[5],config_0[6],config_0[7]];
  const $t1=config_0[2];
  if($t1!==void 0){
    return [[4,$t1,$share(config_0[0]),$share(config_0[1]),events_1]];
  }else if($t1===void 0){
    return [[3,$share(config_0[0]),$share(config_0[1]),events_1]];
  }else{
    $abort('no arm matched');
  }
}
