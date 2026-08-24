const $k0=[0,'doubled'];
const $k1=[180,'lay-col'];
const $k2=[$k1];
const $k3=[5,$k2];
const $k4=[630,'px-r0_5'];
const $k5=[1080,'r-6'];
const $k6=[930,'bg-f0f0f5'];
const $k7=[960,'fg-18181b'];
const $k8=[935,'hover_bg-18181b'];
const $k9=[965,'hover_fg-f0f0f5'];
const $k10=[$k4,$k5,$k6,$k7,$k8,$k9];
const $k11=[5,$k10];
const $k12=[$k11];
const $k13=[180,'lay-row'];
const $k14=[$k13];
const $k15=[5,$k14];
$ui_sheet='.lay-col{display:flex;flex-direction:column}\n.lay-row{display:flex;flex-direction:row}\n.px-r0_5{padding-inline:0.5rem}\n.bg-f0f0f5{background-color:rgb(240,240,245)}\n.hover_bg-18181b:hover{background-color:rgb(24,24,27)}\n.fg-18181b{color:rgb(24,24,27)}\n.hover_fg-f0f0f5:hover{color:rgb(240,240,245)}\n.r-6{border-radius:6px}\n';
function __cmd_x_main$main(){
  const ctx_0=[[],[],[],[]];
  $host_HostStdout_println(ctx_0[1],'mounted');
  const label_4='clicks';
  const count_5=[$host_HostUi_signal(ctx_0[2],0)];
  const children_20=[[5,[0,label_4],(c_6,e_7)=>$host_HostUi_write(c_6[2],count_5[0],(n_8=>n_8+1)($host_HostUi_read(c_6[2],count_5[0])))],__cmd_x_main$badge$u3rqgv([0,label_4],[1,count_5]),__cmd_x_main$badge$u3rqgv($k0,[2,c_9=>$ui_effect_Scope_read(c_9,count_5[0])*2])];
  return $ui_node_mount(ctx_0,[3,[$k3,[0,[]]],children_20],[]);
}
function __cmd_x_main$badge$u3rqgv(title_0,count_1){
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
  return [3,[$k15,[0,$k12]],[[1,title_0],[1,content_9]]];
}
