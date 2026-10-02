const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const text_3=$str(__cmd_x_main_buri$inRange(5n,1n,10n))+' '+$str(__cmd_x_main_buri$inRange(50n,1n,10n));
  const self_4=$host_HostStdout_println(ctx_1[1],text_3);
  let $t1;
  if(self_4[0]===0){
    $t1=0;
  }else if(self_4[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_12=$str(true)+' '+$str(true);
  const self_13=$host_HostStdout_println(ctx_1[1],text_12);
  let $t3;
  if(self_13[0]===0){
    $t3=0;
  }else if(self_13[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  const text_17=String(1n)+' '+String(2n);
  const self_18=$host_HostStdout_println(ctx_1[1],text_17);
  let $t5;
  if(self_18[0]===0){
    $t5=0;
  }else if(self_18[0]===1){
    $t5=0;
  }else{
    $abort('no arm matched');
  }
  return $k1;
}
function __cmd_x_main_buri$inRange(n_0,lo_1,hi_2){
  return !(n_0<lo_1)&&!(n_0>hi_2);
}
