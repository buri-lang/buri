const $k3=[2n,3n,5n,7n,11n];
const $k4=[0,0];
function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  $host_HostStdout_println(ctx_0[1],String(6n)+' '+'default');
  $host_HostStdout_println(ctx_0[1],String(1n)+' '+String(3n));
  $host_HostStdout_println(ctx_0[1],String(0n)+' '+String($list_len($k3))+' '+String($list_fold($k3,(acc_4,x_5)=>acc_4+x_5,0n)));
  return $k4;
}
