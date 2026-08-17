const $k0=[0,0];
function __cmd_x_main$main(){
  $host_HostStdout_println([],$str(__cmd_x_main$isEven(1e3))+' '+$str(__cmd_x_main$isOdd(1001))+' '+$str(__cmd_x_main$isEven(7)));
  return $k0;
}
function __cmd_x_main$isEven(n_0){
  return $tc0(0,n_0);
}
function __cmd_x_main$isOdd(n_0){
  return $tc0(1,n_0);
}
function $tc0($w,a0_0){
  while(true){
    switch($w){
      case 0:
        if(a0_0===0){
          return true;
        }else{
          a0_0=a0_0-1;
          $w=1;
          continue;
        }
      case 1:
        if(a0_0===0){
          return false;
        }else{
          a0_0=a0_0-1;
          $w=0;
          continue;
        }
    }
  }
}
