function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  core_host$HostStdout_println(ctx_0[1],[__cmd_x_main$inRange(5,1,10),' ',__cmd_x_main$inRange(50,1,10)]);
  core_host$HostStdout_println(ctx_0[1],[__cmd_x_main$either(false,true),' ',__cmd_x_main$neither(false,false)]);
  core_host$HostStdout_println(ctx_0[1],[String(__cmd_x_main$pick(true,1,2)),' ',String(__cmd_x_main$pick(false,1,2))]);
  return [0,0];
}
function __cmd_x_main$inRange(n_0,lo_1,hi_2){
  return !(n_0<lo_1)&&!(n_0>hi_2);
}
function core_host$HostStdout_println(self_0,text_1){
  return $host_HostStdout_println(self_0,text_1);
}
function __cmd_x_main$either(a_0,b_1){
  return a_0?true:b_1;
}
function __cmd_x_main$neither(a_0,b_1){
  return a_0?false:!b_1;
}
function __cmd_x_main$pick(flag_0,a_1,b_2){
  return flag_0?a_1:b_2;
}
