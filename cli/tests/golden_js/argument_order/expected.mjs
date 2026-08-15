const $k0=[0,0];
function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  const p_1=1;
  const a_4=__cmd_x_main$noisy$72mdf3(ctx_0,'first',1);
  let $t1;
  if(p_1===0){
    $t1=0;
  }else if(p_1===1){
    $t1=__cmd_x_main$noisy$72mdf3(ctx_0,'second',2);
  }else{
    $abort('no arm matched');
  }
  const b_5=$t1;
  $host_HostStdout_println(ctx_0[1],String(a_4*100+b_5));
  const a_8=__cmd_x_main$noisy$72mdf3(ctx_0,'one',1);
  const a_6=__cmd_x_main$noisy$72mdf3(ctx_0,'two',2);
  let $t3;
  if(p_1===0){
    $t3=0;
  }else if(p_1===1){
    $t3=__cmd_x_main$noisy$72mdf3(ctx_0,'three',3);
  }else{
    $abort('no arm matched');
  }
  const b_7=$t3;
  $host_HostStdout_println(ctx_0[1],String(a_8*100+(a_6*100+b_7)));
  return $k0;
}
function __cmd_x_main$noisy$72mdf3(ctx_0,tag_1,v_2){
  $host_HostStdout_println(ctx_0[1],tag_1);
  return v_2;
}
