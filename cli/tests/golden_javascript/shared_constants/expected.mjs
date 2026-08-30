const $k0=[0];
const $k2=[0,0];
const $k3=[1,'zero'];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  $host_HostStdout_println(ctx_0[1],String(__cmd_x_main_buri$tally(10n,0n)));
  $host_HostStdout_println(ctx_0[1],String(0n)+' '+String(1n));
  return $k2;
}
function __cmd_x_main_buri$tally(n_0,acc_1){
  while(true){
    if(n_0===0n){
      return acc_1+0n;
    }else{
      const $t3=n_0-1n;
      const n_2=n_0-5n;
      let $t1;
      const $t2=n_2<0n?$k0:n_2===0n?$k3:[2,n_2];
      switch($t2[0]){
        case 0:
          {
            $t1=0n;
          }
          break;
        case 1:
          {
            $t1=1n;
          }
          break;
        case 2:
          {
            $t1=$t2[1];
          }
          break;
        default:
          {
            $abort('no arm matched');
          }
          break;
      }
      n_0=$t3;
      acc_1=acc_1+$t1;
      continue;
    }
  }
}
