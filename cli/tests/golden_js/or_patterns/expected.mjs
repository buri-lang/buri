function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  core_host$HostStdout_println(ctx_0[1],[__cmd_x_main$kind([2]),' ',__cmd_x_main$kind([0]),' ',__cmd_x_main$kind([4,503])]);
  core_host$HostStdout_println(ctx_0[1],[__cmd_x_main$small(2),' ',__cmd_x_main$small(9)]);
  return [0,0];
}
function __cmd_x_main$kind(s_0){
  if(s_0[0]===1||s_0[0]===2){
    return 'missing';
  }else if(s_0[0]===3||s_0[0]===0){
    return 'fine';
  }else if(s_0[0]===4){
    return s_0[1]>=500?'server':'other';
  }else{
    $abort('no arm matched');
  }
}
function core_host$HostStdout_println(self_0,text_1){
  return $host_HostStdout_println(self_0,text_1);
}
function __cmd_x_main$small(n_0){
  return n_0===0||n_0===1||n_0===2||n_0===3;
}
