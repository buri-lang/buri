const $k0=[930,'bg-t_both_bg'];
const $k1=[$k0];
const $k2=[5,$k1];
const $k3=[$k2];
const $k4=[180,'lay-row'];
const $k5=[$k4];
const $k6=[5,$k5];
const $k7=[180,'lay-col'];
const $k8=[$k7];
const $k9=[5,$k8];
const $k10=[0,255,255,255];
$ui_sheet='.lay-col{display:flex;flex-direction:column}\n.lay-row{display:flex;flex-direction:row}\n.bg-t_both_bg{background-color:var(--both-bg)}\n';
$tree_declare_hook=$tree_declare;
$ui_theme_hook=$ui_theme_install;
function __cmd_x_main$main(){
  const ctx_0=[[],[],[],[]];
  const width_1=[$host_HostUi_signal(ctx_0[2],40)];
  $host_HostStdout_println(ctx_0[1],'both');
  const bindings_18=[[[2,['both','bg']],__cmd_x_main$light(0)]];
  return $ui_node_mount(ctx_0,[3,[$k9,[0,$k3]],[[3,[$k6,[0,[[4,scope_2=>[[24,[0,$ui_effect_Scope_read(scope_2,width_1[0])]]]]]]],[]]]],[[0,bindings_18]]);
}
function __cmd_x_main$light(t_0){
  return $k10;
}
