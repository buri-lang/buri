const $k0=['port','8080'];
const $k1=['host','local'];
const $k2=[$k0,$k1];
const $k3=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  const self_7=__cmd_x_main_buri$lookup($k2,'missing');
  let $t1;
  if(self_7!==void 0){
    $t1=self_7;
  }else if(self_7===void 0){
    $t1='none';
  }else{
    $abort('no arm matched');
  }
  const fallback_2=$t1;
  const self_12=$host_HostStdout_println(ctx_0[1],fallback_2);
  let $t3;
  if(self_12[0]===0){
    $t3=0;
  }else if(self_12[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  let $t5;
  const $t6=__cmd_x_main_buri$port($k2);
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
  const text_19=$t5;
  const self_20=$host_HostStdout_println(ctx_0[1],text_19);
  let $t9;
  if(self_20[0]===0){
    $t9=0;
  }else if(self_20[0]===1){
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
  const text_27=$t11;
  const self_28=$host_HostStdout_println(ctx_0[1],text_27);
  let $t15;
  if(self_28[0]===0){
    $t15=0;
  }else if(self_28[0]===1){
    $t15=0;
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
