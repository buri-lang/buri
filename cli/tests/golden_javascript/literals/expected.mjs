const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k4=[2n,3n,5n,7n,11n];
const $k5=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const text_5=String(6n)+' '+'default';
  const self_6=$host_HostStdout_println(ctx_1[1],text_5);
  let $t1;
  if(self_6[0]===0){
    $t1=0;
  }else if(self_6[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_10=String(1n)+' '+String(3n);
  const self_11=$host_HostStdout_println(ctx_1[1],text_10);
  let $t7;
  if(self_11[0]===0){
    $t7=0;
  }else if(self_11[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  const text_18=String(0n)+' '+String($list_length($k4))+' '+String($list_fold($k4,(acc_15,x_16)=>acc_15+x_16,0n));
  const self_19=$host_HostStdout_println(ctx_1[1],text_18);
  let $t9;
  if(self_19[0]===0){
    $t9=0;
  }else if(self_19[0]===1){
    $t9=0;
  }else{
    $abort('no arm matched');
  }
  return $k5;
}
