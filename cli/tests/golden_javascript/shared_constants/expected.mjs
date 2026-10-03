const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[0];
const $k3=[0,0];
const $k4=[1,'zero'];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const text_3=String(__cmd_x_main_buri$tally(10n,0n));
  const self_4=$host_HostStdout_println(ctx_1[1],text_3);
  let $t1;
  if(self_4[0]===0){
    $t1=0;
  }else if(self_4[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_12=String(0n)+' '+String(1n);
  const self_13=$host_HostStdout_println(ctx_1[1],text_12);
  let $t7;
  if(self_13[0]===0){
    $t7=0;
  }else if(self_13[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  return $k3;
}
function __cmd_x_main_buri$tally(n_0,acc_1){
  while(true){
    if(n_0===0n){
      return acc_1+0n;
    }else{
      const $t3=n_0-1n;
      const n_2=n_0-5n;
      let $t1;
      const $t2=n_2<0n?$k1:n_2===0n?$k4:[2,n_2];
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
