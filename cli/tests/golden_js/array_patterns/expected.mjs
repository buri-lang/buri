function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  core_host$HostStdout_println(ctx_0[1],__cmd_x_main$describe([]));
  core_host$HostStdout_println(ctx_0[1],__cmd_x_main$describe([1]));
  core_host$HostStdout_println(ctx_0[1],__cmd_x_main$describe([1,2]));
  core_host$HostStdout_println(ctx_0[1],__cmd_x_main$describe([1,2,3,4]));
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
    return ['head ',String(xs_0[0]),' and ',String(core_list$len$1bogxm(rest_5)),' more'];
  }else{
    $abort('no arm matched');
  }
}
function core_host$HostStdout_println(self_0,text_1){
  return $host_HostStdout_println(self_0,text_1);
}
function core_list$len$1bogxm(self_0){
  return $list_len(self_0);
}
