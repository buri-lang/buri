const $k0=[2];
const $k1=[0];
const $k2=[4,503n];
const $k3=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  $host_HostStdout_println(ctx_0[1],__cmd_x_main_buri$kind($k0)+' '+__cmd_x_main_buri$kind($k1)+' '+__cmd_x_main_buri$kind($k2));
  $host_HostStdout_println(ctx_0[1],$str(true)+' '+$str(false));
  return $k3;
}
function __cmd_x_main_buri$kind(s_0){
  switch(s_0[0]){
    case 1:
    case 2:
      {
        return 'missing';
      }
    case 3:
    case 0:
      {
        return 'fine';
      }
    case 4:
      {
        return s_0[1]>=500n?'server':'other';
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
