const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[0,0];
async function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  __cmd_x_main_buri$eager$9xug0c(ctx_1,1n);
  __cmd_x_main_buri$eager$9xug0c(ctx_1,2n);
  const split_3=(await $lazy(0,$env0)).__cmd_x_main_buri$report$9xug0c;
  split_3(ctx_1,3n);
  return $k1;
}
function __cmd_x_main_buri$eager$9xug0c(ctx_0,n_1){
  const text_5=$str_format(ctx_0,'['+String(n_1)+']');
  const self_6=$host_HostStdout_println(ctx_0[1],text_5);
  if(self_6[0]===0){
    return 0;
  }else if(self_6[0]===1){
    return 0;
  }else{
    $abort('no arm matched');
  }
}
function $env0(){
  return {$abort:$abort,$host_HostStdout_println:$host_HostStdout_println,$str_format:$str_format};
}
