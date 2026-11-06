const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[1n];
const $k2=[2n];
const $k3=[0,0];
function __cmd_x_main_buri$main(){
  const a_2=__cmd_x_main_buri$total$pgqe8e($k1,1n);
  const b_3=__cmd_x_main_buri$total$pgqe8eist2y3($k2,1n);
  const text_5=String(a_2)+' '+String(b_3);
  const self_6=$host_HostStdout_println($k0[1],text_5);
  let $t1;
  if(self_6[0]===0){
    $t1=0;
  }else if(self_6[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  return $k3;
}
function __cmd_x_main_buri$total$pgqe8e(x_0,n_1){
  return n_1<=0n?x_0[0]*10n:x_0[0]*10n+__cmd_x_main_buri$total$pgqe8e(x_0,n_1-1n);
}
function __cmd_x_main_buri$total$pgqe8eist2y3(x_0,n_1){
  return n_1<=0n?x_0[0]*100n:x_0[0]*100n+__cmd_x_main_buri$total$pgqe8eist2y3(x_0,n_1-1n);
}
