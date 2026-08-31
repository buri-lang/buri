const $k0=[0,2];
const $k1=[1,0,1n];
const $k2=[1,3,0n];
const $k3=[2];
const $k4=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  const text_2=__cmd_x_main_buri$render($k0)+' '+__cmd_x_main_buri$render($k1);
  const self_3=$host_HostStdout_println(ctx_0[1],text_2);
  let $t1;
  if(self_3[0]===0){
    $t1=0;
  }else if(self_3[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_7=__cmd_x_main_buri$render($k2)+' '+__cmd_x_main_buri$render($k3);
  const self_8=$host_HostStdout_println(ctx_0[1],text_7);
  let $t3;
  if(self_8[0]===0){
    $t3=0;
  }else if(self_8[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  return $k4;
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
