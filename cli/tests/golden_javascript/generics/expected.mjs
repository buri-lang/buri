const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[9n,8n];
const $k4=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const text_6=String(1n)+' '+'s'+' '+$str(true);
  const self_7=$host_HostStdout_println(ctx_1[1],text_6);
  let $t1;
  if(self_7[0]===0){
    $t1=0;
  }else if(self_7[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  let $t3;
  const $t4=$list_get($k1,0n);
  if($t4!==void 0){
    $t3=$t4;
  }else if($t4===void 0){
    $t3=0n;
  }else{
    $abort('no arm matched');
  }
  let $t5;
  const $t6=$list_get([],0n);
  if($t6!==void 0){
    $t5=$t6;
  }else if($t6===void 0){
    $t5='none';
  }else{
    $abort('no arm matched');
  }
  const text_17=String($t3)+' '+$t5;
  const self_18=$host_HostStdout_println(ctx_1[1],text_17);
  let $t7;
  if(self_18[0]===0){
    $t7=0;
  }else if(self_18[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  const text_24=String(5n)+' '+'b';
  const self_25=$host_HostStdout_println(ctx_1[1],text_24);
  let $t9;
  if(self_25[0]===0){
    $t9=0;
  }else if(self_25[0]===1){
    $t9=0;
  }else{
    $abort('no arm matched');
  }
  return $k4;
}
