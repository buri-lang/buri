const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const text_3=__cmd_x_main_buri$name(0)+' '+__cmd_x_main_buri$name(5);
  const self_4=$host_HostStdout_println(ctx_1[1],text_3);
  let $t1;
  if(self_4[0]===0){
    $t1=0;
  }else if(self_4[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_8=__cmd_x_main_buri$name(2)+' '+__cmd_x_main_buri$name(4);
  const self_9=$host_HostStdout_println(ctx_1[1],text_8);
  let $t3;
  if(self_9[0]===0){
    $t3=0;
  }else if(self_9[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  return $k1;
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
