const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const name_2='world';
  const n_3=42n;
  const self_8=$host_HostStdout_println(ctx_1[1],'hello '+name_2);
  let $t1;
  if(self_8[0]===0){
    $t1=0;
  }else if(self_8[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_12=String(n_3)+' and '+$f64(1.5)+' and '+name_2;
  const self_13=$host_HostStdout_println(ctx_1[1],text_12);
  let $t3;
  if(self_13[0]===0){
    $t3=0;
  }else if(self_13[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  const self_18=$host_HostStdout_println(ctx_1[1],'no holes at all');
  let $t5;
  if(self_18[0]===0){
    $t5=0;
  }else if(self_18[0]===1){
    $t5=0;
  }else{
    $abort('no arm matched');
  }
  const joined_5=$str_format(ctx_1,'n='+String(n_3));
  const self_23=$host_HostStdout_println(ctx_1[1],joined_5+joined_5);
  let $t7;
  if(self_23[0]===0){
    $t7=0;
  }else if(self_23[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  return $k1;
}
