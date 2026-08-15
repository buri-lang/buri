const $k3=[2,3,5,7,11];
const $k4=[0,0];
function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  $host_HostStdout_println(ctx_0[1],String(6)+' '+'default');
  $host_HostStdout_println(ctx_0[1],String(1)+' '+String(3));
  $host_HostStdout_println(ctx_0[1],String(0)+' '+String($list_len($k3))+' '+String($list_fold($k3,(acc_4,x_5)=>acc_4+x_5,0)));
  return $k4;
}
