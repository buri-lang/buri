function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  $host_HostStdout_println(ctx_0[1],[__cmd_x_main$inRange(5,1,10),' ',__cmd_x_main$inRange(50,1,10)]);
  $host_HostStdout_println(ctx_0[1],[true,' ',true]);
  $host_HostStdout_println(ctx_0[1],[String(1),' ',String(2)]);
  return [0,0];
}
function __cmd_x_main$inRange(n_0,lo_1,hi_2){
  return !(n_0<lo_1)&&!(n_0>hi_2);
}
