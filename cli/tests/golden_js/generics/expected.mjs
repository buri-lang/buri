function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  $host_HostStdout_println(ctx_0[1],[String(1),' ','s',' ',true]);
  let $t2;
  const $t3=$list_get([9,8],0);
  if($t3[0]===0){
    $t2=$t3[1];
  }else if($t3[0]===1){
    $t2=0;
  }else{
    $abort('no arm matched');
  }
  let $t4;
  const $t5=$list_get([],0);
  if($t5[0]===0){
    $t4=$t5[1];
  }else if($t5[0]===1){
    $t4='none';
  }else{
    $abort('no arm matched');
  }
  $host_HostStdout_println(ctx_0[1],[String($t2),' ',$t4]);
  $host_HostStdout_println(ctx_0[1],[String(5),' ','b']);
  return [0,0];
}
