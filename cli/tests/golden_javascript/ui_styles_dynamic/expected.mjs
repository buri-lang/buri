const $k0=[930,'bg-dc2626'];
const $k1=[$k0];
const $k2=[5,$k1];
const $k3=[$k2];
const $k4=[930,'bg-16a34a'];
const $k5=[$k4];
const $k6=[5,$k5];
const $k7=[$k6];
const $k8=[180,'lay-col'];
const $k9=[$k8];
const $k10=[5,$k9];
const $k11=[180,'lay-row'];
const $k12=[$k11];
const $k13=[5,$k12];
$ui_sheet='.lay-col{display:flex;flex-direction:column}\n.lay-row{display:flex;flex-direction:row}\n.bg-16a34a{background-color:rgb(22,163,74)}\n.bg-dc2626{background-color:rgb(220,38,38)}\n';
$tree_declare_hook=$tree_declare;
function __cmd_x_main$main(){
  const ctx_0=[[],[],[],[]];
  const lit_1=[$host_HostUi_signal(ctx_0[2],false)];
  const width_2=[$host_HostUi_signal(ctx_0[2],120)];
  $host_HostStdout_println(ctx_0[1],'dynamic');
  const $t1=ui_node$row$u3rqgv([[3,[1,lit_1],$k3,$k7]],[]);
  const $t2=ui_node$row$u3rqgv([[4,scope_3=>[[24,[0,$ui_effect_Scope_read(scope_3,width_2[0])]]]]],[]);
  const children_16=[$t1,$t2,ui_node$row$u3rqgv([[12,$host_HostWatch_read(ctx_0[3],width_2[0])]],[])];
  return $ui_node_mount(ctx_0,[3,[$k10,[0,[]]],children_16],[]);
}
function ui_node$row$u3rqgv(styles_0,children_1){
  return [3,[$k13,[0,styles_0]],children_1];
}
