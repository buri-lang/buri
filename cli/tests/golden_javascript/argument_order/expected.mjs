const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const p_2=1;
  const a_5=__cmd_x_main_buri$noisy$rh6hzk(ctx_1,'first',1n);
  let $t1;
  if(p_2===0){
    $t1=0n;
  }else if(p_2===1){
    $t1=__cmd_x_main_buri$noisy$rh6hzk(ctx_1,'second',2n);
  }else{
    $abort('no arm matched');
  }
  const b_6=$t1;
  const text_8=String(a_5*100n+b_6);
  const self_9=$host_HostStdout_println(ctx_1[1],text_8);
  let $t3;
  if(self_9[0]===0){
    $t3=0;
  }else if(self_9[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  const a_14=__cmd_x_main_buri$noisy$rh6hzk(ctx_1,'one',1n);
  const a_12=__cmd_x_main_buri$noisy$rh6hzk(ctx_1,'two',2n);
  let $t5;
  if(p_2===0){
    $t5=0n;
  }else if(p_2===1){
    $t5=__cmd_x_main_buri$noisy$rh6hzk(ctx_1,'three',3n);
  }else{
    $abort('no arm matched');
  }
  const b_13=$t5;
  const text_17=String(a_14*100n+(a_12*100n+b_13));
  const self_18=$host_HostStdout_println(ctx_1[1],text_17);
  let $t7;
  if(self_18[0]===0){
    $t7=0;
  }else if(self_18[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  return $k1;
}
function __cmd_x_main_buri$noisy$rh6hzk(ctx_0,tag_1,v_2){
  const self_5=$host_HostStdout_println(ctx_0[1],tag_1);
  let $t1;
  if(self_5[0]===0){
    $t1=0;
  }else if(self_5[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  return v_2;
}
