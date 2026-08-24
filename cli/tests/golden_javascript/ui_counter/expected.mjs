const $k0=[0,'doubled'];
const $k1=[1,0];
const $k2=[1,.5];
const $k3=[4,$k2];
const $k4=[0,6];
const $k5=[15,$k4];
const $k6=[0,240,240,245];
const $k7=[11,$k6];
const $k8=[$k3,$k5,$k7];
const $k9=[0,$k8];
const $k10=[$k9];
const $k11=[1,1];
function __cmd_x_main$main(){
  const ctx_0=[[],[],[],[]];
  $host_HostStdout_println(ctx_0[1],'mounted');
  const label_4='clicks';
  const count_5=[$host_HostUi_signal(ctx_0[2],0)];
  const children_20=[[5,[0,label_4],(c_6,e_7)=>$host_HostUi_write(c_6[2],count_5[0],(n_8=>n_8+1)($host_HostUi_read(c_6[2],count_5[0])))],__cmd_x_main$badge$u3rqgv([0,label_4],[1,count_5]),__cmd_x_main$badge$u3rqgv($k0,[2,c_9=>$ui_effect_Scope_read(c_9,count_5[0])*2])];
  return $ui_node_mount(ctx_0,[3,[$k1,[0,[]]],children_20],[]);
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
  return [3,[$k11,[0,$k10]],[[1,title_0],[1,content_9]]];
}
