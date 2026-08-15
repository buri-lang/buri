const $k0=[0,0];
function __cmd_x_main$main(){
  $host_HostStdout_println([],__cmd_x_main$size(500)+' '+__cmd_x_main$size(50)+' '+__cmd_x_main$size(5)+' '+__cmd_x_main$size(0));
  return $k0;
}
function __cmd_x_main$size(n_0){
  while(true){
    if(n_0>100){
      return 'huge';
    }
    if(n_0>10){
      return 'big';
    }
    if(n_0>0){
      return 'small';
    }
    return 'none';
  }
}
