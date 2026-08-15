function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  core_host$HostStdout_println(ctx_0[1],[String(__cmd_x_main$mix(10,4)),' ',String(__cmd_x_main$constants()),' ',$f64(__cmd_x_main$floats(1.5,3))]);
  core_host$HostStdout_println(ctx_0[1],[String(__cmd_x_main$narrow(3,4)),' ',String(core_num$I64_rem(17,5)),' ',String(core_num$I64_div(-7,2))]);
  return [0,0];
}
function __cmd_x_main$mix(a_0,b_1){
  return $divi((a_0+1)*2-b_1,3);
}
function __cmd_x_main$constants(){
  return core_num$I64_sub(core_num$I64_mul(core_num$I64_add(2,3),4),1);
}
function __cmd_x_main$floats(x_0,y_1){
  return (x_0*2+y_1)/4;
}
function core_host$HostStdout_println(self_0,text_1){
  return $host_HostStdout_println(self_0,text_1);
}
function __cmd_x_main$narrow(a_0,b_1){
  return a_0*b_1+1;
}
function core_num$I64_rem(self_0,a0_1){
  return $remi(self_0,a0_1);
}
function core_num$I64_div(self_0,a0_1){
  return $divi(self_0,a0_1);
}
function core_num$I64_add(self_0,a0_1){
  return self_0+a0_1;
}
function core_num$I64_mul(self_0,a0_1){
  return self_0*a0_1;
}
function core_num$I64_sub(self_0,a0_1){
  return self_0-a0_1;
}
