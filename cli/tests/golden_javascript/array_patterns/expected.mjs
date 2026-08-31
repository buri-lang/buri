const $k0=[1n];
const $k1=[1n,2n];
const $k2=[1n,2n,3n,4n];
const $k3=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  const text_2=__cmd_x_main_buri$describe([]);
  const self_3=$host_HostStdout_println(ctx_0[1],text_2);
  let $t1;
  if(self_3[0]===0){
    $t1=0;
  }else if(self_3[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_7=__cmd_x_main_buri$describe($k0);
  const self_8=$host_HostStdout_println(ctx_0[1],text_7);
  let $t3;
  if(self_8[0]===0){
    $t3=0;
  }else if(self_8[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  const text_12=__cmd_x_main_buri$describe($k1);
  const self_13=$host_HostStdout_println(ctx_0[1],text_12);
  let $t5;
  if(self_13[0]===0){
    $t5=0;
  }else if(self_13[0]===1){
    $t5=0;
  }else{
    $abort('no arm matched');
  }
  const text_17=__cmd_x_main_buri$describe($k2);
  const self_18=$host_HostStdout_println(ctx_0[1],text_17);
  let $t7;
  if(self_18[0]===0){
    $t7=0;
  }else if(self_18[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  return $k3;
}
function __cmd_x_main_buri$describe(xs_0){
  if(xs_0.length===0){
    return 'empty';
  }else if(xs_0.length===1){
    return 'one: '+String(xs_0[0]);
  }else if(xs_0.length===2){
    return 'two: '+String(xs_0[0])+','+String(xs_0[1]);
  }else if(xs_0.length>=1){
    const rest_5=xs_0.slice(1);
    return 'head '+String(xs_0[0])+' and '+String($list_len(rest_5))+' more';
  }else{
    $abort('no arm matched');
  }
}
