const $k0=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  $host_HostStdout_println(ctx_0[1],__cmd_x_main_buri$name(0)+' '+__cmd_x_main_buri$name(5));
  $host_HostStdout_println(ctx_0[1],__cmd_x_main_buri$name(2)+' '+__cmd_x_main_buri$name(4));
  return $k0;
}
function __cmd_x_main_buri$name(c_0){
  switch(c_0){
    case 0:
      {
        return 'red';
      }
    case 1:
      {
        return 'green';
      }
    case 2:
      {
        return 'blue';
      }
    case 3:
      {
        return 'cyan';
      }
    case 4:
      {
        return 'magenta';
      }
    case 5:
      {
        return 'yellow';
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
