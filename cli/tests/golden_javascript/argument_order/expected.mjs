const $k0=[0,0];
function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  const p_1=1;
  const a_4=__cmd_x_main$noisy$u3rqgv(ctx_0,'first',1n);
  let $t1;
  if(p_1===0){
    $t1=0n;
  }else if(p_1===1){
    $t1=__cmd_x_main$noisy$u3rqgv(ctx_0,'second',2n);
  }else{
    $abort('no arm matched');
  }
  const b_5=$t1;
  $host_HostStdout_println(ctx_0[1],String(a_4*100n+b_5));
  const a_8=__cmd_x_main$noisy$u3rqgv(ctx_0,'one',1n);
  const a_6=__cmd_x_main$noisy$u3rqgv(ctx_0,'two',2n);
  let $t3;
  if(p_1===0){
    $t3=0n;
  }else if(p_1===1){
    $t3=__cmd_x_main$noisy$u3rqgv(ctx_0,'three',3n);
  }else{
    $abort('no arm matched');
  }
  const b_7=$t3;
  $host_HostStdout_println(ctx_0[1],String(a_8*100n+(a_6*100n+b_7)));
  return $k0;
}
function __cmd_x_main$noisy$u3rqgv(ctx_0,tag_1,v_2){
  $host_HostStdout_println(ctx_0[1],tag_1);
  return v_2;
}
