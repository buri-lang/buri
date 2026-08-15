function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  core_host$HostStdout_println(ctx_0[1],[$f64(__cmd_x_main$area([0]))]);
  core_host$HostStdout_println(ctx_0[1],[$f64(__cmd_x_main$area([1,2]))]);
  core_host$HostStdout_println(ctx_0[1],[$f64(__cmd_x_main$area([2,3,4]))]);
  core_host$HostStdout_println(ctx_0[1],[$f64(__cmd_x_main$area([3,5]))]);
  return [0,0];
}
function __cmd_x_main$area(s_0){
  if(s_0[0]===0){
    return 0;
  }else if(s_0[0]===1){
    const r_1=s_0[1];
    return 3*r_1*r_1;
  }else if(s_0[0]===2){
    return s_0[1]*s_0[2];
  }else if(s_0[0]===3){
    const side_4=s_0[1];
    return side_4*side_4;
  }else{
    $abort('no arm matched');
  }
}
function core_host$HostStdout_println(self_0,text_1){
  return $host_HostStdout_println(self_0,text_1);
}
