function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  $host_HostStdout_println(ctx_0[1],__cmd_x_main$describe([]));
  $host_HostStdout_println(ctx_0[1],__cmd_x_main$describe([1]));
  $host_HostStdout_println(ctx_0[1],__cmd_x_main$describe([1,2]));
  $host_HostStdout_println(ctx_0[1],__cmd_x_main$describe([1,2,3,4]));
  return [0,0];
}
function __cmd_x_main$describe(xs_0){
  if(xs_0.length===0){
    return ['empty'];
  }else if(xs_0.length===1){
    return ['one: ',String(xs_0[0])];
  }else if(xs_0.length===2){
    return ['two: ',String(xs_0[0]),',',String(xs_0[1])];
  }else if(xs_0.length>=1){
    const rest_5=xs_0.slice(1);
    return ['head ',String(xs_0[0]),' and ',String($list_len(rest_5)),' more'];
  }else{
    $abort('no arm matched');
  }
}
