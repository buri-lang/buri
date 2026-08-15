function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  core_host$HostStdout_println(ctx_0[1],[String(__cmd_x_main$identity$1bogxm(1)),' ',__cmd_x_main$identity$ea3yj9('s'),' ',__cmd_x_main$identity$1iz826(true)]);
  core_host$HostStdout_println(ctx_0[1],[String(__cmd_x_main$firstOr$1bogxm([9,8],0)),' ',__cmd_x_main$firstOr$ea3yj9([],'none')]);
  core_host$HostStdout_println(ctx_0[1],[String(__cmd_x_main$unbox$1bogxm([5])),' ',__cmd_x_main$unbox$ea3yj9(['b'])]);
  return [0,0];
}
function __cmd_x_main$identity$1bogxm(x_0){
  return x_0;
}
function __cmd_x_main$identity$ea3yj9(x_0){
  return x_0;
}
function __cmd_x_main$identity$1iz826(x_0){
  return x_0;
}
function core_host$HostStdout_println(self_0,text_1){
  return $host_HostStdout_println(self_0,text_1);
}
function __cmd_x_main$firstOr$1bogxm(xs_0,fallback_1){
  const $t1=core_list$first$1bogxm(xs_0);
  if($t1[0]===0){
    return $t1[1];
  }else if($t1[0]===1){
    return fallback_1;
  }else{
    $abort('no arm matched');
  }
}
function __cmd_x_main$firstOr$ea3yj9(xs_0,fallback_1){
  const $t1=core_list$first$ea3yj9(xs_0);
  if($t1[0]===0){
    return $t1[1];
  }else if($t1[0]===1){
    return fallback_1;
  }else{
    $abort('no arm matched');
  }
}
function __cmd_x_main$unbox$1bogxm(b_0){
  return b_0[0];
}
function __cmd_x_main$unbox$ea3yj9(b_0){
  return b_0[0];
}
function core_list$first$ea3yj9(self_0){
  return core_list$get$ea3yj9(self_0,0);
}
function core_list$get$ea3yj9(self_0,index_1){
  return $list_get(self_0,index_1);
}
function core_list$first$1bogxm(self_0){
  return core_list$get$1bogxm(self_0,0);
}
function core_list$get$1bogxm(self_0,index_1){
  return $list_get(self_0,index_1);
}
