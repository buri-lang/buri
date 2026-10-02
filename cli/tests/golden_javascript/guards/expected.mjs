const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[0,0];
function __cmd_x_main_buri$main(){
  const text_3=__cmd_x_main_buri$size(500n)+' '+__cmd_x_main_buri$size(50n)+' '+__cmd_x_main_buri$size(5n)+' '+__cmd_x_main_buri$size(0n);
  const self_4=$host_HostStdout_println([$k0[0],$k0[1]][1],text_3);
  let $t1;
  if(self_4[0]===0){
    $t1=0;
  }else if(self_4[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  return $k1;
}
function __cmd_x_main_buri$size(n_0){
  if(n_0>100n){
    return 'huge';
  }
  if(n_0>10n){
    return 'big';
  }
  if(n_0>0n){
    return 'small';
  }
  return 'none';
}
