const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=['port','8080'];
const $k2=['host','local'];
const $k3=[$k1,$k2];
const $k4=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const self_8=__cmd_x_main_buri$lookup($k3,'missing');
  let $t1;
  if(self_8!==void 0){
    $t1=self_8;
  }else if(self_8===void 0){
    $t1='none';
  }else{
    $abort('no arm matched');
  }
  const fallback_3=$t1;
  const self_13=$host_HostStdout_println(ctx_1[1],fallback_3);
  let $t3;
  if(self_13[0]===0){
    $t3=0;
  }else if(self_13[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  let $t5;
  const $t6=__cmd_x_main_buri$port($k3);
  if($t6[0]===0){
    $t5='port '+String($t6[1]);
  }else if($t6[0]===1){
    let $t7;
    const $t8=$t6[1];
    if($t8[0]===0){
      $t7=$t8[1];
    }else if($t8[0]===1){
      $t7=$t8[1];
    }else{
      $abort('no arm matched');
    }
    $t5='bad '+$t7;
  }else{
    $abort('no arm matched');
  }
  const text_20=$t5;
  const self_21=$host_HostStdout_println(ctx_1[1],text_20);
  let $t9;
  if(self_21[0]===0){
    $t9=0;
  }else if(self_21[0]===1){
    $t9=0;
  }else{
    $abort('no arm matched');
  }
  let $t11;
  const $t12=__cmd_x_main_buri$port([]);
  if($t12[0]===0){
    $t11='port '+String($t12[1]);
  }else if($t12[0]===1){
    let $t13;
    const $t14=$t12[1];
    if($t14[0]===0){
      $t13=$t14[1];
    }else if($t14[0]===1){
      $t13=$t14[1];
    }else{
      $abort('no arm matched');
    }
    $t11='bad '+$t13;
  }else{
    $abort('no arm matched');
  }
  const text_28=$t11;
  const self_29=$host_HostStdout_println(ctx_1[1],text_28);
  let $t15;
  if(self_29[0]===0){
    $t15=0;
  }else if(self_29[0]===1){
    $t15=0;
  }else{
    $abort('no arm matched');
  }
  return $k4;
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
