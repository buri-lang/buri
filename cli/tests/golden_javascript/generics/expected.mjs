const $k0=[9n,8n];
const $k3=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  const text_5=String(1n)+' '+'s'+' '+$str(true);
  const self_6=$host_HostStdout_println(ctx_0[1],text_5);
  let $t1;
  if(self_6[0]===0){
    $t1=0;
  }else if(self_6[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  let $t3;
  const $t4=$list_get($k0,0n);
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
  const text_16=String($t3)+' '+$t5;
  const self_17=$host_HostStdout_println(ctx_0[1],text_16);
  let $t7;
  if(self_17[0]===0){
    $t7=0;
  }else if(self_17[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  const text_23=String(5n)+' '+'b';
  const self_24=$host_HostStdout_println(ctx_0[1],text_23);
  let $t9;
  if(self_24[0]===0){
    $t9=0;
  }else if(self_24[0]===1){
    $t9=0;
  }else{
    $abort('no arm matched');
  }
  return $k3;
}
