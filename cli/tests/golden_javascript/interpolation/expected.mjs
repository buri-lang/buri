const $k0=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  const name_1='world';
  const n_2=42n;
  const self_7=$host_HostStdout_println(ctx_0[1],'hello '+name_1);
  let $t1;
  if(self_7[0]===0){
    $t1=0;
  }else if(self_7[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_11=String(n_2)+' and '+$f64(1.5)+' and '+name_1;
  const self_12=$host_HostStdout_println(ctx_0[1],text_11);
  let $t3;
  if(self_12[0]===0){
    $t3=0;
  }else if(self_12[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  const self_17=$host_HostStdout_println(ctx_0[1],'no holes at all');
  let $t5;
  if(self_17[0]===0){
    $t5=0;
  }else if(self_17[0]===1){
    $t5=0;
  }else{
    $abort('no arm matched');
  }
  const joined_4=$str_format(ctx_0,'n='+String(n_2));
  const self_22=$host_HostStdout_println(ctx_0[1],joined_4+joined_4);
  let $t7;
  if(self_22[0]===0){
    $t7=0;
  }else if(self_22[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  return $k0;
}
