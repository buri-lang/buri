const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[]];
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
  const $t3=ui_node$stack$u3rqgv([$k6,[ui_node$stack$u3rqgv([[$k10,[4,scope_3=>[[24,[0,$ui_effect_Scope_read(scope_3,width_2[0])]]]]],[],void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0])],void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0]);
  const bindings_14=[[__cmd_x_main_buri$Token_color(0),__cmd_x_main_buri$light(0)]];
  return $ui_node_mount(ctx_1,$t3,[[[0,bindings_14]]]);
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
function __cmd_x_main_buri$light(t_0){
  return $k11;
}
function __cmd_x_main_buri$Token_color(self_0){
  return [2,['both','bg']];
}
