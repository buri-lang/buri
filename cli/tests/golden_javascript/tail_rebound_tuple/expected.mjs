const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[0,0];
function __cmd_x_main_buri$main(){
  const text_3=String(__cmd_x_main_buri$doubledSum(3n,0n))+' '+String(__cmd_x_main_buri$ping(5n,0n));
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
function __cmd_x_main_buri$doubledSum(n_0,acc_1){
  while(true){
    const pair_2=[n_0*2n,acc_1+1n];
    if(n_0===0n){
      return acc_1;
    }else{
      n_0=n_0-1n;
      acc_1=acc_1+pair_2[0];
      continue;
    }
  }
}
function __cmd_x_main_buri$ping(n_0,acc_1){
  return $tc0(0,n_0,acc_1);
}
function $tc0($w,a0_0,a1_1){
  while(true){
    switch($w){
      case 0:
        {
          const pair_4=[a0_0*2n,a1_1];
          if(a0_0===0n){
            return a1_1;
          }else{
            a0_0=a0_0-1n;
            a1_1=a1_1+pair_4[0];
            $w=1;
            continue;
          }
        }
      case 1:
        {
          const pair_7=[a0_0*3n,a1_1];
          if(a0_0===0n){
            return a1_1;
          }else{
            a0_0=a0_0-1n;
            a1_1=a1_1+pair_7[0];
            $w=0;
            continue;
          }
        }
    }
  }
}
