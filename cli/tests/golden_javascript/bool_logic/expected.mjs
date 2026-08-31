const $k0=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  const text_2=$str(__cmd_x_main_buri$inRange(5n,1n,10n))+' '+$str(__cmd_x_main_buri$inRange(50n,1n,10n));
  const self_3=$host_HostStdout_println(ctx_0[1],text_2);
  let $t1;
  if(self_3[0]===0){
    $t1=0;
  }else if(self_3[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_11=$str(true)+' '+$str(true);
  const self_12=$host_HostStdout_println(ctx_0[1],text_11);
  let $t3;
  if(self_12[0]===0){
    $t3=0;
  }else if(self_12[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  const text_16=String(1n)+' '+String(2n);
  const self_17=$host_HostStdout_println(ctx_0[1],text_16);
  let $t5;
  if(self_17[0]===0){
    $t5=0;
  }else if(self_17[0]===1){
    $t5=0;
  }else{
    $abort('no arm matched');
  }
  return $k0;
}
function __cmd_x_main_buri$inRange(n_0,lo_1,hi_2){
  return !(n_0<lo_1)&&!(n_0>hi_2);
}
