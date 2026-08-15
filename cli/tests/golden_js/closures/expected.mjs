function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  const xs_1=[1,2,3,4,5];
  const bias_2=10;
  const biased_4=core_list$map$vaza5f(xs_1,ctx_0,x_3=>core_num$I64_add(x_3,bias_2));
  const doubled_6=core_list$map$vaza5f(xs_1,ctx_0,x_5=>core_num$I64_mul(x_5,2));
  const summed_9=core_list$fold$71n5xt(xs_1,(acc_7,x_8)=>__cmd_x_main$add(acc_7,x_8),0);
  const big_11=core_list$filter$hv6580(xs_1,ctx_0,x_10=>x_10>2);
  core_host$HostStdout_println(ctx_0[1],[String(core_list$sum(biased_4)),' ',String(core_list$sum(doubled_6)),' ',String(summed_9),' ',String(core_list$len$1bogxm(big_11))]);
  return [0,0];
}
function core_num$I64_add(self_0,a0_1){
  return self_0+a0_1;
}
function core_list$map$vaza5f(self_0,ctx_1,f_2){
  return $list_map(self_0,ctx_1,f_2);
}
function core_num$I64_mul(self_0,a0_1){
  return self_0*a0_1;
}
function __cmd_x_main$add(a_0,b_1){
  return a_0+b_1;
}
function core_list$fold$71n5xt(self_0,f_1,init_2){
  return $list_fold(self_0,f_1,init_2);
}
function core_list$filter$hv6580(self_0,ctx_1,keep_2){
  return $list_filter(self_0,ctx_1,keep_2);
}
function core_list$sum(self_0){
  return core_list$fold$71n5xt(self_0,(acc_1,x_2)=>acc_1+x_2,0);
}
function core_list$len$1bogxm(self_0){
  return $list_len(self_0);
}
function core_host$HostStdout_println(self_0,text_1){
  return $host_HostStdout_println(self_0,text_1);
}
