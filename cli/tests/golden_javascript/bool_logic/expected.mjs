const $k0=[0,0];
function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  $host_HostStdout_println(ctx_0[1],$str(__cmd_x_main$inRange(5n,1n,10n))+' '+$str(__cmd_x_main$inRange(50n,1n,10n)));
  $host_HostStdout_println(ctx_0[1],$str(true)+' '+$str(true));
  $host_HostStdout_println(ctx_0[1],String(1n)+' '+String(2n));
  return $k0;
}
function __cmd_x_main$inRange(n_0,lo_1,hi_2){
  return !(n_0<lo_1)&&!(n_0>hi_2);
}
