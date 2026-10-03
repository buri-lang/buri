const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[1n];
const $k2=[1n,2n];
const $k3=[1n,2n,3n,4n];
const $k4=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const text_3=__cmd_x_main_buri$describe([]);
  const self_4=$host_HostStdout_println(ctx_1[1],text_3);
  let $t1;
  if(self_4[0]===0){
    $t1=0;
  }else if(self_4[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_8=__cmd_x_main_buri$describe($k1);
  const self_9=$host_HostStdout_println(ctx_1[1],text_8);
  let $t3;
  if(self_9[0]===0){
    $t3=0;
  }else if(self_9[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  const text_13=__cmd_x_main_buri$describe($k2);
  const self_14=$host_HostStdout_println(ctx_1[1],text_13);
  let $t5;
  if(self_14[0]===0){
    $t5=0;
  }else if(self_14[0]===1){
    $t5=0;
  }else{
    $abort('no arm matched');
  }
  const text_18=__cmd_x_main_buri$describe($k3);
  const self_19=$host_HostStdout_println(ctx_1[1],text_18);
  let $t7;
  if(self_19[0]===0){
    $t7=0;
  }else if(self_19[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  return $k4;
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
    return 'head '+String(xs_0[0])+' and '+String($list_length(rest_5))+' more';
  }else{
    $abort('no arm matched');
  }
}
