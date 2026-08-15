function __cmd_x_main$main(){
  core_host$HostStdout_println([[],[]][1],[String(__cmd_x_main$code('get')),' ',String(__cmd_x_main$code('delete')),' ',String(__cmd_x_main$code('nope'))]);
  return [0,0];
}
function __cmd_x_main$code(name_0){
  return name_0==='get'?1:name_0==='put'?2:name_0==='post'?3:name_0==='patch'?4:name_0==='delete'?5:name_0==='head'?6:0;
}
function core_host$HostStdout_println(self_0,text_1){
  return $host_HostStdout_println(self_0,text_1);
}
