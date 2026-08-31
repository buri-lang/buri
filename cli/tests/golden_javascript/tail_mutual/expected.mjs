const $k0=[0,0];
function __cmd_x_main_buri$main(){
  const text_2=$str(__cmd_x_main_buri$isEven(1000n))+' '+$str(__cmd_x_main_buri$isOdd(1001n))+' '+$str(__cmd_x_main_buri$isEven(7n));
  const self_3=$host_HostStdout_println([[],[]][1],text_2);
  let $t1;
  if(self_3[0]===0){
    $t1=0;
  }else if(self_3[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  return $k0;
}
function __cmd_x_main_buri$isEven(n_0){
  return $tc0(0,n_0);
}
function __cmd_x_main_buri$isOdd(n_0){
  return $tc0(1,n_0);
}
function $tc0($w,a0_0){
  while(true){
    switch($w){
      case 0:
        if(a0_0===0n){
          return true;
        }else{
          a0_0=a0_0-1n;
          $w=1;
          continue;
        }
      case 1:
        if(a0_0===0n){
          return false;
        }else{
          a0_0=a0_0-1n;
          $w=0;
          continue;
        }
    }
  }
}
