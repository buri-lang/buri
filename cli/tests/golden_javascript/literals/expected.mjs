const $k3=[2n,3n,5n,7n,11n];
const $k4=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  const text_4=String(6n)+' '+'default';
  const self_5=$host_HostStdout_println(ctx_0[1],text_4);
  let $t1;
  if(self_5[0]===0){
    $t1=0;
  }else if(self_5[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_9=String(1n)+' '+String(3n);
  const self_10=$host_HostStdout_println(ctx_0[1],text_9);
  let $t7;
  if(self_10[0]===0){
    $t7=0;
  }else if(self_10[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  const text_17=String(0n)+' '+String($list_len($k3))+' '+String($list_fold($k3,(acc_14,x_15)=>acc_14+x_15,0n));
  const self_18=$host_HostStdout_println(ctx_0[1],text_17);
  let $t9;
  if(self_18[0]===0){
    $t9=0;
  }else if(self_18[0]===1){
    $t9=0;
  }else{
    $abort('no arm matched');
  }
  return $k4;
}
