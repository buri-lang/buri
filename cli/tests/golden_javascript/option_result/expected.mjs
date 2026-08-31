const $k0=['port','8080'];
const $k1=['host','local'];
const $k2=[$k0,$k1];
const $k3=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  const $t1=__cmd_x_main_buri$lookup($k2,'missing');
  const self_9=$host_HostStdout_println(ctx_0[1],$t1!==void 0?$t1:'none');
  let $t2;
  if(self_9[0]===0){
    $t2=0;
  }else if(self_9[0]===1){
    $t2=0;
  }else{
    $abort('no arm matched');
  }
  let $t4;
  const $t5=__cmd_x_main_buri$port($k2);
  if($t5[0]===0){
    $t4='port '+String($t5[1]);
  }else if($t5[0]===1){
    let $t6;
    const $t7=$t5[1];
    if($t7[0]===0){
      $t6=$t7[1];
    }else if($t7[0]===1){
      $t6=$t7[1];
    }else{
      $abort('no arm matched');
    }
    $t4='bad '+$t6;
  }else{
    $abort('no arm matched');
  }
  const text_16=$t4;
  const self_17=$host_HostStdout_println(ctx_0[1],text_16);
  let $t8;
  if(self_17[0]===0){
    $t8=0;
  }else if(self_17[0]===1){
    $t8=0;
  }else{
    $abort('no arm matched');
  }
  let $t10;
  const $t11=__cmd_x_main_buri$port([]);
  if($t11[0]===0){
    $t10='port '+String($t11[1]);
  }else if($t11[0]===1){
    let $t12;
    const $t13=$t11[1];
    if($t13[0]===0){
      $t12=$t13[1];
    }else if($t13[0]===1){
      $t12=$t13[1];
    }else{
      $abort('no arm matched');
    }
    $t10='bad '+$t12;
  }else{
    $abort('no arm matched');
  }
  const text_24=$t10;
  const self_25=$host_HostStdout_println(ctx_0[1],text_24);
  let $t14;
  if(self_25[0]===0){
    $t14=0;
  }else if(self_25[0]===1){
    $t14=0;
  }else{
    $abort('no arm matched');
  }
  return $k3;
}
function __cmd_x_main_buri$lookup(pairs_0,key_1){
  const $t1=$list_find(pairs_0,p_2=>p_2[0]===key_1);
  if($t1!==void 0){
    return $t1[1];
  }else if($t1===void 0){
    return void 0;
  }else{
    $abort('no arm matched');
  }
}
function __cmd_x_main_buri$port(pairs_0){
  const pairs_3=$share(pairs_0);
  const key_4='port';
  let $t3;
  const $t2=__cmd_x_main_buri$lookup(pairs_3,key_4);
  if($t2!==void 0){
    $t3=[0,$t2];
  }else if($t2===void 0){
    $t3=[1,[0,key_4]];
  }else{
    $abort('no arm matched');
  }
  if($t3[0]!==0){
    return $t3;
  }
  const raw_1=$t3[1];
  const $t4=$str_toInt(raw_1);
  if($t4!==void 0){
    return [0,$t4];
  }else if($t4===void 0){
    return [1,[1,raw_1]];
  }else{
    $abort('no arm matched');
  }
}
