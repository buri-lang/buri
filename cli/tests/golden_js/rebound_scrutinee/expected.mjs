function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  core_host$HostStdout_println(ctx_0[1],[String(__cmd_x_main$unwrap_([0,7],0)),' ',String(__cmd_x_main$unwrap_([1],9))]);
  core_host$HostStdout_println(ctx_0[1],[String(__cmd_x_main$firstField([3,'x'])),' ',String(__cmd_x_main$passthrough([0,5])),' ',String(__cmd_x_main$passthrough([1]))]);
  return [0,0];
}
function __cmd_x_main$unwrap_(w_0,fallback_1){
  if(w_0[0]===0){
    return w_0[1];
  }else if(w_0[0]===1){
    return fallback_1;
  }else{
    $abort('no arm matched');
  }
}
function core_host$HostStdout_println(self_0,text_1){
  return $host_HostStdout_println(self_0,text_1);
}
function __cmd_x_main$firstField(pair_0){
  return pair_0[0];
}
function __cmd_x_main$passthrough(o_0){
  if(o_0[0]===0){
    return o_0[1];
  }else if(o_0[0]===1){
    return 0;
  }else{
    $abort('no arm matched');
  }
}
