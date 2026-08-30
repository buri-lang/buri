const $k0=[0,'doubled'];
const $k1=[180n,'lay-col'];
const $k2=[$k1];
const $k3=[$k2];
const $k4=[5,$k3];
const $k5=[630n,'px-r0_5'];
const $k6=[1080n,'r-6'];
const $k7=[930n,'bg-f0f0f5'];
const $k8=[960n,'fg-18181b'];
const $k9=[935n,'hover_bg-18181b'];
const $k10=[965n,'hover_fg-f0f0f5'];
const $k11=[$k5,$k6,$k7,$k8,$k9,$k10];
const $k12=[$k11];
const $k13=[5,$k12];
const $k14=[$k13];
const $k15=[180n,'lay-row'];
const $k16=[$k15];
const $k17=[$k16];
const $k18=[5,$k17];
$ui_sheet='.lay-col{display:flex;flex-direction:column}\n.lay-row{display:flex;flex-direction:row}\n.px-r0_5{padding-inline:0.5rem}\n.bg-f0f0f5{background-color:rgb(240,240,245)}\n.hover_bg-18181b:hover{background-color:rgb(24,24,27)}\n.fg-18181b{color:rgb(24,24,27)}\n.hover_fg-f0f0f5:hover{color:rgb(240,240,245)}\n.r-6{border-radius:6px}\n';
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[],[],[]];
  $host_HostStdout_println(ctx_0[1],'mounted');
  const label_4='clicks';
  const count_5=[$host_HostUi_signal(ctx_0[2],0n)];
  const children_20=[[[5,[0,label_4],(c_6,e_7)=>$host_HostUi_write(c_6[2],count_5[0],(n_8=>n_8+1n)($host_HostUi_read(c_6[2],count_5[0])))]],__cmd_x_main_buri$badge$u3rqgv([0,label_4],[1,count_5]),__cmd_x_main_buri$badge$u3rqgv($k0,[2,c_9=>$ui_effect_Scope_read(c_9,count_5[0])*2n])];
  return $ui_node_mount(ctx_0,[[3,[$k4,[0,[]]],children_20]],[]);
}
function __cmd_x_main_buri$badge$u3rqgv(title_0,count_1){
  const content_9=[2,c_2=>{
    let $t1;
    if(count_1[0]===0){
      $t1=count_1[1];
    }else if(count_1[0]===1){
      $t1=$ui_effect_Scope_read(c_2,count_1[1][0]);
    }else if(count_1[0]===2){
      $t1=count_1[1](c_2);
    }else{
      $abort('no arm matched');
    }
    return String($t1);
  }];
  return [[3,[$k18,[0,$k14]],[[[1,title_0]],[[1,content_9]]]]];
}
