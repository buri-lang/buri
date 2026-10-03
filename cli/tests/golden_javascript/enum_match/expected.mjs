const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[0];
const $k2=[1,2];
const $k3=[2,3,4];
const $k4=[3,5];
const $k5=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const text_3=$f64(__cmd_x_main_buri$area($k1));
  const self_4=$host_HostStdout_println(ctx_1[1],text_3);
  let $t1;
  if(self_4[0]===0){
    $t1=0;
  }else if(self_4[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_8=$f64(__cmd_x_main_buri$area($k2));
  const self_9=$host_HostStdout_println(ctx_1[1],text_8);
  let $t3;
  if(self_9[0]===0){
    $t3=0;
  }else if(self_9[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  const text_13=$f64(__cmd_x_main_buri$area($k3));
  const self_14=$host_HostStdout_println(ctx_1[1],text_13);
  let $t5;
  if(self_14[0]===0){
    $t5=0;
  }else if(self_14[0]===1){
    $t5=0;
  }else{
    $abort('no arm matched');
  }
  const text_18=$f64(__cmd_x_main_buri$area($k4));
  const self_19=$host_HostStdout_println(ctx_1[1],text_18);
  let $t7;
  if(self_19[0]===0){
    $t7=0;
  }else if(self_19[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  return $k5;
}
function __cmd_x_main_buri$area(s_0){
  switch(s_0[0]){
    case 0:
      {
        return 0;
      }
    case 1:
      {
        const r_1=s_0[1];
        return 3*r_1*r_1;
      }
    case 2:
      {
        return s_0[1]*s_0[2];
      }
    case 3:
      {
        const side_4=s_0[1];
        return side_4*side_4;
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
