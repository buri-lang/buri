const $k0=[0,0];
function __cmd_x_main_buri$main(){
  $host_HostStdout_println([],__cmd_x_main_buri$size(500n)+' '+__cmd_x_main_buri$size(50n)+' '+__cmd_x_main_buri$size(5n)+' '+__cmd_x_main_buri$size(0n));
  return $k0;
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
