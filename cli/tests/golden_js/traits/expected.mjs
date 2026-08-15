function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  const s_1=[3];
  const r_2=[2,5];
  core_host$HostStdout_println(ctx_0[1],[String(__cmd_x_main$Sq_area(s_1)),' ',String(__cmd_x_main$Rect_area(r_2)),' ',String(__cmd_x_main$Sq_describe(s_1))]);
  core_host$HostStdout_println(ctx_0[1],[String(__cmd_x_main$total$umrook(s_1,[4])),' ',String(__cmd_x_main$total$loew50(r_2,[1,1]))]);
  return [0,0];
}
function __cmd_x_main$Sq_area(self_0){
  return self_0[0]*self_0[0];
}
function __cmd_x_main$Rect_area(self_0){
  return self_0[0]*self_0[1];
}
function __cmd_x_main$Sq_describe(self_0){
  return __cmd_x_main$Sq_area(self_0)*2;
}
function core_host$HostStdout_println(self_0,text_1){
  return $host_HostStdout_println(self_0,text_1);
}
function __cmd_x_main$total$umrook(a_0,b_1){
  return __cmd_x_main$Sq_area(a_0)+__cmd_x_main$Sq_area(b_1);
}
function __cmd_x_main$total$loew50(a_0,b_1){
  return __cmd_x_main$Rect_area(a_0)+__cmd_x_main$Rect_area(b_1);
}
