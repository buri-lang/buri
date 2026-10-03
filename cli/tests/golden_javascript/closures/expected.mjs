const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[1n,2n,3n,4n,5n];
const $k2=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const bias_3=10n;
  const biased_5=$list_map($k1,ctx_1,x_4=>x_4+bias_3);
  const doubled_7=$list_map($k1,ctx_1,x_6=>x_6*2n);
  const summed_10=$list_fold($k1,(acc_8,x_9)=>acc_8+x_9,0n);
  const big_12=$list_filter($k1,ctx_1,x_11=>x_11>2n);
  const text_16=String(core_list$sum(biased_5))+' '+String(core_list$sum(doubled_7))+' '+String(summed_10)+' '+String($list_length(big_12));
  const self_17=$host_HostStdout_println(ctx_1[1],text_16);
  let $t1;
  if(self_17[0]===0){
    $t1=0;
  }else if(self_17[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  return $k2;
}
function core_list$sum(self_0){
  return $list_fold(self_0,(acc_1,x_2)=>acc_1+x_2,0n);
}
