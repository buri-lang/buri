const $k0=['port','8080'];
const $k1=['host','local'];
const $k2=[$k0,$k1];
const $k3=[0,0];
const $k4=[1];
function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  const $t1=__cmd_x_main$lookup($k2,'missing');
  $host_HostStdout_println(ctx_0[1],[$t1[0]===0?$t1[1]:'none']);
  let $t2;
  const $t3=__cmd_x_main$port($k2);
  if($t3[0]===0){
    $t2=['port ',String($t3[1])];
  }else if($t3[0]===1){
    let $t4;
    const $t5=$t3[1];
    if($t5[0]===0){
      $t4=$t5[1];
    }else if($t5[0]===1){
      $t4=$t5[1];
    }else{
      $abort('no arm matched');
    }
    $t2=['bad ',$t4];
  }else{
    $abort('no arm matched');
  }
  $host_HostStdout_println(ctx_0[1],$t2);
  let $t7;
  const $t8=__cmd_x_main$port([]);
  if($t8[0]===0){
    $t7=['port ',String($t8[1])];
  }else if($t8[0]===1){
    let $t9;
    const $t10=$t8[1];
    if($t10[0]===0){
      $t9=$t10[1];
    }else if($t10[0]===1){
      $t9=$t10[1];
    }else{
      $abort('no arm matched');
    }
    $t7=['bad ',$t9];
  }else{
    $abort('no arm matched');
  }
  $host_HostStdout_println(ctx_0[1],$t7);
  return $k3;
}
function __cmd_x_main$lookup(pairs_0,key_1){
  const $t1=$list_find(pairs_0,p_2=>p_2[0]===key_1);
  if($t1[0]===0){
    return [0,$t1[1][1]];
  }else if($t1[0]===1){
    return $k4;
  }else{
    $abort('no arm matched');
  }
}
function __cmd_x_main$port(pairs_0){
  const key_4='port';
  let $t1;
  const $t2=__cmd_x_main$lookup(pairs_0,key_4);
  if($t2[0]===0){
    $t1=[0,$t2[1]];
  }else if($t2[0]===1){
    $t1=[1,[0,key_4]];
  }else{
    $abort('no arm matched');
  }
  const $t3=$t1;
  if($t3[0]!==0){
    return $t3;
  }
  const raw_1=$t3[1];
  const $t4=$str_toInt(raw_1);
  if($t4[0]===0){
    return [0,$t4[1]];
  }else if($t4[0]===1){
    return [1,[1,raw_1]];
  }else{
    $abort('no arm matched');
  }
}
