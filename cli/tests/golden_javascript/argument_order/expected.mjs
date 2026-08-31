const $k0=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  const p_1=1;
  const a_4=__cmd_x_main_buri$noisy$u3rqgv(ctx_0,'first',1n);
  let $t1;
  if(p_1===0){
    $t1=0n;
  }else if(p_1===1){
    $t1=__cmd_x_main_buri$noisy$u3rqgv(ctx_0,'second',2n);
  }else{
    $abort('no arm matched');
  }
  const b_5=$t1;
  const text_7=String(a_4*100n+b_5);
  const self_8=$host_HostStdout_println(ctx_0[1],text_7);
  let $t3;
  if(self_8[0]===0){
    $t3=0;
  }else if(self_8[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  const a_13=__cmd_x_main_buri$noisy$u3rqgv(ctx_0,'one',1n);
  const a_11=__cmd_x_main_buri$noisy$u3rqgv(ctx_0,'two',2n);
  let $t5;
  if(p_1===0){
    $t5=0n;
  }else if(p_1===1){
    $t5=__cmd_x_main_buri$noisy$u3rqgv(ctx_0,'three',3n);
  }else{
    $abort('no arm matched');
  }
  const b_12=$t5;
  const text_16=String(a_13*100n+(a_11*100n+b_12));
  const self_17=$host_HostStdout_println(ctx_0[1],text_16);
  let $t7;
  if(self_17[0]===0){
    $t7=0;
  }else if(self_17[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  return $k0;
}
function __cmd_x_main_buri$noisy$u3rqgv(ctx_0,tag_1,v_2){
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
