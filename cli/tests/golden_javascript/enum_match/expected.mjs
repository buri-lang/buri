const $k0=[0];
const $k1=[1,2];
const $k2=[2,3,4];
const $k3=[3,5];
const $k4=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  const text_2=$f64(__cmd_x_main_buri$area($k0));
  const self_3=$host_HostStdout_println(ctx_0[1],text_2);
  let $t1;
  if(self_3[0]===0){
    $t1=0;
  }else if(self_3[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_7=$f64(__cmd_x_main_buri$area($k1));
  const self_8=$host_HostStdout_println(ctx_0[1],text_7);
  let $t3;
  if(self_8[0]===0){
    $t3=0;
  }else if(self_8[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  const text_12=$f64(__cmd_x_main_buri$area($k2));
  const self_13=$host_HostStdout_println(ctx_0[1],text_12);
  let $t5;
  if(self_13[0]===0){
    $t5=0;
  }else if(self_13[0]===1){
    $t5=0;
  }else{
    $abort('no arm matched');
  }
  const text_17=$f64(__cmd_x_main_buri$area($k3));
  const self_18=$host_HostStdout_println(ctx_0[1],text_17);
  let $t7;
  if(self_18[0]===0){
    $t7=0;
  }else if(self_18[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  return $k4;
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
