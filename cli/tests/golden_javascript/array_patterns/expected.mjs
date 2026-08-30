const $k0=[1n];
const $k1=[1n,2n];
const $k2=[1n,2n,3n,4n];
const $k3=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  $host_HostStdout_println(ctx_0[1],__cmd_x_main_buri$describe([]));
  $host_HostStdout_println(ctx_0[1],__cmd_x_main_buri$describe($k0));
  $host_HostStdout_println(ctx_0[1],__cmd_x_main_buri$describe($k1));
  $host_HostStdout_println(ctx_0[1],__cmd_x_main_buri$describe($k2));
  return $k3;
}
function __cmd_x_main_buri$describe(xs_0){
  if(xs_0.length===0){
    return 'empty';
  }else if(xs_0.length===1){
    return 'one: '+String(xs_0[0]);
  }else if(xs_0.length===2){
    return 'two: '+String(xs_0[0])+','+String(xs_0[1]);
  }else if(xs_0.length>=1){
    const rest_5=xs_0.slice(1);
    return 'head '+String(xs_0[0])+' and '+String($list_len(rest_5))+' more';
  }else{
    $abort('no arm matched');
  }
}
