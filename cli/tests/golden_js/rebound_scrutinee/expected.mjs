function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  let $t1;
  const $t2=[0,7];
  if($t2[0]===0){
    $t1=7;
  }else if($t2[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  let $t3;
  const $t4=[1];
  if($t4[0]===0){
    $t3=[1][1];
  }else if($t4[0]===1){
    $t3=9;
  }else{
    $abort('no arm matched');
  }
  $host_HostStdout_println(ctx_0[1],[String($t1),' ',String($t3)]);
  let $t8;
  const $t9=[0,5];
  if($t9[0]===0){
    $t8=5;
  }else if($t9[0]===1){
    $t8=0;
  }else{
    $abort('no arm matched');
  }
  let $t10;
  const $t11=[1];
  if($t11[0]===0){
    $t10=[1][1];
  }else if($t11[0]===1){
    $t10=0;
  }else{
    $abort('no arm matched');
  }
  $host_HostStdout_println(ctx_0[1],[String(3),' ',String($t8),' ',String($t10)]);
  return [0,0];
}
