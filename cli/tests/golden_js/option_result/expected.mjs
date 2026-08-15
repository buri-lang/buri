function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  const pairs_1=[['port','8080'],['host','local']];
  const $t1=__cmd_x_main$lookup(pairs_1,'missing');
  core_host$HostStdout_println(ctx_0[1],[$t1[0]===0?$t1[1]:'none']);
  let $t2;
  const $t3=__cmd_x_main$port(pairs_1);
  if($t3[0]===0){
    $t2=['port ',String($t3[1])];
  }else if($t3[0]===1){
    $t2=['bad ',__cmd_x_main$reason($t3[1])];
  }else{
    $abort('no arm matched');
  }
  core_host$HostStdout_println(ctx_0[1],$t2);
  let $t4;
  const $t5=__cmd_x_main$port([]);
  if($t5[0]===0){
    $t4=['port ',String($t5[1])];
  }else if($t5[0]===1){
    $t4=['bad ',__cmd_x_main$reason($t5[1])];
  }else{
    $abort('no arm matched');
  }
  core_host$HostStdout_println(ctx_0[1],$t4);
  return [0,0];
}
function __cmd_x_main$lookup(pairs_0,key_1){
  const $t1=core_list$find$sarfai(pairs_0,p_2=>p_2[0]===key_1);
  if($t1[0]===0){
    return [0,$t1[1][1]];
  }else if($t1[0]===1){
    return [1];
  }else{
    $abort('no arm matched');
  }
}
function core_host$HostStdout_println(self_0,text_1){
  return $host_HostStdout_println(self_0,text_1);
}
function __cmd_x_main$port(pairs_0){
  const $t1=__cmd_x_main$require_(pairs_0,'port');
  if($t1[0]!==0){
    return $t1;
  }
  const raw_1=$t1[1];
  const $t2=core_str$Str_toInt(raw_1);
  if($t2[0]===0){
    return [0,$t2[1]];
  }else if($t2[0]===1){
    return [1,[1,raw_1]];
  }else{
    $abort('no arm matched');
  }
}
function __cmd_x_main$reason(b_0){
  if(b_0[0]===0){
    return b_0[1];
  }else if(b_0[0]===1){
    return b_0[1];
  }else{
    $abort('no arm matched');
  }
}
function __cmd_x_main$require_(pairs_0,key_1){
  const $t1=__cmd_x_main$lookup(pairs_0,key_1);
  if($t1[0]===0){
    return [0,$t1[1]];
  }else if($t1[0]===1){
    return [1,[0,key_1]];
  }else{
    $abort('no arm matched');
  }
}
function core_str$Str_toInt(self_0){
  return $str_toInt(self_0);
}
function core_list$find$sarfai(self_0,pred_1){
  return $list_find(self_0,pred_1);
}
