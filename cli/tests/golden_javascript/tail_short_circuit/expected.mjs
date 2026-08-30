const $k0=[3n,1n,4n,1n,5n];
const $k1=[0,0];
function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  $host_HostStdout_println(ctx_0[1],$str(__cmd_x_main$allBelow($k0,10n,0n))+' '+$str(__cmd_x_main$allBelow($k0,4n,0n)));
  $host_HostStdout_println(ctx_0[1],$str(__cmd_x_main$anyAtLeast($k0,5n,0n))+' '+$str(__cmd_x_main$anyAtLeast($k0,9n,0n)));
  $host_HostStdout_println(ctx_0[1],String(__cmd_x_main$firstOr($k0,2n))+' '+String(__cmd_x_main$firstOr($k0,9n)));
  $host_HostStdout_println(ctx_0[1],$str(__cmd_x_main$bothSmall(1n,2n))+' '+$str(__cmd_x_main$bothSmall(1n,20n)));
  return $k1;
}
function __cmd_x_main$allBelow(xs_0,limit_1,i_2){
  while(true){
    if(i_2>=$list_len(xs_0)){
      return true;
    }else{
      const $t1=$list_get(xs_0,i_2);
      if(($t1!==void 0?$t1:0n)<limit_1){
        i_2=i_2+1n;
        continue;
      }else{
        return false;
      }
    }
  }
}
function __cmd_x_main$anyAtLeast(xs_0,limit_1,i_2){
  while(true){
    if(i_2>=$list_len(xs_0)){
      return false;
    }else{
      const $t1=$list_get(xs_0,i_2);
      if(($t1!==void 0?$t1:0n)>=limit_1){
        return true;
      }else{
        i_2=i_2+1n;
        continue;
      }
    }
  }
}
function __cmd_x_main$firstOr(xs_0,i_1){
  while(true){
    if(i_1>=$list_len(xs_0)){
      return -1n;
    }else{
      const $t1=$list_get(xs_0,i_1);
      if($t1!==void 0){
        return $t1;
      }else{
        i_1=i_1+1n;
        continue;
      }
    }
  }
}
function __cmd_x_main$bothSmall(a_0,b_1){
  return a_0<10n&&b_1<10n;
}
