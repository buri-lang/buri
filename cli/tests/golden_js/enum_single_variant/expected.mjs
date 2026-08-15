function __cmd_x_main$main(){
  core_host$HostStdout_println([[],[]][1],__cmd_x_main$describe([0,7,'seven']));
  return [0,0];
}
function __cmd_x_main$describe(w_0){
  return [w_0[2],':',String(w_0[1])];
}
function core_host$HostStdout_println(self_0,text_1){
  return $host_HostStdout_println(self_0,text_1);
}
