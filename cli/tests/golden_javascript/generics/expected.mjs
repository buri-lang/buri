const $k0=[9n,8n];
const $k3=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  $host_HostStdout_println(ctx_0[1],String(1n)+' '+'s'+' '+$str(true));
  let $t2;
  const $t3=$list_get($k0,0n);
  if($t3!==void 0){
    $t2=$t3;
  }else if($t3===void 0){
    $t2=0n;
  }else{
    $abort('no arm matched');
  }
  let $t4;
  const $t5=$list_get([],0n);
  if($t5!==void 0){
    $t4=$t5;
  }else if($t5===void 0){
    $t4='none';
  }else{
    $abort('no arm matched');
  }
  $host_HostStdout_println(ctx_0[1],String($t2)+' '+$t4);
  $host_HostStdout_println(ctx_0[1],String(5n)+' '+'b');
  return $k3;
}
