let $abort;
let $host_HostStdout_println;
let $str_format;
function $bind(m){
  $abort=m.$abort;
  $host_HostStdout_println=m.$host_HostStdout_println;
  $str_format=m.$str_format;
}
function __cmd_x_main_buri$report$u3rqgv(ctx_0,n_1){
  const text_5=$str_format(ctx_0,'row '+$str_format(ctx_0,'['+String(n_1)+']'));
  const self_6=$host_HostStdout_println(ctx_0[1],text_5);
  if(self_6[0]===0){
    return 0;
  }else if(self_6[0]===1){
    return 0;
  }else{
    $abort('no arm matched');
  }
}
export{$bind,__cmd_x_main_buri$report$u3rqgv};
