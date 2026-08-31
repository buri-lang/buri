const $k0=[930n,'bg-dc2626'];
const $k1=[$k0];
const $k2=[$k1];
const $k3=[5,$k2];
const $k4=[$k3];
const $k5=[930n,'bg-16a34a'];
const $k6=[$k5];
const $k7=[$k6];
const $k8=[5,$k7];
const $k9=[$k8];
const $k10=[180n,'lay-col'];
const $k11=[$k10];
const $k12=[$k11];
const $k13=[5,$k12];
const $k14=[180n,'lay-row'];
const $k15=[$k14];
const $k16=[$k15];
const $k17=[5,$k16];
$ui_sheet='.lay-col{display:flex;flex-direction:column}\n.lay-row{display:flex;flex-direction:row}\n.bg-16a34a{background-color:rgb(22,163,74)}\n.bg-dc2626{background-color:rgb(220,38,38)}\n';
$tree_declare_hook=$tree_declare;
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[],[],[]];
  const lit_1=[$host_HostUi_signal(ctx_0[2],false)];
  const width_2=[$host_HostUi_signal(ctx_0[2],120n)];
  const self_11=$host_HostStdout_println(ctx_0[1],'dynamic');
  let $t1;
  if(self_11[0]===0){
    $t1=0;
  }else if(self_11[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const $t3=ui_node$row$u3rqgv([[3,[1,lit_1],$k4,$k9]],[]);
  const $t4=ui_node$row$u3rqgv([[4,scope_3=>[[24,[0,$ui_effect_Scope_read(scope_3,width_2[0])]]]]],[]);
  const children_19=[$t3,$t4,ui_node$row$u3rqgv([[12,$host_HostWatch_read(ctx_0[3],width_2[0])]],[])];
  return $ui_node_mount(ctx_0,[[3,[$k13,[0,[]]],children_19]],[]);
}
function ui_node$row$u3rqgv(styles_0,children_1){
  return [[3,[$k17,[0,styles_0]],children_1]];
}
