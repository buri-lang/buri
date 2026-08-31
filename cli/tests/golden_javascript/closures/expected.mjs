const $k0=[1n,2n,3n,4n,5n];
const $k1=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  const bias_2=10n;
  const biased_4=$list_map($k0,ctx_0,x_3=>x_3+bias_2);
  const doubled_6=$list_map($k0,ctx_0,x_5=>x_5*2n);
  const summed_9=$list_fold($k0,(acc_7,x_8)=>acc_7+x_8,0n);
  const big_11=$list_filter($k0,ctx_0,x_10=>x_10>2n);
  $host_HostStdout_println(ctx_0[1],String(core_list$sum(biased_4))+' '+String(core_list$sum(doubled_6))+' '+String(summed_9)+' '+String($list_len(big_11)));
  return $k1;
}
function core_list$sum(self_0){
  return $list_fold(self_0,(acc_1,x_2)=>acc_1+x_2,0n);
}
