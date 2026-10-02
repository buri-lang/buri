const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[2];
const $k2=[0];
const $k3=[4,503n];
const $k4=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const text_3=__cmd_x_main_buri$kind($k1)+' '+__cmd_x_main_buri$kind($k2)+' '+__cmd_x_main_buri$kind($k3);
  const self_4=$host_HostStdout_println(ctx_1[1],text_3);
  let $t1;
  if(self_4[0]===0){
    $t1=0;
  }else if(self_4[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_10=$str(true)+' '+$str(false);
  const self_11=$host_HostStdout_println(ctx_1[1],text_10);
  let $t7;
  if(self_11[0]===0){
    $t7=0;
  }else if(self_11[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  return $k4;
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
