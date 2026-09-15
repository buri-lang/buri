const $k0=[1200n,'lay-col'];
const $k1=[6200n,'bg-t_both_bg'];
const $k2=[$k0,$k1];
const $k3=[$k2];
const $k4=[5,$k3];
const $k5=[$k4];
const $k6=[1200n,'lay-row'];
const $k7=[$k6];
const $k8=[$k7];
const $k9=[5,$k8];
const $k10=[0,255n,255n,255n];
$ui_sheet='*,*::before,*::after{box-sizing:border-box}\n*,*::before,*::after{border-width:0}\n*{overflow-wrap:break-word}\n:where(body){margin:0}\n:where(div,nav,main,header,footer,aside,article,search,ul,li,hr,form,a,button){display:flex;flex-direction:column}\n.lay-col{display:flex;flex-direction:column}\n.lay-row{display:flex;flex-direction:row}\n.w-var{width:var(--buri-w)}\n.bg-t_both_bg{background-color:var(--both-bg)}\n';
$tree_declare_hook=$tree_declare;
$ui_theme_hook=$ui_theme_install;
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[],[],[]];
  const width_1=[$host_HostUi_signal(ctx_0[2],40n)];
  const self_7=$host_HostStdout_println(ctx_0[1],'both');
  let $t1;
  if(self_7[0]===0){
    $t1=0;
  }else if(self_7[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const $t5=ui_node$stack$u3rqgv([$k5,[ui_node$stack$u3rqgv([[$k9,[4,scope_2=>[[24,[0,$ui_effect_Scope_read(scope_2,width_1[0])]]]]],[],void 0,void 0,void 0,void 0,void 0,void 0])],void 0,void 0,void 0,void 0,void 0,void 0]);
  const bindings_16=[[[2,['both','bg']],__cmd_x_main_buri$light(0)]];
  return $ui_node_mount(ctx_0,$t5,[[[0,bindings_16]]]);
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
function __cmd_x_main_buri$light(t_0){
  return $k10;
}
