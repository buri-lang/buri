const $k0=[930,'bg-t_both_bg'];
const $k1=[$k0];
const $k2=[$k1];
const $k3=[5,$k2];
const $k4=[$k3];
const $k5=[180,'lay-row'];
const $k6=[$k5];
const $k7=[$k6];
const $k8=[5,$k7];
const $k9=[180,'lay-col'];
const $k10=[$k9];
const $k11=[$k10];
const $k12=[5,$k11];
const $k13=[0,255,255,255];
$ui_sheet='.lay-col{display:flex;flex-direction:column}\n.lay-row{display:flex;flex-direction:row}\n.bg-t_both_bg{background-color:var(--both-bg)}\n';
$tree_declare_hook=$tree_declare;
$ui_theme_hook=$ui_theme_install;
function __cmd_x_main$main(){
  const ctx_0=[[],[],[],[]];
  const width_1=[$host_HostUi_signal(ctx_0[2],40)];
  $host_HostStdout_println(ctx_0[1],'both');
  const bindings_18=[[[2,['both','bg']],__cmd_x_main$light(0)]];
  return $ui_node_mount(ctx_0,[[3,[$k12,[0,$k4]],[[[3,[$k8,[0,[[4,scope_2=>[[24,[0,$ui_effect_Scope_read(scope_2,width_1[0])]]]]]]],[]]]]]],[[[0,bindings_18]]]);
}
function __cmd_x_main$light(t_0){
  return $k13;
}
