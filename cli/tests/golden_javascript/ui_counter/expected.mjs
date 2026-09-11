const $k0=[0,false];
const $k1=[0,'doubled'];
const $k2=[1200n,'lay-col'];
const $k3=[$k2];
const $k4=[$k3];
const $k5=[5,$k4];
const $k6=[4200n,'px-r0_5'];
const $k7=[7400n,'r-6'];
const $k8=[6200n,'bg-f0f0f5'];
const $k9=[6400n,'fg-18181b'];
const $k10=[6205n,'hover_bg-18181b'];
const $k11=[6405n,'hover_fg-f0f0f5'];
const $k12=[$k6,$k7,$k8,$k9,$k10,$k11];
const $k13=[$k12];
const $k14=[5,$k13];
const $k15=[$k14];
const $k16=[1200n,'lay-row'];
const $k17=[$k16];
const $k18=[$k17];
const $k19=[5,$k18];
$ui_sheet='*,*::before,*::after{box-sizing:border-box}\n:where(body){margin:0}\n:where(div,nav,main,header,footer,aside,article,search,ul,li,hr,form,a,button){display:flex;flex-direction:column}\n:where(button){appearance:none;background:none;border:0;padding:0;font:inherit;color:inherit}\n.lay-col{display:flex;flex-direction:column}\n.lay-row{display:flex;flex-direction:row}\n.px-r0_5{padding-inline:0.5rem}\n.bg-f0f0f5{background-color:rgb(240,240,245)}\n.hover_bg-18181b:hover{background-color:rgb(24,24,27)}\n.fg-18181b{color:rgb(24,24,27)}\n.hover_fg-f0f0f5:hover{color:rgb(240,240,245)}\n.r-6{border-radius:6px}\n';
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[],[],[]];
  const self_3=$host_HostStdout_println(ctx_0[1],'mounted');
  let $t1;
  if(self_3[0]===0){
    $t1=0;
  }else if(self_3[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const label_7='clicks';
  const count_8=[$host_HostUi_signal(ctx_0[2],0n)];
  const children_26=[[[5,[0,label_7],[],[],(c_9,e_10)=>$host_HostUi_write(c_9[2],count_8[0],(n_11=>n_11+1n)($host_HostUi_read(c_9[2],count_8[0]))),$k0]],__cmd_x_main_buri$badge$u3rqgv([0,label_7],[1,count_8]),__cmd_x_main_buri$badge$u3rqgv($k1,[2,c_12=>$ui_effect_Scope_read(c_12,count_8[0])*2n])];
  return $ui_node_mount(ctx_0,[[3,[$k5,[0,[]]],children_26]],[]);
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
  return [[3,[$k19,[0,$k15]],[[[1,title_0]],[[1,content_9]]]]];
}
