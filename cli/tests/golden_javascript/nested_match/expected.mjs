const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[0,2];
const $k2=[1,0,1n];
const $k3=[1,3,0n];
const $k4=[2];
const $k5=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const text_3=__cmd_x_main_buri$render($k1)+' '+__cmd_x_main_buri$render($k2);
  const self_4=$host_HostStdout_println(ctx_1[1],text_3);
  let $t1;
  if(self_4[0]===0){
    $t1=0;
  }else if(self_4[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_8=__cmd_x_main_buri$render($k3)+' '+__cmd_x_main_buri$render($k4);
  const self_9=$host_HostStdout_println(ctx_1[1],text_8);
  let $t3;
  if(self_9[0]===0){
    $t3=0;
  }else if(self_9[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  return $k5;
}
function __cmd_x_main_buri$render(o_0){
  if(o_0[0]===0&&o_0[1]===0){
    return '1A';
  }else if(o_0[0]===0&&o_0[1]===1){
    return '1B';
  }else if(o_0[0]===0&&o_0[1]===2){
    return '1C';
  }else if(o_0[0]===0&&o_0[1]===3){
    return '1D';
  }else if(o_0[0]===1&&o_0[1]===0){
    return o_0[2]>0n?'2A+':'2A-';
  }else if(o_0[0]===1){
    return '2*';
  }else if(o_0[0]===2){
    return '3';
  }else{
    $abort('no arm matched');
  }
}
