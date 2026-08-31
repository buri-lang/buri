const $k0=[2];
const $k1=[0];
const $k2=[4,503n];
const $k3=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  const text_2=__cmd_x_main_buri$kind($k0)+' '+__cmd_x_main_buri$kind($k1)+' '+__cmd_x_main_buri$kind($k2);
  const self_3=$host_HostStdout_println(ctx_0[1],text_2);
  let $t1;
  if(self_3[0]===0){
    $t1=0;
  }else if(self_3[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_9=$str(true)+' '+$str(false);
  const self_10=$host_HostStdout_println(ctx_0[1],text_9);
  let $t7;
  if(self_10[0]===0){
    $t7=0;
  }else if(self_10[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
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
